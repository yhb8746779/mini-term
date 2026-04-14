use portable_pty::{native_pty_system, CommandBuilder, PtySize, MasterPty, Child};
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::io::{Read, Write};
use std::sync::{Arc, Mutex};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter};

/// 从命令 token（已小写）中提取 AI provider 名称
fn detect_provider_from_token(token: &str) -> Option<&'static str> {
    for &ai in AI_COMMANDS {
        if token == ai || token.ends_with(&format!("/{ai}")) || token.ends_with(&format!("\\{ai}")) {
            return Some(ai);
        }
    }
    None
}

/// 从看起来像命令调用的单行中提取被调用的 AI provider 名称。
/// 与 line_contains_ai_command 共用提示符识别逻辑，避免把 AI 回答里
/// 提到的模型名（如 "Codex" 出现在输出文本中）误认为 provider。
fn extract_provider_from_command_line(line: &str) -> Option<&'static str> {
    let line = line.trim();
    if line.is_empty() { return None; }

    const TERMINAL_PROMPT_CHARS: &[char] = &['>', '$', '%', '#'];
    const UNICODE_PROMPT_CHARS: &[char] = &['❯', '➜', '›', 'λ'];

    let cmd_start: &str = if let Some(pos) = line.rfind(TERMINAL_PROMPT_CHARS) {
        let ch = line[pos..].chars().next().unwrap();
        line[pos + ch.len_utf8()..].trim()
    } else if let Some(pos) = line.rfind(UNICODE_PROMPT_CHARS) {
        let ch = line[pos..].chars().next().unwrap();
        line[pos + ch.len_utf8()..].trim()
    } else {
        line
    };

    let first_token = cmd_start.split_whitespace().next().unwrap_or("");
    detect_provider_from_token(&first_token.to_lowercase())
}

/// 从原始输出文本中提取最后一次调用的 AI provider 名称。
/// 只识别看起来像命令调用的行（通过 extract_provider_from_command_line），
/// 忽略 AI 回答正文里提到的模型名，防止 provider 被错误染色。
fn detect_provider_from_output(output: &str) -> Option<&'static str> {
    let stripped = strip_ansi_codes(output).replace('\r', "\n");
    let mut last_found: Option<&'static str> = None;
    for line in stripped.lines() {
        let collapsed = apply_backspaces(line);
        if let Some(provider) = extract_provider_from_command_line(&collapsed) {
            last_found = Some(provider);
        }
    }
    last_found
}

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

const AI_COMMANDS: &[&str] = &["claude", "codex", "gemini"];

/// 这些标志表示非交互命令（仅输出信息后退出），不应触发 AI 会话状态
const NON_INTERACTIVE_FLAGS: &[&str] = &[
    "-v", "--version",
    "-h", "--help",
    "-p", "--print",
];

/// AI 会话中的显式退出命令
const AI_EXIT_COMMANDS: &[&str] = &[
    "/exit", "exit",       // Claude Code & Codex & Gemini 通用
    "/quit", "quit",       // Claude Code & Codex & Gemini 通用
    ":quit",               // Codex 交互式退出
    "/logout",             // Codex 退出
];

/// 连续两次 Ctrl+C 退出的时间窗口
const DOUBLE_CTRLC_WINDOW: Duration = Duration::from_millis(1000);

/// PSReadLine 接受 inline prediction 后，shell echo 可能在 Enter 之后才到达 PTY。
/// 在这个短窗口内继续观察输出，避免”右箭头补全 + 回车”漏判 AI 会话。
const PREDICTION_ECHO_GRACE: Duration = Duration::from_millis(300);

/// TUI 应用（Claude/Codex）在收到 ConPTY resize 信号后会重绘 Alternate Screen Buffer。
/// 在此冷却窗口内的 PTY 输出不刷新 last_output，避免误判为 AI 活跃。
const RESIZE_COOLDOWN: Duration = Duration::from_millis(800);

