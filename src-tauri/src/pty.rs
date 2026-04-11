use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::io::{Read, Write};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
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

#[derive(Clone)]
enum EscapeState {
    None,
    Escape,
    Csi(String),
    Ss3,
}

impl Default for EscapeState {
    fn default() -> Self {
        Self::None
    }
}

#[derive(Clone, Default)]
struct InputState {
    line: Vec<char>,
    cursor: usize,
    escape: EscapeState,
}

impl InputState {
    fn clear_line(&mut self) {
        self.line.clear();
        self.cursor = 0;
        self.escape = EscapeState::None;
    }

    fn insert_char(&mut self, ch: char) {
        self.line.insert(self.cursor, ch);
        self.cursor += 1;
    }

    fn backspace(&mut self) {
        if self.cursor == 0 {
            return;
        }
        self.cursor -= 1;
        self.line.remove(self.cursor);
    }

    fn delete(&mut self) {
        if self.cursor < self.line.len() {
            self.line.remove(self.cursor);
        }
    }

    fn move_left(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
    }

    fn move_right(&mut self) {
        if self.cursor < self.line.len() {
            self.cursor += 1;
        }
    }

    fn move_home(&mut self) {
        self.cursor = 0;
    }

    fn move_end(&mut self) {
        self.cursor = self.line.len();
    }

    fn take_line(&mut self) -> String {
        let line = self.line.iter().collect();
        self.clear_line();
        line
    }

    fn apply_csi(&mut self, sequence: &str) {
        match sequence {
            "C" => self.move_right(),
            "D" => self.move_left(),
            "H" | "1~" | "7~" => self.move_home(),
            "F" | "4~" | "8~" => self.move_end(),
            "3~" => self.delete(),
            // Up/Down and other editing shortcuts can replace the whole shell line.
            // We can't reconstruct those mutations reliably from input alone.
            "A" | "B" => self.clear_line(),
            _ => self.clear_line(),
        }
    }

    fn apply_ss3(&mut self, code: char) {
        match code {
            'C' => self.move_right(),
            'D' => self.move_left(),
            'H' => self.move_home(),
            'F' => self.move_end(),
            _ => self.clear_line(),
        }
    }

    fn consume_escape_char(&mut self, ch: char) -> bool {
        match &mut self.escape {
            EscapeState::None => false,
            EscapeState::Escape => {
                self.escape = match ch {
                    '[' => EscapeState::Csi(String::new()),
                    'O' => EscapeState::Ss3,
                    _ => {
                        self.clear_line();
                        EscapeState::None
                    }
                };
                true
            }
            EscapeState::Csi(sequence) => {
                sequence.push(ch);
                if ('@'..='~').contains(&ch) {
                    let completed = std::mem::take(sequence);
                    self.escape = EscapeState::None;
                    self.apply_csi(&completed);
                }
                true
            }
            EscapeState::Ss3 => {
                self.escape = EscapeState::None;
                self.apply_ss3(ch);
                true
            }
        }
    }
}

const AI_COMMANDS: &[&str] = &["claude", "codex"];

/// 这些标志表示非交互命令（仅输出信息后退出），不应触发 AI 会话状态
const NON_INTERACTIVE_FLAGS: &[&str] = &["-v", "--version", "-h", "--help", "-p", "--print"];

/// AI 会话中的显式退出命令
const AI_EXIT_COMMANDS: &[&str] = &[
    "/exit", "exit", // Claude Code & Codex 通用
    "/quit", "quit",    // Claude Code & Codex 通用
    ":quit",   // Codex 交互式退出
    "/logout", // Codex 退出
];

/// 连续两次 Ctrl+C 退出的时间窗口
const DOUBLE_CTRLC_WINDOW: Duration = Duration::from_millis(1000);

/// 按下 Enter 后扫描输出以检测 AI 命令 echo 的时间窗口
const AI_ENTER_SCAN_WINDOW: Duration = Duration::from_millis(2000);

