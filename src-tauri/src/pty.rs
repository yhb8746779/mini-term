use portable_pty::{native_pty_system, CommandBuilder, PtySize, MasterPty, Child};
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::io::{Read, Write};
use std::sync::{Arc, Mutex};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter};

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct PtyOutputPayload {
    pty_id: u32,
    data: String,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct PtyExitPayload {
    pty_id: u32,
    exit_code: i32,
}

struct PtyInstance {
    writer: Box<dyn Write + Send>,
    master: Box<dyn MasterPty + Send>,
    child: Box<dyn Child + Send + Sync>,
}

const AI_COMMANDS: &[&str] = &["claude", "codex"];

/// 这些标志表示非交互命令（仅输出信息后退出），不应触发 AI 会话状态
const NON_INTERACTIVE_FLAGS: &[&str] = &[
    "-v", "--version",
    "-h", "--help",
    "-p", "--print",
];

/// AI 会话中的显式退出命令
const AI_EXIT_COMMANDS: &[&str] = &[
    "/exit", "exit",       // Claude Code & Codex 通用
    "/quit", "quit",       // Claude Code & Codex 通用
    ":quit",               // Codex 交互式退出
    "/logout",             // Codex 退出
];

/// 连续两次 Ctrl+C 退出的时间窗口
const DOUBLE_CTRLC_WINDOW: Duration = Duration::from_millis(1000);

/// PSReadLine 接受 inline prediction 后，shell echo 可能在 Enter 之后才到达 PTY。
/// 在这个短窗口内继续观察输出，避免“右箭头补全 + 回车”漏判 AI 会话。
const PREDICTION_ECHO_GRACE: Duration = Duration::from_millis(300);

/// 去除 ANSI 转义序列，返回纯文本
fn strip_ansi_codes(s: &str) -> String {
    let mut result = String::new();
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            match chars.peek() {
                Some(&'[') => {
                    chars.next(); // consume '['
                    // CSI sequence: skip until final byte (0x40–0x7E)
                    for c2 in chars.by_ref() {
                        if ('\x40'..='\x7e').contains(&c2) { break; }
                    }
                }
                Some(&'O') => { chars.next(); chars.next(); } // SS3: ESC O <final>
                _ => { chars.next(); } // other two-char escape
            }
        } else {
            result.push(c);
        }
    }
    result
}

/// 检查 PTY 输出中是否包含 AI 命令被 echo（例如 "PS C:\> claude" 或单独的 "claude"）
/// 同时过滤非交互式标志（-v/--version/-h/--help 等），避免误识别
fn output_contains_ai_command(output: &str) -> bool {
    let stripped = strip_ansi_codes(output).replace('\r', "\n");
    for line in stripped.lines() {
        let line = line.trim();
        if line.is_empty() { continue; }
        // 取提示符（> 或 $）后的命令部分（处理 PowerShell "PS C:\> cmd" 和
        // bash "user@host:~$ cmd" 格式）；若无提示符则检查整行。
        let cmd_part = line.rfind(|c| c == '>' || c == '$')
            .map(|pos| &line[pos + 1..])
            .unwrap_or(line)
            .trim();
        let mut words = cmd_part.split_whitespace();
        let first = words.next().unwrap_or("").to_lowercase();
        let is_ai = AI_COMMANDS.iter().any(|&ai| {
            first == ai
                || first.ends_with(&format!("/{ai}"))
                || first.ends_with(&format!("\\{ai}"))
        });
        if is_ai {
            let has_non_interactive = words.any(|w| {
                NON_INTERACTIVE_FLAGS.iter().any(|&f| w == f)
            });
            if !has_non_interactive {
                return true;
            }
        }
    }
    false
}

#[derive(Clone)]
pub struct PtyManager {
    instances: Arc<Mutex<HashMap<u32, PtyInstance>>>,
    next_id: Arc<Mutex<u32>>,
    last_output: Arc<Mutex<HashMap<u32, Instant>>>,
    ai_sessions: Arc<Mutex<HashSet<u32>>>,
    input_buffers: Arc<Mutex<HashMap<u32, String>>>,
    last_ctrlc: Arc<Mutex<HashMap<u32, Instant>>>,
    last_enter: Arc<Mutex<HashMap<u32, Instant>>>,
    /// PTY 自上次 Enter 以来输出的所有字符（上限 16KB）
    /// 用于检测 PSReadLine inline prediction（右箭头接受后 Enter）
    output_since_enter: Arc<Mutex<HashMap<u32, String>>>,
}