/// 去除 ANSI 转义序列，返回纯文本。
///
/// 特殊处理 CSI G（Cursor Horizontal Absolute，光标横向绝对定位）：
/// PSReadLine 等行编辑器用 `\x1b[NG]` 覆写当前行内容（历史导航、上下箭头）。
/// 直接丢弃这个序列会导致旧命令与新命令字节拼接成乱码，无法识别 AI 命令。
/// 将 CSI G 转义为 `\r`，配合后续 `replace('\r', '\n')` 把行在覆写点断开，
/// 使最终写入的命令（如 `claude`）出现在独立的一行中可被正确识别。
fn strip_ansi_codes(s: &str) -> String {
    let mut result = String::new();
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            match chars.peek() {
                Some(&'[') => {
                    chars.next(); // consume '['
                    // CSI sequence: collect until final byte (0x40–0x7E)
                    let mut final_byte = '\0';
                    for c2 in chars.by_ref() {
                        if ('\x40'..='\x7e').contains(&c2) {
                            final_byte = c2;
                            break;
                        }
                    }
                    // CHA (Cursor Horizontal Absolute) = 'G'
                    // PSReadLine 用此序列在行内定位后覆写命令，等价于 \r
                    if final_byte == 'G' {
                        result.push('\r');
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

/// 应用退格语义：将每个 \x08 (BS) 视为"删除前一个字符"
///
/// zsh-syntax-highlighting 等插件在每次按键时会用 BS 回退再重绘带颜色的字符，
/// 导致 PTY 流中出现 `c\bc` 这样的字节序列。strip_ansi 只去 ANSI 转义，
/// 不处理 BS，会把 token 切成 `c\bc\bclaude` 之类的乱码，匹配失败。
fn apply_backspaces(line: &str) -> String {
    let mut result = String::new();
    for ch in line.chars() {
        if ch == '\x08' {
            result.pop();
        } else {
            result.push(ch);
        }
    }
    result
}

/// 判断一个 token 是否是 AI 命令（精确匹配或路径结尾匹配）
fn is_ai_command_token(token: &str) -> bool {
    let t = token.to_lowercase();
    AI_COMMANDS.iter().any(|&ai| {
        t == ai
            || t.ends_with(&format!("/{ai}"))
            || t.ends_with(&format!("\\{ai}"))
    })
}

/// 判断参数迭代器中是否包含非交互式标志（-v/--version/-h/--help/-p/--print）
fn has_non_interactive_flag<'a>(args: impl Iterator<Item = &'a str>) -> bool {
    args.into_iter().any(|w| NON_INTERACTIVE_FLAGS.iter().any(|&f| w == f))
}

/// 检查单行文本是否含有 AI 命令的 echo。
///
/// 三层识别策略：
/// 1. **提示符 fast path**：
///    - 终端提示符（`>` `$` `%` `#`）：出现在 prompt 末尾，命令紧随其后；
///      用 rfind 找最后一个，仅检查其后首 token。
///    - Unicode 主题提示符（`❯` `➜` `›` `λ`）：出现在行首，命令在目录信息之后；
///      扫描其后所有 token，找到第一个 AI 命令 token。
/// 2. **Token fallback**：无提示符时，对整行首 token 做检查。
/// 3. **保守性约束**：fallback 时只匹配行首第一个 token，防止
///    `npm install @anthropic-ai/claude-sdk` 等中间词被误判。
fn line_contains_ai_command(line: &str) -> bool {
    let line = line.trim();
    if line.is_empty() {
        return false;
    }

    // 终端提示符：出现在行尾附近，命令紧跟其后，只检查首 token
    const TERMINAL_PROMPT_CHARS: &[char] = &['>', '$', '%', '#'];
    // Unicode 主题提示符：出现在行首，命令在目录名之后，需扫描所有 token
    const UNICODE_PROMPT_CHARS: &[char] = &['❯', '➜', '›', 'λ'];

    // 第一层：先找终端提示符（rfind，命中最靠近命令的那个）
    if let Some(pos) = line.rfind(TERMINAL_PROMPT_CHARS) {
        let ch = line[pos..].chars().next().unwrap();
        let cmd_part = line[pos + ch.len_utf8()..].trim();
        let mut words = cmd_part.split_whitespace();
        let first = words.next().unwrap_or("");
        if is_ai_command_token(first) {
            return !has_non_interactive_flag(words);
        }
        // 终端提示符存在但其后首 token 不是 AI → 此行不匹配
        return false;
    }

    // 第一层（续）：Unicode 主题提示符（❯ ➜ › λ）位于行首，命令在目录信息之后；
    // 扫描提示符后所有 token，找到第一个 AI 命令 token。
    if let Some(pos) = line.rfind(UNICODE_PROMPT_CHARS) {
        let ch = line[pos..].chars().next().unwrap();
        let cmd_part = line[pos + ch.len_utf8()..].trim();
        let tokens: Vec<&str> = cmd_part.split_whitespace().collect();
        for (i, &tok) in tokens.iter().enumerate() {
            if is_ai_command_token(tok) {
                return !has_non_interactive_flag(tokens[i + 1..].iter().copied());
            }
        }
        return false;
    }

    // 第二层 + 第三层：整行 token fallback，只看行首第一个 token
    let mut words = line.split_whitespace();
    let first = words.next().unwrap_or("");
    if is_ai_command_token(first) {
        return !has_non_interactive_flag(words);
    }

    false
}

/// 检查 PTY 输出中是否包含 AI 命令被 echo（支持 PS/bash/zsh/fish/主题提示符）
/// 同时过滤非交互式标志（-v/--version/-h/--help 等），避免误识别
fn output_contains_ai_command(output: &str) -> bool {
    let stripped = strip_ansi_codes(output).replace('\r', "\n");
    stripped.lines().any(|line| {
        let collapsed = apply_backspaces(line);
        line_contains_ai_command(&collapsed)
    })
}

#[derive(Clone)]
pub struct PtyManager {
    instances: Arc<Mutex<HashMap<u32, PtyInstance>>>,
    next_id: Arc<Mutex<u32>>,
    last_output: Arc<Mutex<HashMap<u32, Instant>>>,
    ai_sessions: Arc<Mutex<HashSet<u32>>>,
    /// 当前 AI 会话的 provider（"claude" / "codex" / "gemini"）
    ai_providers: Arc<Mutex<HashMap<u32, String>>>,
    input_buffers: Arc<Mutex<HashMap<u32, String>>>,
    last_ctrlc: Arc<Mutex<HashMap<u32, Instant>>>,
    last_enter: Arc<Mutex<HashMap<u32, Instant>>>,
    /// PTY 自上次 Enter 以来输出的所有字符（上限 16KB）
    /// 用于检测 PSReadLine inline prediction（右箭头接受后 Enter）
    output_since_enter: Arc<Mutex<HashMap<u32, String>>>,
    /// resize 冷却窗口结束时间：在此之前 PTY 输出不刷新 last_output，
    /// 避免 TUI 应用 resize 重绘被误判为 AI 活跃。
    resize_cooldown_until: Arc<Mutex<HashMap<u32, Instant>>>,
    /// 近期输出滚动窗口（上限 8KB），供 process_monitor 检测 awaiting-input 短语
    recent_output_window: Arc<Mutex<HashMap<u32, String>>>,
}

impl PtyManager {
    pub fn new() -> Self {
        Self {
            instances: Arc::new(Mutex::new(HashMap::new())),
            next_id: Arc::new(Mutex::new(1)),
            last_output: Arc::new(Mutex::new(HashMap::new())),
            ai_sessions: Arc::new(Mutex::new(HashSet::new())),
            ai_providers: Arc::new(Mutex::new(HashMap::new())),
            input_buffers: Arc::new(Mutex::new(HashMap::new())),
            last_ctrlc: Arc::new(Mutex::new(HashMap::new())),
            last_enter: Arc::new(Mutex::new(HashMap::new())),
            output_since_enter: Arc::new(Mutex::new(HashMap::new())),
            resize_cooldown_until: Arc::new(Mutex::new(HashMap::new())),
            recent_output_window: Arc::new(Mutex::new(HashMap::new())),
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

    pub fn get_ai_provider(&self, pty_id: u32) -> Option<String> {
        self.ai_providers.lock().unwrap().get(&pty_id).cloned()
    }

    /// 返回近期输出窗口的原始内容（含 ANSI 转义），供 process_monitor 做 awaiting-input 检测
    pub fn get_recent_output_window(&self, pty_id: u32) -> String {
        self.recent_output_window.lock().unwrap()
            .get(&pty_id)
            .cloned()
            .unwrap_or_default()
    }

    fn append_recent_output_window(&self, pty_id: u32, data: &str) {
        let mut map = self.recent_output_window.lock().unwrap();
        let entry = map.entry(pty_id).or_default();
        entry.push_str(data);
        const ROW_CAP: usize = 8 * 1024;
        if entry.len() > ROW_CAP {
            let excess = entry.len() - ROW_CAP;
            let boundary = (excess..=entry.len())
                .find(|&i| entry.is_char_boundary(i))
                .unwrap_or(entry.len());
            entry.drain(..boundary);
        }
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
        let elapsed_opt = self.last_enter.lock().unwrap()
            .get(&pty_id)
            .map(|t| t.elapsed());
        let entered_recently = elapsed_opt.map_or(false, |e| e <= PREDICTION_ECHO_GRACE);
        if entered_recently {
            let detected = output_contains_ai_command(output);
            #[cfg(debug_assertions)]
            {
                let stripped = strip_ansi_codes(output).replace('\r', "\n");
                eprintln!("[PTY-DBG pty={pty_id}] grace-path: elapsed={:?} detected={detected}",
                    elapsed_opt);
                for (i, line) in stripped.lines().enumerate().take(10) {
                    let collapsed = apply_backspaces(line);
                    if !collapsed.trim().is_empty() {
                        let n = collapsed.char_indices().nth(120).map_or(collapsed.len(), |(i, _)| i);
                        eprintln!("[PTY-DBG pty={pty_id}]   grace[{i}]: {:?}", &collapsed[..n]);
                    }
                }
            }
            if detected {
                self.ai_sessions.lock().unwrap().insert(pty_id);
                if let Some(provider) = detect_provider_from_output(output) {
                    self.ai_providers.lock().unwrap().insert(pty_id, provider.to_string());
                }
            }
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
        let mut detected_provider: Option<&'static str> = None;
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
                            if is_ai_cmd && !has_non_interactive_flag {
                                enter_ai = true;
                                detected_provider = detect_provider_from_token(first_word);
                            }
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
                let detected = output_contains_ai_command(ose_data);
                #[cfg(debug_assertions)]
                {
                    let stripped = strip_ansi_codes(ose_data).replace('\r', "\n");
                    eprintln!("[PTY-DBG pty={pty_id}] Enter: OSE len={}, detected={detected}",
                        ose_data.len());
                    for (i, line) in stripped.lines().enumerate().take(20) {
                        let collapsed = apply_backspaces(line);
                        if !collapsed.trim().is_empty() {
                            let n = collapsed.char_indices().nth(120).map_or(collapsed.len(), |(i, _)| i);
                            eprintln!("[PTY-DBG pty={pty_id}]   line[{i}]: {:?}", &collapsed[..n]);
                        }
                    }
                }
                if detected {
                    enter_ai = true;
                    if detected_provider.is_none() {
                        detected_provider = detect_provider_from_output(ose_data);
                    }
                }
            } else {
                #[cfg(debug_assertions)]
                eprintln!("[PTY-DBG pty={pty_id}] Enter: OSE entry missing");
            }
        }
        // Enter 后重置 output_since_enter，为下一条命令重新积累
        if entered {
            self.output_since_enter.lock().unwrap().insert(pty_id, String::new());
            // AI 会话中用户回车响应（确认/输入指令），清空近期输出检测窗口，
            // 避免残留的 "Press Enter" / "y/n" 等短语继续触发 awaiting-input
            if in_ai {
                self.recent_output_window.lock().unwrap().insert(pty_id, String::new());
            }
        }

        if enter_ai || exit_ai {
            let mut sessions = self.ai_sessions.lock().unwrap();
            let mut providers = self.ai_providers.lock().unwrap();
            if enter_ai {
                sessions.insert(pty_id);
                if let Some(p) = detected_provider {
                    providers.insert(pty_id, p.to_string());
                }
            } else {
                sessions.remove(&pty_id);
                providers.remove(&pty_id);
            }
        }
    }

    /// 仅供单元测试使用：向 output_since_enter 注入模拟 PTY 输出
    #[cfg(test)]
    fn inject_pty_output(&self, pty_id: u32, data: &str) {
        let output = self.append_output_since_enter(pty_id, data);
        self.try_enter_ai_from_recent_output(pty_id, &output);
    }
}

/// 返回 `bytes` 中最后一个完整 UTF-8 字符之后的偏移量。
/// 用于 PTY 输出 flush 时避免在多字节字符边界截断。
fn find_valid_utf8_prefix_len(bytes: &[u8]) -> usize {
    let mut i = bytes.len();
    while i > 0 {
        i -= 1;
        let byte = bytes[i];
        if byte < 0x80 {
            // ASCII，本身完整
            return bytes.len();
        } else if byte >= 0xC0 {
            // 多字节序列起始字节：检查后续延续字节是否足够
            let expected = if byte >= 0xF0 { 4 } else if byte >= 0xE0 { 3 } else { 2 };
            if bytes.len() - i >= expected {
                return bytes.len(); // 序列完整
            }
            return i; // 截断到该起始字节之前
        }
        // 0x80..0xBF 是延续字节，继续向前
    }
    i
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

    // ── UTF-8 locale 注入 ──────────────────────────────────────────────────
    // GUI 应用启动时环境里可能没有完整 LANG/LC_CTYPE，导致子进程回退到 C/ASCII。
    // 策略：继承父进程已有的 UTF-8 locale；仅在缺失时补完整 locale 名称。
    // 不写死裸 "UTF-8"（非标准），不强制覆盖用户已有的 UTF-8 设置。
    {
        fn has_utf8_locale(value: &str) -> bool {
            value.to_ascii_uppercase().contains("UTF-8")
        }

        let fallback_locale = if cfg!(target_os = "macos") {
            "en_US.UTF-8"
        } else {
            "C.UTF-8"
        };

        let inherited_lang = std::env::var("LANG").ok();
        let inherited_lc_ctype = std::env::var("LC_CTYPE").ok();

        if !inherited_lang.as_deref().is_some_and(has_utf8_locale) {
            cmd.env("LANG", fallback_locale);
        }

        if !inherited_lc_ctype.as_deref().is_some_and(has_utf8_locale) {
            let lc_ctype_val = inherited_lang
                .as_deref()
                .filter(|v| has_utf8_locale(v))
                .unwrap_or(fallback_locale);
            cmd.env("LC_CTYPE", lc_ctype_val);
        }
    }

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
                        // 复用与常规 flush 相同的 UTF-8 边界截断逻辑，
                        // 避免在 PTY 关闭瞬间把尾部截断的多字节字符替换成 U+FFFD。
                        let valid_len = find_valid_utf8_prefix_len(&pending);
                        let slice = if valid_len > 0 { &pending[..valid_len] } else { &pending[..] };
                        let data = String::from_utf8_lossy(slice).into_owned();
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
                let valid_len = find_valid_utf8_prefix_len(&pending);

                if valid_len > 0 {
                    let data = String::from_utf8_lossy(&pending[..valid_len]).into_owned();

                    // 将本批输出追加到 output_since_enter，供 track_input 在 Enter 时检测
                    // PSReadLine inline prediction（右箭头接受）会在 Enter 前渲染命令文本
                    let recent_output = pty_state_for_output
                        .append_output_since_enter(pty_id_for_reader, &data);
                    pty_state_for_output
                        .try_enter_ai_from_recent_output(pty_id_for_reader, &recent_output);

                    // 更新近期输出滚动窗口，供 process_monitor 做 awaiting-input 检测
                    pty_state_for_output
                        .append_recent_output_window(pty_id_for_reader, &data);

                    let _ = app_flush.emit("pty-output", PtyOutputPayload {
                        pty_id: pty_id_for_reader, data,
                    });

                    // 冷却窗口内（刚 resize 过）的输出不刷新 last_output。
                    // Claude/Codex 等 TUI 在 ConPTY resize 后会全屏重绘，
                    // 这些重绘数据不应被 process_monitor 当作 AI 活跃信号。
                    let in_cooldown = pty_state_for_output
                        .resize_cooldown_until
                        .lock()
                        .ok()
                        .and_then(|m| m.get(&pty_id_for_reader).copied())
                        .map_or(false, |until| Instant::now() < until);
                    if !in_cooldown {
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
    {
        let instances = state.instances.lock().unwrap();
        let instance = instances.get(&pty_id).ok_or("PTY not found")?;
        instance.master
            .resize(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 })
            .map_err(|e| e.to_string())?;
    }
    // 开启冷却窗口：之后 RESIZE_COOLDOWN 内的 PTY 输出（主要是 TUI 重绘）
    // 不会刷新 last_output，从而避免被 process_monitor 误判为 AI 活跃。
    state.resize_cooldown_until
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
    state.ai_providers.lock().unwrap().remove(&pty_id);
    state.input_buffers.lock().unwrap().remove(&pty_id);
    state.last_ctrlc.lock().unwrap().remove(&pty_id);
    state.last_enter.lock().unwrap().remove(&pty_id);
    state.output_since_enter.lock().unwrap().remove(&pty_id);
    state.resize_cooldown_until.lock().unwrap().remove(&pty_id);
    state.recent_output_window.lock().unwrap().remove(&pty_id);

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
    fn detect_gemini_command() {
        let mgr = PtyManager::new();
        mgr.track_input(1, "gemini\r");
        assert!(mgr.is_ai_session(1));
    }

    #[test]
    fn echo_powershell_prompt_gemini_detected() {
        assert!(output_contains_ai_command("PS C:\\workspace> gemini"));
    }

    #[test]
    fn gemini_version_flag_not_detected() {
        let mgr = PtyManager::new();
        mgr.track_input(1, "gemini --version\r");
        assert!(!mgr.is_ai_session(1));
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
    fn psreadline_history_nav_cha_overwrite_detected() {
        // 模拟：用户先输入 "ls ~/.codex"，再按上箭头切换为 "claude --model sonnet"。
        // PSReadLine 用 CSI G（\x1b[NG]，光标横向绝对定位）把光标移回命令起始列，
        // 再 erase-to-end（\x1b[K），再写入新命令。
        // strip_ansi_codes 将 CSI G 转为 \r，output_contains_ai_command 把行在此断开，
        // 使 "claude" 出现在独立行上被正确识别。
        let mgr = PtyManager::new();
        // \x1b[32G = 移到第 32 列（prompt 结束后），\x1b[K = erase-to-end
        mgr.inject_pty_output(1,
            "PS H:\\workspace\\self\\mini-term> ls ~/.codex\x1b[32G\x1b[Kclaude --model sonnet");
        mgr.track_input(1, "\r");
        assert!(mgr.is_ai_session(1),
            "history navigation via CHA should be detected");
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

    // ── 扩展提示符：zsh % / root # / oh-my-zsh ➜ / Pure ❯ / fish › / λ ──

    #[test]
    fn echo_zsh_percent_prompt_claude_detected() {
        assert!(output_contains_ai_command("user@host:~% claude"));
    }

    #[test]
    fn echo_zsh_percent_prompt_claude_with_args_detected() {
        assert!(output_contains_ai_command("user@host:~/projects% claude --model sonnet"));
    }

    #[test]
    fn echo_zsh_percent_prompt_codex_detected() {
        assert!(output_contains_ai_command("yhb@macbook:~% codex"));
    }

    #[test]
    fn echo_bash_root_hash_prompt_claude_detected() {
        assert!(output_contains_ai_command("root@server:/# claude --model sonnet"));
    }

    #[test]
    fn echo_bash_root_hash_prompt_codex_detected() {
        assert!(output_contains_ai_command("root@ubuntu:~# codex"));
    }

    #[test]
    fn echo_ohmyzsh_arrow_prompt_claude_detected() {
        // oh-my-zsh arrow theme: ➜  <dir> command
        assert!(output_contains_ai_command("➜  mini-term claude"));
    }

    #[test]
    fn echo_pure_prompt_claude_detected() {
        // Pure prompt: ❯ command
        assert!(output_contains_ai_command("❯ claude --model opus"));
    }

    #[test]
    fn echo_pure_prompt_codex_detected() {
        assert!(output_contains_ai_command("❯ codex"));
    }

    #[test]
    fn echo_narrow_angle_prompt_claude_detected() {
        // › prompt (fish / some themes)
        assert!(output_contains_ai_command("› claude"));
    }

    #[test]
    fn echo_lambda_prompt_claude_detected() {
        // λ prompt
        assert!(output_contains_ai_command("λ claude --model sonnet"));
    }

    // ── 扩展提示符：非交互标志仍需过滤 ──────────────────────────────────

    #[test]
    fn echo_zsh_percent_claude_version_not_detected() {
        assert!(!output_contains_ai_command("user@host:~% claude --version"));
    }

    #[test]
    fn echo_zsh_percent_claude_help_not_detected() {
        assert!(!output_contains_ai_command("user@host:~% claude -h"));
    }

    #[test]
    fn echo_pure_prompt_claude_version_not_detected() {
        assert!(!output_contains_ai_command("❯ claude -v"));
    }

    #[test]
    fn echo_root_hash_codex_help_not_detected() {
        assert!(!output_contains_ai_command("root@host:~# codex --help"));
    }

    // ── Token fallback（无提示符）：保守性约束验证 ────────────────────────

    #[test]
    fn token_fallback_plain_claude_detected() {
        // 无提示符，纯 "claude" 行（terminal 回显）
        assert!(output_contains_ai_command("claude"));
        assert!(output_contains_ai_command("codex"));
        assert!(output_contains_ai_command("gemini --model flash"));
    }

    #[test]
    fn npm_install_claude_sdk_not_false_positive() {
        // claude 在中间 token，不应触发
        assert!(!output_contains_ai_command("npm install @anthropic-ai/claude-sdk"));
    }

    #[test]
    fn echo_claude_sdk_package_name_not_false_positive() {
        // claude 出现在包名末尾但不是首 token
        assert!(!output_contains_ai_command("pip install anthropic-claude"));
    }

    #[test]
    fn grep_result_line_not_false_positive() {
        // grep 结果中包含 "claude" 关键字，不应触发
        assert!(!output_contains_ai_command("README.md:5: uses claude for AI features"));
    }

    // ── PSReadLine / zsh autosuggestion 接受路径 ──────────────────────────

    #[test]
    fn psreadline_zsh_autosuggestion_accept_detected() {
        // 模拟 zsh autosuggestion：右箭头接受后 PTY 渲染提示行，input_buf 为空
        let mgr = PtyManager::new();
        mgr.inject_pty_output(1, "user@host:~% claude --model sonnet");
        mgr.track_input(1, "\r");
        assert!(mgr.is_ai_session(1));
    }

    #[test]
    fn pure_prompt_autosuggestion_accept_detected() {
        let mgr = PtyManager::new();
        mgr.inject_pty_output(1, "❯ claude");
        mgr.track_input(1, "\r");
        assert!(mgr.is_ai_session(1));
    }

    #[test]
    fn zsh_percent_autosuggestion_version_flag_not_detected() {
        // autosuggestion 接受了带 --version 的命令，不应触发 AI 会话
        let mgr = PtyManager::new();
        mgr.inject_pty_output(1, "user@host:~% claude --version");
        mgr.track_input(1, "\r");
        assert!(!mgr.is_ai_session(1));
    }

    // ── zsh-syntax-highlighting BS 重绘 + autosuggestion 真实字节流 ──────────
    //
    // 真实捕获自 macOS zsh + zsh-syntax-highlighting + zsh-autosuggestions：
    // 用户输入 `c`，syntax-highlighting 用 `\b\x1b[1m\x1b[31mc\x1b[0m\x1b[39m`
    // 把字符重写成红色，autosuggestion 在后面追加 `\x1b[90mlaude --model sonnet\x1b[39m`
    // 然后 `\x1b[<n>D` 把光标拉回。用户按右箭头接受后再次重绘命令文本。
    // strip_ansi 去掉转义后会留下 BS 字节，导致 split_whitespace 切出 `c\bc\bclaude` 乱码。
    #[test]
    fn zsh_syntax_highlight_backspace_rerender_detected() {
        let raw = "yuhongbin@yuhongbindeMacBook-Pro jingju % c\x08\x1b[1m\x1b[31mc\x1b[0m\x1b[39m\x1b[90mlaude --model sonnet\x1b[39m\x1b[20D\x1b[0m\x1b[32mc\x1b[32ml\x1b[32ma\x1b[32mu\x1b[32md\x1b[32me\x1b[39m";
        assert!(output_contains_ai_command(raw));
    }

    #[test]
    fn apply_backspaces_collapses_rerender() {
        // c<BS>c<BS>claude → claude
        assert_eq!(apply_backspaces("c\x08c\x08claude"), "claude");
        // 空字符串与无 BS 字符串保持不变
        assert_eq!(apply_backspaces(""), "");
        assert_eq!(apply_backspaces("claude"), "claude");
        // BS 不能弹出已删完的栈
        assert_eq!(apply_backspaces("\x08\x08abc"), "abc");
    }
}