/// PTY resize 后的 TUI 重绘冷却窗口
///
/// 窗口内的 PTY 输出不刷新 last_output 时间戳。用于屏蔽 Claude/Codex 等 TUI
/// 应用在收到 ConPTY resize 信号后重绘 Alternate Screen Buffer 产生的伪输出,
/// 避免 process_monitor 把这些重绘误判为 AI 活跃,导致 ai-working 状态闪烁以及
/// 误触发 ai-working → ai-idle 的"任务完成"通知。
const RESIZE_COOLDOWN: Duration = Duration::from_millis(800);

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
                        if ('\x40'..='\x7e').contains(&c2) {
                            break;
                        }
                    }
                }
                Some(&'O') => {
                    chars.next();
                    chars.next();
                } // SS3: ESC O <final>
                _ => {
                    chars.next();
                } // other two-char escape
            }
        } else {
            result.push(c);
        }
    }
    result
}

/// 检查 PTY 输出中是否包含 AI 命令被 echo（例如 "PS C:\> claude" 或单独的 "claude"）
fn output_contains_ai_command(output: &str) -> bool {
    let stripped = strip_ansi_codes(output);
    for line in stripped.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        // 检查首词和末词（首词捕获纯命令行，末词捕获 "prompt> claude" 格式）
        let first = line.split_whitespace().next().unwrap_or("").to_lowercase();
        let last = line.split_whitespace().last().unwrap_or("").to_lowercase();
        for word in [&first, &last] {
            for &ai in AI_COMMANDS {
                if *word == ai
                    || word.ends_with(&format!("/{ai}"))
                    || word.ends_with(&format!("\\{ai}"))
                {
                    return true;
                }
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
    input_states: Arc<Mutex<HashMap<u32, InputState>>>,
    last_ctrlc: Arc<Mutex<HashMap<u32, Instant>>>,
    last_enter: Arc<Mutex<HashMap<u32, Instant>>>,
    /// resize 冷却窗口结束时间:在此之前 PTY 输出不刷新 last_output
    resize_cooldown_until: Arc<Mutex<HashMap<u32, Instant>>>,
}

impl PtyManager {
    pub fn new() -> Self {
        Self {
            instances: Arc::new(Mutex::new(HashMap::new())),
            next_id: Arc::new(Mutex::new(1)),
            last_output: Arc::new(Mutex::new(HashMap::new())),
            ai_sessions: Arc::new(Mutex::new(HashSet::new())),
            input_states: Arc::new(Mutex::new(HashMap::new())),
            last_ctrlc: Arc::new(Mutex::new(HashMap::new())),
            last_enter: Arc::new(Mutex::new(HashMap::new())),
            resize_cooldown_until: Arc::new(Mutex::new(HashMap::new())),
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

    /// 追踪用户输入，检测 AI 命令（claude/codex）的执行与退出
    ///
    /// 进入 AI 会话：在 shell 中输入 claude/codex + Enter
    /// 退出 AI 会话：Ctrl+D（EOF）、或输入退出命令（/exit /quit exit quit :quit /logout）
    /// 注意：Ctrl+C 在 AI 会话中是取消当前任务，不是退出会话
    pub fn track_input(&self, pty_id: u32, data: &str) {
        let in_ai = self.is_ai_session(pty_id);
        let mut enter_ai = false;
        let mut exit_ai = false;
        {
            let mut states = self.input_states.lock().unwrap();
            let state = states.entry(pty_id).or_default();
            for ch in data.chars() {
                if state.consume_escape_char(ch) {
                    continue;
                }
                match ch {
                    '\x1b' => {
                        state.escape = EscapeState::Escape;
                    }
                    '\x03' => {
                        state.clear_line();
                        if in_ai {
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
                        }
                    }
                    '\x04' => {
                        state.clear_line();
                        if in_ai {
                            // Ctrl+D (EOF) → 退出 AI 会话
                            exit_ai = true;
                        }
                    }
                    '\r' | '\n' => {
                        // 记录 Enter 时间，供输出扫描用
                        self.last_enter
                            .lock()
                            .unwrap()
                            .insert(pty_id, Instant::now());
                        let cmd = state.take_line().trim().to_lowercase();
                        if in_ai {
                            // AI 会话中：识别显式退出命令
                            if AI_EXIT_COMMANDS.iter().any(|&c| cmd == c) {
                                exit_ai = true;
                            }
                        } else if !cmd.is_empty() {
                            // 非 AI 会话：检测 AI 命令启动
                            let mut words = cmd.split_whitespace();
                            let first_word = words.next().unwrap_or("");
                            let is_ai_cmd = AI_COMMANDS.iter().any(|&ai| {
                                first_word == ai
                                    || first_word.ends_with(&format!("/{ai}"))
                                    || first_word.ends_with(&format!("\\{ai}"))
                            });
                            // 排除带有非交互标志的命令（如 claude -v, codex --help）
                            let has_non_interactive_flag = is_ai_cmd
                                && words.any(|w| NON_INTERACTIVE_FLAGS.iter().any(|&f| w == f));
                            if is_ai_cmd && !has_non_interactive_flag {
                                enter_ai = true;
                            }
                        }
                    }
                    '\x7f' | '\x08' => {
                        state.backspace();
                    }
                    c if c >= ' ' => state.insert_char(c),
                    _ => {}
                }
            }
        }
        if enter_ai || exit_ai {
            let mut sessions = self.ai_sessions.lock().unwrap();
            if enter_ai {
                sessions.insert(pty_id);
            } else {
                sessions.remove(&pty_id);
            }
        }
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
        .openpty(PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        })
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
    let ai_sessions_flush = state.ai_sessions.clone();
    let last_enter_flush = state.last_enter.clone();
    let resize_cooldown_flush = state.resize_cooldown_until.clone();
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
                        let _ = app_flush.emit(
                            "pty-output",
                            PtyOutputPayload {
                                pty_id: pty_id_for_reader,
                                data,
                            },
                        );
                    }

                    let exit_code = {
                        let mut instances = instances_clone.lock().unwrap();
                        if let Some(mut inst) = instances.remove(&pty_id_for_reader) {
                            inst.child
                                .try_wait()
                                .ok()
                                .flatten()
                                .map(|status| status.exit_code() as i32)
                                .unwrap_or(0)
                        } else {
                            0
                        }
                    };

                    let _ = app_flush.emit(
                        "pty-exit",
                        PtyExitPayload {
                            pty_id: pty_id_for_reader,
                            exit_code,
                        },
                    );
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
                            let expected_len = if byte >= 0xF0 {
                                4
                            } else if byte >= 0xE0 {
                                3
                            } else {
                                2
                            };
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

                    // 基于输出扫描检测 AI 会话（补偿上箭头历史调用 / PSReadLine 补全）：
                    // 若在 Enter 后 2 秒内收到包含 AI 命令 echo 的输出，自动标记为 AI 会话
                    {
                        let recently_entered = last_enter_flush
                            .lock()
                            .unwrap()
                            .get(&pty_id_for_reader)
                            .map(|t| t.elapsed() < AI_ENTER_SCAN_WINDOW)
                            .unwrap_or(false);
                        if recently_entered {
                            let mut sessions = ai_sessions_flush.lock().unwrap();
                            if !sessions.contains(&pty_id_for_reader)
                                && output_contains_ai_command(&data)
                            {
                                sessions.insert(pty_id_for_reader);
                            }
                        }
                    }

                    let _ = app_flush.emit(
                        "pty-output",
                        PtyOutputPayload {
                            pty_id: pty_id_for_reader,
                            data,
                        },
                    );

                    // 冷却窗口内(刚 resize 过)的输出不刷新 last_output。
                    // Claude/Codex 等 TUI 应用在 ConPTY resize 信号后会重绘
                    // Alternate Screen Buffer,这些重绘数据不能被 process_monitor
                    // 当作 AI 活跃信号,否则会触发 ai-working 状态闪烁和假完成通知。
                    let in_resize_cooldown = resize_cooldown_flush
                        .lock()
                        .ok()
                        .and_then(|m| m.get(&pty_id_for_reader).copied())
                        .map_or(false, |until| Instant::now() < until);

                    if !in_resize_cooldown {
                        if let Ok(mut map) = last_output.lock() {
                            map.insert(pty_id_for_reader, Instant::now());
                        }
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
        instances.insert(
            pty_id,
            PtyInstance {
                writer,
                master,
                child,
            },
        );
    }

    Ok(pty_id)
}

#[tauri::command]
pub fn write_pty(
    state: tauri::State<'_, PtyManager>,
    pty_id: u32,
    data: String,
) -> Result<(), String> {
    {
        let mut instances = state.instances.lock().unwrap();
        let instance = instances.get_mut(&pty_id).ok_or("PTY not found")?;
        instance
            .writer
            .write_all(data.as_bytes())
            .map_err(|e| e.to_string())?;
        instance.writer.flush().map_err(|e| e.to_string())?;
    }
    state.track_input(pty_id, &data);
    Ok(())
}

#[tauri::command]
pub fn resize_pty(
    state: tauri::State<'_, PtyManager>,
    pty_id: u32,
    cols: u16,
    rows: u16,
) -> Result<(), String> {
    {
        let instances = state.instances.lock().unwrap();
        let instance = instances.get(&pty_id).ok_or("PTY not found")?;
        instance
            .master
            .resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| e.to_string())?;
    }

    // 开启冷却窗口:之后 RESIZE_COOLDOWN 内的 PTY 输出(主要是 TUI 重绘)
    // 不会刷新 last_output,从而避免被 process_monitor 误判为 AI 活跃。
    state
        .resize_cooldown_until
        .lock()
        .unwrap()
        .insert(pty_id, Instant::now() + RESIZE_COOLDOWN);

    Ok(())
}

#[tauri::command]
pub fn kill_pty(state: tauri::State<'_, PtyManager>, pty_id: u32) -> Result<(), String> {
    // Remove metadata maps immediately so subsequent lookups return nothing.
    let instance = state.instances.lock().unwrap().remove(&pty_id);
    state.last_output.lock().unwrap().remove(&pty_id);
    state.ai_sessions.lock().unwrap().remove(&pty_id);
    state.input_states.lock().unwrap().remove(&pty_id);
    state.last_ctrlc.lock().unwrap().remove(&pty_id);
    state.last_enter.lock().unwrap().remove(&pty_id);
    state.resize_cooldown_until.lock().unwrap().remove(&pty_id);

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

    #[test]
    fn left_right_arrows_preserve_inline_edit_for_claude() {
        let mgr = PtyManager::new();
        mgr.track_input(1, "clade");
        mgr.track_input(1, "\x1b[D");
        mgr.track_input(1, "\x1b[D");
        mgr.track_input(1, "u");
        mgr.track_input(1, "\x1b[C");
        mgr.track_input(1, "\x1b[C");
        mgr.track_input(1, "\r");
        assert!(mgr.is_ai_session(1));
    }

    #[test]
    fn split_escape_sequence_still_moves_cursor() {
        let mgr = PtyManager::new();
        mgr.track_input(1, "clade");
        mgr.track_input(1, "\x1b");
        mgr.track_input(1, "[D");
        mgr.track_input(1, "\x1b");
        mgr.track_input(1, "[D");
        mgr.track_input(1, "u\r");
        assert!(mgr.is_ai_session(1));
    }

    #[test]
    fn edited_non_interactive_flag_does_not_start_ai_session() {
        let mgr = PtyManager::new();
        mgr.track_input(1, "claude --versin");
        mgr.track_input(1, "\x1b[D");
        mgr.track_input(1, "o\r");
        assert!(!mgr.is_ai_session(1));
    }
}