impl PtyManager {
    pub fn new() -> Self {
        Self {
            instances: Arc::new(Mutex::new(HashMap::new())),
            next_id: Arc::new(Mutex::new(1)),
            last_output: Arc::new(Mutex::new(HashMap::new())),
            ai_sessions: Arc::new(Mutex::new(HashSet::new())),
            input_buffers: Arc::new(Mutex::new(HashMap::new())),
            last_ctrlc: Arc::new(Mutex::new(HashMap::new())),
            last_enter: Arc::new(Mutex::new(HashMap::new())),
            output_since_enter: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn get_pty_ids(&self) -> Vec<u32> {
        self.instances.lock().unwrap().keys().copied().collect()
    }

    pub fn has_recent_output(&self, pty_id: u32, within: Duration) -> bool {
        let map = self.last_output.lock().unwrap();
        map.get(&pty_id).map_or(false, |t| t.elapsed() < within)
    }

    pub fn is_ai_session(&self, pty_id: u32) -> bool {
        self.ai_sessions.lock().unwrap().contains(&pty_id)
    }

    fn append_output_since_enter(&self, pty_id: u32, data: &str) -> String {
        let mut ose = self.output_since_enter.lock().unwrap();
        let entry = ose.entry(pty_id).or_default();
        entry.push_str(data);
        const OSE_CAP: usize = 16 * 1024;
        if entry.len() > OSE_CAP {
            let excess = entry.len() - OSE_CAP;
            let boundary = (excess..=entry.len())
                .find(|&i| entry.is_char_boundary(i))
                .unwrap_or(entry.len());
            entry.drain(..boundary);
        }
        entry.clone()
    }

    fn try_enter_ai_from_recent_output(&self, pty_id: u32, output: &str) {
        if self.is_ai_session(pty_id) {
            return;
        }
        let entered_recently = self.last_enter.lock().unwrap()
            .get(&pty_id)
            .map_or(false, |t| t.elapsed() <= PREDICTION_ECHO_GRACE);
        if entered_recently && output_contains_ai_command(output) {
            self.ai_sessions.lock().unwrap().insert(pty_id);
        }
    }

    /// 追踪用户输入，检测 AI 命令（claude/codex）的执行与退出
    ///
    /// 进入 AI 会话：在 shell 中输入 claude/codex + Enter
    /// 退出 AI 会话：Ctrl+D（EOF）、或输入退出命令（/exit /quit exit quit :quit /logout）
    /// 注意：Ctrl+C 在 AI 会话中是取消当前任务，不是退出会话
    pub fn track_input(&self, pty_id: u32, data: &str) {
        let in_ai = self.is_ai_session(pty_id);
        let mut enter_ai = false;
        let mut exit_ai = false;
        let mut entered = false;
        {
            let mut buffers = self.input_buffers.lock().unwrap();
            let buf = buffers.entry(pty_id).or_default();
            // 本地 ANSI 转义序列状态（xterm.js 每次发送完整转义序列）
            let mut skip_ansi = false; // 已遇 ESC，等待 [ 或 O
            let mut skip_csi  = false; // 在 CSI/SS3 序列中
            for ch in data.chars() {
                if skip_ansi {
                    skip_ansi = false;
                    if ch == '[' || ch == 'O' { skip_csi = true; }
                    continue;
                }
                if skip_csi {
                    if ('@'..='~').contains(&ch) { skip_csi = false; }
                    continue;
                }
                match ch {
                    '\x1b' => {
                        // 导航键/编辑键：清空缓冲区，防止 "[A" 等污染
                        buf.clear();
                        skip_ansi = true;
                    }
                    '\x03' if in_ai => {
                        // Ctrl+C: 单次取消当前任务，连续两次退出 AI 会话
                        let mut last = self.last_ctrlc.lock().unwrap();
                        let now = Instant::now();
                        if let Some(prev) = last.get(&pty_id) {
                            if now.duration_since(*prev) < DOUBLE_CTRLC_WINDOW {
                                exit_ai = true;
                                last.remove(&pty_id);
                            } else {
                                last.insert(pty_id, now);
                            }
                        } else {
                            last.insert(pty_id, now);
                        }
                        buf.clear();
                    }
                    '\x04' if in_ai => {
                        // Ctrl+D (EOF) → 退出 AI 会话
                        exit_ai = true;
                        buf.clear();
                    }
                    '\r' | '\n' => {
                        entered = true;
                        self.last_enter.lock().unwrap().insert(pty_id, Instant::now());
                        let cmd = buf.trim().to_lowercase();
                        if in_ai {
                            // AI 会话中：识别显式退出命令
                            if AI_EXIT_COMMANDS.iter().any(|&c| cmd == c) {
                                exit_ai = true;
                            }
                        } else if !cmd.is_empty() {
                            // 非 AI 会话：检测 AI 命令启动（直接键入路径）
                            let mut words = cmd.split_whitespace();
                            let first_word = words.next().unwrap_or("");
                            let is_ai_cmd = AI_COMMANDS.iter().any(|&ai| {
                                first_word == ai
                                    || first_word.ends_with(&format!("/{ai}"))
                                    || first_word.ends_with(&format!("\\{ai}"))
                            });
                            // 排除带有非交互标志的命令（如 claude -v, codex --help）
                            let has_non_interactive_flag = is_ai_cmd && words.any(|w| {
                                NON_INTERACTIVE_FLAGS.iter().any(|&f| w == f)
                            });
                            if is_ai_cmd && !has_non_interactive_flag { enter_ai = true; }
                        }
                        buf.clear();
                    }
                    '\x7f' | '\x08' => { buf.pop(); }
                    c if c >= ' ' => buf.push(c),
                    _ => {}
                }
            }
        }

        // PSReadLine inline prediction 补偿：当用户按右箭头接受预测文本后再 Enter 时，
        // input_buffers 中的 buf 因 ESC 序列清空而为空，直接输入检测无法命中。
        // 此时扫描 output_since_enter（PTY 在 Enter 前渲染到屏幕的内容）来识别 AI 命令。
        if entered && !in_ai && !enter_ai {
            let ose = self.output_since_enter.lock().unwrap();
            if let Some(ose_data) = ose.get(&pty_id) {
                if output_contains_ai_command(ose_data) {
                    enter_ai = true;
                }
            }
        }
        // Enter 后重置 output_since_enter，为下一条命令重新积累
        if entered {
            self.output_since_enter.lock().unwrap().insert(pty_id, String::new());
        }

        if enter_ai || exit_ai {
            let mut sessions = self.ai_sessions.lock().unwrap();
            if enter_ai { sessions.insert(pty_id); } else { sessions.remove(&pty_id); }
        }
    }

    /// 仅供单元测试使用：向 output_since_enter 注入模拟 PTY 输出
    #[cfg(test)]
    fn inject_pty_output(&self, pty_id: u32, data: &str) {
        let output = self.append_output_since_enter(pty_id, data);
        self.try_enter_ai_from_recent_output(pty_id, &output);
    }
}

#[tauri::command]
pub fn create_pty(
    app: AppHandle,
    state: tauri::State<'_, PtyManager>,
    shell: String,
    args: Vec<String>,
    cwd: String,
) -> Result<u32, String> {
    let pty_system = native_pty_system();
    let pair = pty_system
        // 先以较宽的初始尺寸启动 shell，避免 UI 首屏布局未稳定时 banner/prompt 被硬换行。
        .openpty(PtySize { rows: 40, cols: 200, pixel_width: 0, pixel_height: 0 })
        .map_err(|e| e.to_string())?;

    let mut cmd = CommandBuilder::new(&shell);
    for arg in &args {
        cmd.arg(arg);
    }
    cmd.cwd(&cwd);

    // Advertise terminal capabilities so TUI apps (Claude Code, etc.)
    // enable colors and advanced cursor rendering.
    cmd.env("TERM", "xterm-256color");
    cmd.env("COLORTERM", "truecolor");
    // Ensure UTF-8 encoding for proper CJK/emoji rendering.
    // Only set LC_CTYPE to avoid overriding the user's locale preferences.
    cmd.env("LC_CTYPE", "UTF-8");

    let child = pair.slave.spawn_command(cmd).map_err(|e| e.to_string())?;

    let pty_id = {
        let mut next = state.next_id.lock().unwrap();
        let id = *next;
        *next += 1;
        id
    };

    let writer = pair.master.take_writer().map_err(|e| e.to_string())?;
    let mut reader = pair.master.try_clone_reader().map_err(|e| e.to_string())?;
    let master = pair.master;

    // Channel + flush 定时器实现 16ms 批量缓冲
    let (tx, rx) = mpsc::channel::<Vec<u8>>();
    let instances_clone = state.instances.clone();
    let pty_id_for_reader = pty_id;

    thread::spawn(move || {
        let mut buf = [0u8; 4096];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    if tx.send(buf[..n].to_vec()).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });

    let app_flush = app.clone();
    let last_output = state.last_output.clone();
    let pty_state_for_output = state.inner().clone();
    thread::spawn(move || {
        let mut pending = Vec::new();

        loop {
            match rx.recv_timeout(Duration::from_millis(16)) {
                Ok(data) => {
                    pending.extend(data);
                    while let Ok(more) = rx.try_recv() {
                        pending.extend(more);
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    if !pending.is_empty() {
                        let data = String::from_utf8_lossy(&pending).into_owned();
                        let _ = app_flush.emit("pty-output", PtyOutputPayload {
                            pty_id: pty_id_for_reader, data,
                        });
                    }

                    let exit_code = {
                        let mut instances = instances_clone.lock().unwrap();
                        if let Some(mut inst) = instances.remove(&pty_id_for_reader) {
                            inst.child.try_wait()
                                .ok()
                                .flatten()
                                .map(|status| status.exit_code() as i32)
                                .unwrap_or(0)
                        } else {
                            0
                        }
                    };

                    let _ = app_flush.emit("pty-exit", PtyExitPayload {
                        pty_id: pty_id_for_reader,
                        exit_code,
                    });
                    return;
                }
            }

            if !pending.is_empty() {
                // 找到最后一个完整 UTF-8 字符的边界，避免截断多字节字符
                let valid_len = {
                    let mut i = pending.len();
                    // 从末尾向前扫描，找到可能不完整的 UTF-8 序列起始位置
                    while i > 0 {
                        i -= 1;
                        let byte = pending[i];
                        if byte < 0x80 {
                            // ASCII 字符，本身就是完整的
                            i = pending.len();
                            break;
                        } else if byte >= 0xC0 {
                            // 多字节序列的起始字节，检查序列是否完整
                            let expected_len = if byte >= 0xF0 { 4 }
                                else if byte >= 0xE0 { 3 }
                                else { 2 };
                            let remaining = pending.len() - i;
                            if remaining >= expected_len {
                                // 序列完整
                                i = pending.len();
                            }
                            // 否则 i 就是不完整序列的起始位置
                            break;
                        }
                        // 0x80..0xBF 是延续字节，继续向前找起始字节
                    }
                    i
                };

                if valid_len > 0 {
                    let data = String::from_utf8_lossy(&pending[..valid_len]).into_owned();

                    // 将本批输出追加到 output_since_enter，供 track_input 在 Enter 时检测
                    // PSReadLine inline prediction（右箭头接受）会在 Enter 前渲染命令文本
                    let recent_output = pty_state_for_output
                        .append_output_since_enter(pty_id_for_reader, &data);
                    pty_state_for_output
                        .try_enter_ai_from_recent_output(pty_id_for_reader, &recent_output);

                    let _ = app_flush.emit("pty-output", PtyOutputPayload {
                        pty_id: pty_id_for_reader, data,
                    });
                    if let Ok(mut map) = last_output.lock() {
                        map.insert(pty_id_for_reader, Instant::now());
                    }
                }

                // 保留不完整的 UTF-8 字节到下次刷新
                if valid_len < pending.len() {
                    let leftover = pending[valid_len..].to_vec();
                    pending.clear();
                    pending.extend(leftover);
                } else {
                    pending.clear();
                }
            }
        }
    });

    {
        let mut instances = state.instances.lock().unwrap();
        instances.insert(pty_id, PtyInstance {
            writer,
            master,
            child,
        });
    }

    Ok(pty_id)
}

#[tauri::command]
pub fn write_pty(state: tauri::State<'_, PtyManager>, pty_id: u32, data: String) -> Result<(), String> {
    {
        let mut instances = state.instances.lock().unwrap();
        let instance = instances.get_mut(&pty_id).ok_or("PTY not found")?;
        instance.writer.write_all(data.as_bytes()).map_err(|e| e.to_string())?;
        instance.writer.flush().map_err(|e| e.to_string())?;
    }
    state.track_input(pty_id, &data);
    Ok(())
}

#[tauri::command]
pub fn resize_pty(state: tauri::State<'_, PtyManager>, pty_id: u32, cols: u16, rows: u16) -> Result<(), String> {
    let instances = state.instances.lock().unwrap();
    let instance = instances.get(&pty_id).ok_or("PTY not found")?;
    instance.master
        .resize(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 })
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn kill_pty(state: tauri::State<'_, PtyManager>, pty_id: u32) -> Result<(), String> {
    // Remove metadata maps immediately so subsequent lookups return nothing.
    let instance = state.instances.lock().unwrap().remove(&pty_id);
    state.last_output.lock().unwrap().remove(&pty_id);
    state.ai_sessions.lock().unwrap().remove(&pty_id);
    state.input_buffers.lock().unwrap().remove(&pty_id);
    state.last_ctrlc.lock().unwrap().remove(&pty_id);
    state.last_enter.lock().unwrap().remove(&pty_id);
    state.output_since_enter.lock().unwrap().remove(&pty_id);

    // Drop the PTY instance on a background thread.
    //
    // On Windows, dropping `master` triggers `ClosePseudoConsole()`, which is
    // synchronous and blocks until every process in the console session exits.
    // When a long-running AI process (claude/codex) is still alive, this call
    // never returns on the calling thread, freezing the whole app ("未响应").
    //
    // Fix: kill the shell process first (stops new output), then drop on a
    // background thread so the UI stays responsive regardless of how long
    // cleanup takes.
    if let Some(mut inst) = instance {
        thread::spawn(move || {
            // Kill the shell (e.g., pwsh). This signals the ConPTY server that
            // the primary process is gone, allowing ClosePseudoConsole to return
            // once in-flight output is drained.
            let _ = inst.child.kill();
            // Now drop writer → master → child in background.
            drop(inst);
        });
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── output_contains_ai_command ───────────────────────────────────────

    #[test]
    fn echo_plain_claude_detected() {
        assert!(output_contains_ai_command("claude"));
    }

    #[test]
    fn echo_powershell_prompt_claude_with_args_detected() {
        // 用 Up 箭头从历史调用 "claude --model sonnet" 后 Enter，
        // shell echo 格式为 "PS C:\workspace> claude --model sonnet"
        assert!(output_contains_ai_command(
            "PS C:\\workspace\\self\\mini-term> claude --model sonnet"
        ));
    }

    #[test]
    fn echo_powershell_prompt_plain_claude_detected() {
        assert!(output_contains_ai_command("PS C:\\Users\\foo> claude"));
    }

    #[test]
    fn echo_bash_prompt_claude_detected() {
        assert!(output_contains_ai_command("user@host:~$ claude --model opus"));
    }

    #[test]
    fn echo_last_word_sonnet_not_false_positive() {
        // 不能只因为某行最后一个词是 "sonnet" 就触发
        assert!(!output_contains_ai_command("npm install sonnet"));
    }

    // ── track_input / AI session detection ─────────────────────────────

    #[test]
    fn detect_claude_command() {
        let mgr = PtyManager::new();
        mgr.track_input(1, "claude\r");
        assert!(mgr.is_ai_session(1));
    }

    #[test]
    fn detect_codex_command() {
        let mgr = PtyManager::new();
        mgr.track_input(1, "codex\r");
        assert!(mgr.is_ai_session(1));
    }

    #[test]
    fn non_ai_command_not_ai_session() {
        let mgr = PtyManager::new();
        mgr.track_input(1, "npm install\r");
        assert!(!mgr.is_ai_session(1));
    }

    #[test]
    fn prompt_in_ai_session_stays() {
        let mgr = PtyManager::new();
        mgr.track_input(1, "claude\r");
        assert!(mgr.is_ai_session(1));
        // 在 Claude 内输入提示词不应退出 AI 会话
        mgr.track_input(1, "fix the bug\r");
        assert!(mgr.is_ai_session(1));
    }

    #[test]
    fn single_ctrl_c_does_not_exit_ai_session() {
        let mgr = PtyManager::new();
        mgr.track_input(1, "claude\r");
        assert!(mgr.is_ai_session(1));
        // 单次 Ctrl+C 是取消当前任务，不退出
        mgr.track_input(1, "\x03");
        assert!(mgr.is_ai_session(1));
    }

    #[test]
    fn double_ctrl_c_exits_ai_session() {
        let mgr = PtyManager::new();
        mgr.track_input(1, "claude\r");
        assert!(mgr.is_ai_session(1));
        // 连续两次 Ctrl+C 退出 AI 会话
        mgr.track_input(1, "\x03");
        assert!(mgr.is_ai_session(1));
        mgr.track_input(1, "\x03");
        assert!(!mgr.is_ai_session(1));
    }

    #[test]
    fn ctrl_d_exits_ai_session() {
        let mgr = PtyManager::new();
        mgr.track_input(1, "claude\r");
        assert!(mgr.is_ai_session(1));
        mgr.track_input(1, "\x04");
        assert!(!mgr.is_ai_session(1));
    }

    #[test]
    fn slash_exit_exits_ai_session() {
        let mgr = PtyManager::new();
        mgr.track_input(1, "claude\r");
        assert!(mgr.is_ai_session(1));
        mgr.track_input(1, "/exit\r");
        assert!(!mgr.is_ai_session(1));
    }

    #[test]
    fn exit_command_exits_ai_session() {
        let mgr = PtyManager::new();
        mgr.track_input(1, "claude\r");
        assert!(mgr.is_ai_session(1));
        mgr.track_input(1, "exit\r");
        assert!(!mgr.is_ai_session(1));
    }

    #[test]
    fn slash_quit_exits_ai_session() {
        let mgr = PtyManager::new();
        mgr.track_input(1, "claude\r");
        assert!(mgr.is_ai_session(1));
        mgr.track_input(1, "/quit\r");
        assert!(!mgr.is_ai_session(1));
    }

    #[test]
    fn quit_exits_ai_session() {
        let mgr = PtyManager::new();
        mgr.track_input(1, "codex\r");
        assert!(mgr.is_ai_session(1));
        mgr.track_input(1, "quit\r");
        assert!(!mgr.is_ai_session(1));
    }

    #[test]
    fn colon_quit_exits_codex_session() {
        let mgr = PtyManager::new();
        mgr.track_input(1, "codex\r");
        assert!(mgr.is_ai_session(1));
        mgr.track_input(1, ":quit\r");
        assert!(!mgr.is_ai_session(1));
    }

    #[test]
    fn slash_logout_exits_codex_session() {
        let mgr = PtyManager::new();
        mgr.track_input(1, "codex\r");
        assert!(mgr.is_ai_session(1));
        mgr.track_input(1, "/logout\r");
        assert!(!mgr.is_ai_session(1));
    }

    #[test]
    fn claude_with_interactive_args() {
        let mgr = PtyManager::new();
        mgr.track_input(1, "claude --model opus\r");
        assert!(mgr.is_ai_session(1));
    }

    #[test]
    fn claude_version_not_ai_session() {
        let mgr = PtyManager::new();
        mgr.track_input(1, "claude -v\r");
        assert!(!mgr.is_ai_session(1));
    }

    #[test]
    fn claude_long_version_not_ai_session() {
        let mgr = PtyManager::new();
        mgr.track_input(1, "claude --version\r");
        assert!(!mgr.is_ai_session(1));
    }

    #[test]
    fn claude_help_not_ai_session() {
        let mgr = PtyManager::new();
        mgr.track_input(1, "claude -h\r");
        assert!(!mgr.is_ai_session(1));
    }

    #[test]
    fn claude_print_not_ai_session() {
        let mgr = PtyManager::new();
        mgr.track_input(1, "claude -p \"hello\"\r");
        assert!(!mgr.is_ai_session(1));
    }

    #[test]
    fn codex_version_not_ai_session() {
        let mgr = PtyManager::new();
        mgr.track_input(1, "codex --version\r");
        assert!(!mgr.is_ai_session(1));
    }

    #[test]
    fn codex_help_not_ai_session() {
        let mgr = PtyManager::new();
        mgr.track_input(1, "codex --help\r");
        assert!(!mgr.is_ai_session(1));
    }

    #[test]
    fn backspace_corrects_input() {
        let mgr = PtyManager::new();
        mgr.track_input(1, "claue\x7fde\r");
        assert!(mgr.is_ai_session(1));
    }

    #[test]
    fn empty_enter_keeps_ai_session() {
        let mgr = PtyManager::new();
        mgr.track_input(1, "claude\r");
        assert!(mgr.is_ai_session(1));
        mgr.track_input(1, "\r");
        assert!(mgr.is_ai_session(1));
    }

    #[test]
    fn char_by_char_input() {
        let mgr = PtyManager::new();
        for ch in "claude\r".chars() {
            mgr.track_input(1, &ch.to_string());
        }
        assert!(mgr.is_ai_session(1));
    }

    // ── output_since_enter / PSReadLine inline prediction ───────────────

    #[test]
    fn psreadline_right_arrow_accept_detected() {
        // 模拟：用户按右箭头接受 PSReadLine 预测 "claude --model sonnet"，
        // 此时 output_since_enter 中有渲染文本，但 input_buffers 为空（ESC 序列清空了）
        let mgr = PtyManager::new();
        // 注入 PTY 渲染的提示行（PSReadLine 将接受后的命令渲染到终端）
        mgr.inject_pty_output(1, "PS C:\\workspace> claude --model sonnet");
        // 只发送 Enter（buf 为空，直接输入检测无法命中）
        mgr.track_input(1, "\r");
        assert!(mgr.is_ai_session(1));
    }

    #[test]
    fn psreadline_echo_arrives_after_enter_still_detected() {
        let mgr = PtyManager::new();
        mgr.track_input(1, "\r");
        mgr.inject_pty_output(1, "PS C:\\workspace> claude --model sonnet");
        assert!(mgr.is_ai_session(1));
    }

    #[test]
    fn psreadline_version_flag_not_detected() {
        // PSReadLine 渲染了 "claude -v" —— 非交互，不应触发 AI 会话
        let mgr = PtyManager::new();
        mgr.inject_pty_output(1, "PS C:\\workspace> claude -v");
        mgr.track_input(1, "\r");
        assert!(!mgr.is_ai_session(1));
    }

    #[test]
    fn output_since_enter_resets_after_enter() {
        // 第一次 Enter 后 output_since_enter 应重置；
        // 第二次 Enter 时如果没有新的 AI 输出，不应误触发
        let mgr = PtyManager::new();
        mgr.inject_pty_output(1, "PS C:\\workspace> claude");
        mgr.track_input(1, "\r");
        assert!(mgr.is_ai_session(1));

        // 退出 AI 会话
        mgr.track_input(1, "/exit\r");
        assert!(!mgr.is_ai_session(1));

        // 此时 output_since_enter 已清空；再 Enter 不应触发
        mgr.track_input(1, "\r");
        assert!(!mgr.is_ai_session(1));
    }

    #[test]
    fn output_contains_ai_command_version_flag_false() {
        assert!(!output_contains_ai_command("PS C:\\> claude -v"));
        assert!(!output_contains_ai_command("PS C:\\> claude --version"));
        assert!(!output_contains_ai_command("PS C:\\> claude -h"));
        assert!(!output_contains_ai_command("PS C:\\> claude --help"));
    }

    #[test]
    fn output_contains_ai_command_plain_claude_true() {
        assert!(output_contains_ai_command("PS C:\\> claude"));
        assert!(output_contains_ai_command("PS C:\\workspace> claude --model sonnet"));
    }

    #[test]
    fn output_contains_ai_command_with_carriage_return_true() {
        assert!(output_contains_ai_command("PS C:\\workspace> cl\rPS C:\\workspace> claude --model sonnet"));
    }
}
