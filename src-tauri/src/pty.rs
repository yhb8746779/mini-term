use portable_pty::{native_pty_system, CommandBuilder, PtySize, MasterPty, Child};
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::io::{Read, Write};
use std::sync::{Arc, Mutex};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter};

/// Layer 2（进程级兜底）：从近期 PTY 输出中识别 AI CLI 启动 banner，
/// 用于在 Layer 1（命令 echo 解析）漏判时恢复会话状态。
///
/// 从最近行向前扫描（逐行匹配，最新 banner 优先）：
/// 当用户从 Codex 切换到 Claude，两个 banner 都在窗口内时，
/// 最近的一行（Claude）优先返回，不会被旧的 Codex banner 干扰。
///
/// 扫描全部行（无 take(N) 上限）：recent_output_window 在每次会话开始时清空，
/// 不存在跨会话 banner 污染。Claude Code TUI 启动输出密集，有限 take 会
/// 将 banner 推出扫描范围，导致 Layer 2 纠正失效（blue dot 永不恢复橙色）。
///
/// Codex 使用精确短语 ">_ openai codex"（含 ASCII art 前缀），
/// 避免用户在 Claude 会话内讨论 Codex 时被误判。
fn detect_provider_from_banner(raw_output: &str) -> Option<&'static str> {
    let stripped = strip_ansi_codes(raw_output).replace('\r', "\n");
    let lines: Vec<&str> = stripped.lines().filter(|l| !l.trim().is_empty()).collect();
    if lines.is_empty() {
        return None;
    }
    // 从最近行向前扫描，返回第一个匹配（= 最新的 banner）。
    // 注意：不使用 take(N) 限制。recent_output_window 在每次会话开始时被清空，
    // 不存在跨会话的旧 banner 污染问题。Claude Code 的 TUI 启动输出非常密集，
    // take(50) 会导致 banner 行被推出扫描范围，使 Layer 2 纠正失效。
    for line in lines.iter().rev() {
        let lower = line.to_lowercase();
        // Claude Code 启动 banner（多版本兼容）：
        //   旧版："✻ Welcome to Claude Code!" / "Welcome to Claude Code"
        //   新版（v2.x+）："Claude Code v2.1.110" 等版本号行
        //
        // 注意：Claude Code TUI 使用光标定位序列（如 \x1b[1;8H）渲染单词间距，
        // strip_ansi_codes 只删除 CSI G，其他定位序列（CSI H 等）被丢弃而不插入空格，
        // 导致 "Claude Code v2.x" 变为 "ClaudeCode v2.x"（单词间空格丢失）。
        // 因此需同时检查"有空格"和"无空格"两种形式。
        if lower.contains("welcome to claude code") || lower.contains("claude code v") {
            return Some("claude");
        }
        // 无空格形式（光标定位导致空格丢失）："ClaudeCode v2.x" → "claudecodev"
        // 同时检查 OSC terminal-title 形式 "0;Claude Code v..." / "0;claude code"
        {
            let no_space: String = lower.split_whitespace().collect();
            if no_space.contains("claudecodev") || no_space.contains("welcometoclaudecode") {
                return Some("claude");
            }
        }
        // Codex CLI (OpenAI) 启动 banner：">_ OpenAI Codex (v...)"
        // 使用 ">_ openai codex" 精确匹配，避免 Claude 讨论 Codex 时误判
        if lower.contains(">_ openai codex") {
            return Some("codex");
        }
        // Gemini CLI (Google) 启动 banner
        if lower.contains("welcome to gemini") {
            return Some("gemini");
        }
    }
    None
}

/// 从命令 token（已小写）中提取 AI provider 名称
fn detect_provider_from_token(token: &str) -> Option<&'static str> {
    for &ai in AI_COMMANDS {
        if token == ai || token.ends_with(&format!("/{ai}")) || token.ends_with(&format!("\\{ai}")) {
            return Some(ai);
        }
    }
    None
}

/// 从单行（已完成 ANSI 剥离和退格处理）中检测 AI 命令并返回 provider。
///
/// 与 line_contains_ai_command 使用完全相同的提示符解析逻辑（Part A）：
/// - 终端提示符（> $ % #）：只检查其后首 token
/// - Unicode 主题提示符（❯ ➜ › λ）：扫描其后所有 token（同 line_contains_ai_command）
/// - 无提示符 fallback：只检查行首首 token
///
/// 消除了旧版 extract_provider_from_command_line 对 Unicode 提示符只检查首 token
/// 导致的不对称问题——provider 检测能力现与会话检测能力完全对称。
fn detect_ai_provider_from_line(line: &str) -> Option<&'static str> {
    let line = line.trim();
    if line.is_empty() { return None; }

    const TERMINAL_PROMPT_CHARS: &[char] = &['>', '$', '%', '#'];
    const UNICODE_PROMPT_CHARS: &[char] = &['❯', '➜', '›', 'λ'];

    // 终端提示符：出现在行尾附近，命令紧跟其后，只检查首 token
    if let Some(pos) = line.rfind(TERMINAL_PROMPT_CHARS) {
        let ch = line[pos..].chars().next().unwrap();
        let cmd_part = line[pos + ch.len_utf8()..].trim();
        let mut words = cmd_part.split_whitespace();
        let first = words.next().unwrap_or("");
        if let Some(provider) = detect_provider_from_token(&first.to_lowercase()) {
            if !has_non_interactive_flag(words) {
                return Some(provider);
            }
        }
        return None;
    }

    // Unicode 主题提示符（❯ ➜ › λ）位于行首，命令在目录信息之后；
    // 扫描其后所有 token，找到第一个 AI 命令 token（与 line_contains_ai_command 一致）
    if let Some(pos) = line.rfind(UNICODE_PROMPT_CHARS) {
        let ch = line[pos..].chars().next().unwrap();
        let cmd_part = line[pos + ch.len_utf8()..].trim();
        let tokens: Vec<&str> = cmd_part.split_whitespace().collect();
        for (i, &tok) in tokens.iter().enumerate() {
            if let Some(provider) = detect_provider_from_token(&tok.to_lowercase()) {
                if !has_non_interactive_flag(tokens[i + 1..].iter().copied()) {
                    return Some(provider);
                }
                return None;
            }
        }
        return None;
    }

    // 无提示符 fallback：只检查行首第一个 token，防止中间词被误判
    let mut words = line.split_whitespace();
    let first = words.next().unwrap_or("");
    if let Some(provider) = detect_provider_from_token(&first.to_lowercase()) {
        if !has_non_interactive_flag(words) {
            return Some(provider);
        }
    }
    None
}

/// 从原始输出文本中提取最后一次调用的 AI provider 名称。
/// 使用与 output_contains_ai_command 完全相同的解析逻辑（通过 detect_ai_provider_from_line），
/// 确保 provider 检测能力与会话检测能力对称。
fn detect_provider_from_output(output: &str) -> Option<&'static str> {
    let stripped = strip_ansi_codes(output).replace('\r', "\n");
    let mut last_found: Option<&'static str> = None;
    for line in stripped.lines() {
        let collapsed = apply_backspaces(line);
        if let Some(provider) = detect_ai_provider_from_line(&collapsed) {
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
                Some(&']') => {
                    // OSC (Operating System Command): ESC ] ... ST
                    // 终结符 = BEL (\x07) 或 ST (ESC \)
                    // macOS 上 Claude Code / Codex 设置终端标题会输出 OSC 序列，
                    // 不消费的话 "0;Claude Code v2.x" 等会泄漏到输出缓冲区，
                    // 干扰 banner 检测和 provider 识别。
                    chars.next(); // consume ']'
                    loop {
                        match chars.next() {
                            None => break,
                            Some('\x07') => break, // BEL 终结
                            Some('\x1b') => {
                                // ST = ESC '\'
                                if chars.peek() == Some(&'\\') {
                                    chars.next();
                                }
                                break;
                            }
                            Some(_) => {} // 消费 OSC 内容
                        }
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

/// 判断参数迭代器中是否包含非交互式标志（-v/--version/-h/--help/-p/--print）
fn has_non_interactive_flag<'a>(args: impl Iterator<Item = &'a str>) -> bool {
    args.into_iter().any(|w| NON_INTERACTIVE_FLAGS.iter().any(|&f| w == f))
}

/// 检查单行文本（已完成 ANSI 剥离和退格处理）是否含有 AI 命令的 echo。
/// 委托给 detect_ai_provider_from_line，与 provider 提取逻辑完全一致（Part A）。
#[allow(dead_code)]
fn line_contains_ai_command(line: &str) -> bool {
    detect_ai_provider_from_line(line).is_some()
}

/// 检查 PTY 输出中是否包含 AI 命令被 echo（支持 PS/bash/zsh/fish/主题提示符）
/// 同时过滤非交互式标志（-v/--version/-h/--help 等），避免误识别。
/// 等价于 detect_provider_from_output(output).is_some()，确保两者检测能力对称。
#[allow(dead_code)]
fn output_contains_ai_command(output: &str) -> bool {
    detect_provider_from_output(output).is_some()
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
            // Part A+B: 直接用统一函数检测 provider；仅在同时能确定 provider 时才
            // 进入 AI 状态，且在同一锁范围内原子写入 session + provider，
            // 消除 monitor 在两次独立写入之间轮询导致的 provider=None 竞态。
            if let Some(provider) = detect_provider_from_output(output) {
                // 先清空 recent_output_window，再设置 session+provider。
                // 防止 monitor 在两步之间抢跑 try_reconcile_ai_from_banner，
                // 用旧 banner（如上次 Claude 的）覆盖本次检测到的 provider（如 Codex）。
                self.recent_output_window.lock().unwrap().insert(pty_id, String::new());
                let mut sessions = self.ai_sessions.lock().unwrap();
                let mut providers = self.ai_providers.lock().unwrap();
                sessions.insert(pty_id);
                providers.insert(pty_id, provider.to_string());
            }
        }
    }

    /// Part C：当 AI 会话已建立但 provider 仍未知时，从最新 PTY 输出中补填 provider。
    /// 确保用户不需要退出再进入才能恢复 provider 颜色。
    fn try_backfill_provider(&self, pty_id: u32, output: &str) {
        if !self.is_ai_session(pty_id) {
            return;
        }
        if self.get_ai_provider(pty_id).is_some() {
            return; // 已有 provider，无需补填
        }
        if let Some(provider) = detect_provider_from_output(output) {
            self.ai_providers.lock().unwrap().insert(pty_id, provider.to_string());
        }
    }

    /// Part F：原子性读取 AI 会话状态和 provider，消除两次独立锁调用之间的竞态。
    /// 同时持有两个锁，确保 monitor 读到的 (is_ai, provider) 是一致快照。
    /// 锁获取顺序与 track_input 中一致（ai_sessions → ai_providers），不会死锁。
    pub fn get_ai_session_info(&self, pty_id: u32) -> (bool, Option<String>) {
        let sessions = self.ai_sessions.lock().unwrap();
        let providers = self.ai_providers.lock().unwrap();
        let is_ai = sessions.contains(&pty_id);
        let provider = if is_ai { providers.get(&pty_id).cloned() } else { None };
        (is_ai, provider)
    }

    /// Layer 2（进程级兜底）：扫描 recent_output_window 中的 AI CLI 启动 banner。
    /// 由 process_monitor 每 500ms 调用一次。
    ///
    /// 三重情形：
    /// 1. PTY 尚未进入 AI 会话，但近期输出含 banner → 建立会话 + 设置 provider
    /// 2. PTY 已在 AI 会话中，provider 已知但与 banner 不符 → banner 纠正误判（高置信度）
    /// 3. PTY 已在 AI 会话中，但 provider 仍为 None → 先尝试 banner，再回退到
    ///    detect_provider_from_output 扫描 recent_output_window 中的 shell echo 行
    ///    （PSReadLine 路径设置 enter_ai=true 但 provider 仍 None 的恢复路径）
    ///
    /// 安全约束：
    /// - banner 纠正（情形 2）仅依赖高精度 banner 短语，避免 AI 会话内容误触发 provider 切换
    /// - provider 补填（情形 3）同时使用 banner 和 output 检测，覆盖更多场景
    pub fn try_reconcile_ai_from_banner(&self, pty_id: u32) {
        let window = self.get_recent_output_window(pty_id);
        if window.is_empty() {
            return;
        }

        // banner 检测（高精度，用于会话建立和 provider 纠正）
        let banner_detected = detect_provider_from_banner(&window);

        // 原子读取当前状态（锁顺序：ai_sessions → ai_providers）
        let (is_ai, current_provider) = {
            let sessions = self.ai_sessions.lock().unwrap();
            let providers = self.ai_providers.lock().unwrap();
            let is_ai = sessions.contains(&pty_id);
            let p = if is_ai { providers.get(&pty_id).cloned() } else { None };
            (is_ai, p)
        };

        if !is_ai {
            // 情形 1：尚未进入 AI 会话 → banner 触发会话建立（仅限高精度 banner）
            if let Some(detected) = banner_detected {
                let mut sessions = self.ai_sessions.lock().unwrap();
                let mut providers = self.ai_providers.lock().unwrap();
                sessions.insert(pty_id);
                providers.insert(pty_id, detected.to_string());
            }
        } else if let Some(detected) = banner_detected {
            // 情形 2：已在 AI 会话中，banner 纠正错误 provider（高精度）
            if current_provider.as_deref() != Some(detected) {
                self.ai_providers.lock().unwrap().insert(pty_id, detected.to_string());
            }
        } else if current_provider.is_none() {
            // 情形 3：AI 会话已建立但 provider 仍缺失（banner 未命中）
            // 回退到 detect_provider_from_output 扫描近期输出中的 shell echo 行。
            // 仅在 provider=None 时回填，避免用 output 检测覆盖已知 provider（防误判）。
            if let Some(detected) = detect_provider_from_output(&window) {
                self.ai_providers.lock().unwrap().insert(pty_id, detected.to_string());
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
        // Enter 时 buf 的内容（需在锁外使用，所以提升到外层作用域）
        let mut last_cmd = String::new();
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
                        last_cmd = cmd.clone(); // 保存到外层，供 output_since_enter 补偿路径使用
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

        // PSReadLine inline prediction 补偿：当用户按右箭头接受预测文本或上箭头召回历史后
        // 再 Enter 时，input_buffers 中的 buf 因 ESC 序列清空而为空，直接输入检测无法命中。
        // 此时扫描 output_since_enter（PTY 在 Enter 前渲染到屏幕的内容）来判断是否是 AI 命令。
        //
        // 关键约束：
        // 1. 仅在 last_cmd.is_empty() 时才走此路径（即 buf 在 Enter 时确实为空）。
        // 2. detect_provider_from_output 返回 last_found（最后一次匹配），即 output_since_enter
        //    中最近一次渲染的命令。PSReadLine 在每次上下箭头切换时均会用 CSI G 覆写当前行，
        //    strip_ansi_codes 将 CSI G 转为 \r，.replace('\r', '\n') 把它断成独立行，
        //    使"最后渲染的命令"（用户实际执行的）成为 last_found，而非早期的 ghost text。
        //    因此直接用 detect_provider_from_output 的返回值即可得到正确的 provider。
        if entered && !in_ai && !enter_ai && last_cmd.is_empty() {
            let ose = self.output_since_enter.lock().unwrap();
            if let Some(ose_data) = ose.get(&pty_id) {
                if let Some(provider) = detect_provider_from_output(ose_data) {
                    enter_ai = true;
                    detected_provider = Some(provider);
                }
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
            // 先清空 recent_output_window，再更新 session+provider。
            // 消除竞态：若先设置 provider 再清窗口，monitor 可能在两步之间抢跑
            // try_reconcile_ai_from_banner，用旧 banner 覆盖刚设置的新 provider。
            // 例：上次运行 Claude → window 含 Claude banner → 本次启动 Codex →
            // Layer 1 正确设置 provider="codex" → monitor 用旧 Claude banner
            // 把 provider 纠正回 "claude" → 蓝点不变 ← BUG。
            // 先清窗口后，monitor 看到空窗口 → reconcile 为 no-op → provider 正确。
            if enter_ai {
                self.recent_output_window.lock().unwrap().insert(pty_id, String::new());
            }

            {
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
            } // sessions + providers 在此释放，避免下方加锁时死锁

            if exit_ai {
                // 退出 AI 会话时清空输出窗口，防止残留的 AI 启动 banner
                // 在下一次 banner 兜底检测中误判为新会话开始。
                //
                // 对于 Enter 触发的退出（/exit /quit exit quit 等），
                // 下方的 `if entered` 块也会清空这些窗口。
                // 对于 Ctrl+D 和双 Ctrl+C 退出，`entered = false`，
                // 必须在这里显式清空。
                self.recent_output_window.lock().unwrap().insert(pty_id, String::new());
                if !entered {
                    self.output_since_enter.lock().unwrap().insert(pty_id, String::new());
                }
            }
        }
    }

    /// 仅供单元测试使用：向 output_since_enter 注入模拟 PTY 输出
    #[cfg(test)]
    fn inject_pty_output(&self, pty_id: u32, data: &str) {
        let output = self.append_output_since_enter(pty_id, data);
        self.try_enter_ai_from_recent_output(pty_id, &output);
    }

    /// 仅供单元测试使用：向 recent_output_window 注入模拟 PTY 输出（模拟 banner 到来）
    #[cfg(test)]
    fn inject_banner_output(&self, pty_id: u32, data: &str) {
        self.append_recent_output_window(pty_id, data);
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
    crate::path_access::ensure_path_access(&app, &cwd)?;
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
        let mut diag_reads: u64 = 0;
        let mut diag_bytes: u64 = 0;
        let mut diag_flushes: u64 = 0;
        let mut diag_lossy_flushes: u64 = 0;
        let mut diag_max_pending: usize = 0;
        let mut diag_last = Instant::now();

        loop {
            match rx.recv_timeout(Duration::from_millis(16)) {
                Ok(data) => {
                    diag_reads += 1;
                    diag_bytes += data.len() as u64;
                    pending.extend(data);
                    diag_max_pending = diag_max_pending.max(pending.len());
                    while let Ok(more) = rx.try_recv() {
                        diag_reads += 1;
                        diag_bytes += more.len() as u64;
                        pending.extend(more);
                        diag_max_pending = diag_max_pending.max(pending.len());
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
                    diag_flushes += 1;
                    if valid_len < pending.len() {
                        diag_lossy_flushes += 1;
                    }
                    let data = String::from_utf8_lossy(&pending[..valid_len]).into_owned();

                    // 将本批输出追加到 output_since_enter，供 track_input 在 Enter 时检测
                    // PSReadLine inline prediction（右箭头接受）会在 Enter 前渲染命令文本
                    let recent_output = pty_state_for_output
                        .append_output_since_enter(pty_id_for_reader, &data);
                    pty_state_for_output
                        .try_enter_ai_from_recent_output(pty_id_for_reader, &recent_output);
                    // Part C: 若 session 已建立但 provider 仍缺失，尝试从新输出中补全
                    pty_state_for_output
                        .try_backfill_provider(pty_id_for_reader, &data);

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

                if diag_last.elapsed() >= Duration::from_secs(5) || diag_lossy_flushes > 0 {
                    crate::perf_log::log_perf(
                        &app_flush,
                        "pty_output_diag",
                        &format!(
                            "pty_id={} | reads={} | bytes={} | flushes={} | utf8_leftover_flushes={} | max_pending={}",
                            pty_id_for_reader,
                            diag_reads,
                            diag_bytes,
                            diag_flushes,
                            diag_lossy_flushes,
                            diag_max_pending
                        ),
                    );
                    diag_reads = 0;
                    diag_bytes = 0;
                    diag_flushes = 0;
                    diag_lossy_flushes = 0;
                    diag_max_pending = 0;
                    diag_last = Instant::now();
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

    // ── Layer 2：banner 兜底检测 ───────────────────────────────────────────

    #[test]
    fn banner_reconcile_detects_claude_session() {
        // 模拟：用户上箭头历史召回 + Enter，Layer 1 命令 echo 漏判，
        // 但 Claude Code 启动 banner 出现在 recent_output_window 中。
        let mgr = PtyManager::new();
        mgr.inject_banner_output(
            1,
            "╭──────────────────────────────╮\n│ \u{273b} Welcome to Claude Code! │\n╰──────────────────────────────╯\n",
        );
        // 模拟 process_monitor 调用 reconcile
        mgr.try_reconcile_ai_from_banner(1);
        assert!(mgr.is_ai_session(1), "banner 检测应恢复 AI 会话");
        assert_eq!(mgr.get_ai_provider(1).as_deref(), Some("claude"));
    }

    #[test]
    fn banner_reconcile_detects_codex_session() {
        let mgr = PtyManager::new();
        mgr.inject_banner_output(1, ">_ OpenAI Codex (v0.122.0)\n");
        mgr.try_reconcile_ai_from_banner(1);
        assert!(mgr.is_ai_session(1));
        assert_eq!(mgr.get_ai_provider(1).as_deref(), Some("codex"));
    }

    #[test]
    fn banner_reconcile_detects_gemini_session() {
        let mgr = PtyManager::new();
        mgr.inject_banner_output(1, "Welcome to Gemini CLI\n");
        mgr.try_reconcile_ai_from_banner(1);
        assert!(mgr.is_ai_session(1));
        assert_eq!(mgr.get_ai_provider(1).as_deref(), Some("gemini"));
    }

    #[test]
    fn banner_reconcile_no_false_positive_after_exit_command() {
        // 用户通过 /exit 退出后，残留 banner 不应误触发重进 AI 会话
        let mgr = PtyManager::new();
        mgr.track_input(1, "claude\r");
        assert!(mgr.is_ai_session(1));
        // 注入 banner（模拟 claude 启动输出）
        mgr.inject_banner_output(1, "Welcome to Claude Code!\n");
        // 通过 /exit 退出（Enter 触发，会清空 recent_output_window）
        mgr.track_input(1, "/exit\r");
        assert!(!mgr.is_ai_session(1));
        // banner 已被清空，reconcile 不应误判
        mgr.try_reconcile_ai_from_banner(1);
        assert!(!mgr.is_ai_session(1), "退出后残留 banner 不应重新触发 AI 会话");
    }

    #[test]
    fn banner_reconcile_no_false_positive_after_ctrl_d() {
        // Ctrl+D 退出也应清空窗口
        let mgr = PtyManager::new();
        mgr.track_input(1, "claude\r");
        mgr.inject_banner_output(1, "Welcome to Claude Code!\n");
        mgr.track_input(1, "\x04"); // Ctrl+D
        assert!(!mgr.is_ai_session(1));
        mgr.try_reconcile_ai_from_banner(1);
        assert!(!mgr.is_ai_session(1), "Ctrl+D 退出后 banner 不应重新触发");
    }

    #[test]
    fn banner_reconcile_no_false_positive_after_double_ctrl_c() {
        // 双 Ctrl+C 退出也应清空窗口
        let mgr = PtyManager::new();
        mgr.track_input(1, "claude\r");
        mgr.inject_banner_output(1, "Welcome to Claude Code!\n");
        mgr.track_input(1, "\x03");
        mgr.track_input(1, "\x03"); // 双 Ctrl+C
        assert!(!mgr.is_ai_session(1));
        mgr.try_reconcile_ai_from_banner(1);
        assert!(!mgr.is_ai_session(1), "双 Ctrl+C 退出后 banner 不应重新触发");
    }

    #[test]
    fn banner_reconcile_no_op_when_provider_already_correct() {
        // 已在 AI 会话中且 provider 正确时，reconcile 应为 no-op
        let mgr = PtyManager::new();
        mgr.track_input(1, "codex\r");
        assert_eq!(mgr.get_ai_provider(1).as_deref(), Some("codex"));
        // 注入真实的 Codex 启动 banner（AI 启动后实际会输出的内容）
        mgr.inject_banner_output(1, ">_ OpenAI Codex (v0.122.0)\n");
        mgr.try_reconcile_ai_from_banner(1);
        // provider 应保持不变
        assert_eq!(mgr.get_ai_provider(1).as_deref(), Some("codex"),
            "provider 已正确时 reconcile 应为 no-op");
    }

    #[test]
    fn banner_reconcile_corrects_wrong_provider_via_startup_banner() {
        // Layer 1 因 PSReadLine ghost text 误判 provider 为 "claude"，
        // Layer 2 应在看到真实 Codex banner 后纠正为 "codex"。
        let mgr = PtyManager::new();
        // 模拟：Layer 1 错误地将 provider 设为 "claude"（ghost text 误判）
        {
            let mut sessions = mgr.ai_sessions.lock().unwrap();
            let mut providers = mgr.ai_providers.lock().unwrap();
            sessions.insert(1);
            providers.insert(1, "claude".to_string());
        }
        assert_eq!(mgr.get_ai_provider(1).as_deref(), Some("claude"));
        // 模拟：Codex 启动后将真实 banner 输出到 recent_output_window
        mgr.inject_banner_output(1, ">_ OpenAI Codex (v0.122.0)\n");
        // Layer 2 应纠正 provider
        mgr.try_reconcile_ai_from_banner(1);
        assert_eq!(mgr.get_ai_provider(1).as_deref(), Some("codex"),
            "Layer 2 应通过 banner 纠正 Layer 1 的错误 provider");
    }

    // ── 同 pane 切换 provider：竞态修复验证 ───────────────────────────────

    #[test]
    fn same_pane_claude_then_codex_no_stale_banner() {
        // 模拟 Mac 上同一 pane 的 provider 切换场景：
        // 1. 开 Claude → banner 写入 window
        // 2. /exit 退出 → session 清除，window 清空
        // 3. 开 Codex → provider 应为 "codex"
        // 4. reconcile 不应用旧 Claude banner 覆盖
        let mgr = PtyManager::new();

        // Step 1: 开 Claude
        mgr.track_input(1, "claude\r");
        assert_eq!(mgr.get_ai_provider(1).as_deref(), Some("claude"));
        mgr.inject_banner_output(1, "Welcome to Claude Code!\n");

        // Step 2: 退出
        mgr.track_input(1, "/exit\r");
        assert!(!mgr.is_ai_session(1));

        // Step 3: 开 Codex（enter_ai 路径已清空 window，再设 provider）
        mgr.track_input(1, "codex\r");
        assert!(mgr.is_ai_session(1));
        assert_eq!(mgr.get_ai_provider(1).as_deref(), Some("codex"),
            "直接输入 codex 应正确设置 provider");

        // Step 4: reconcile 应为 no-op（window 已清空，无旧 banner）
        mgr.try_reconcile_ai_from_banner(1);
        assert_eq!(mgr.get_ai_provider(1).as_deref(), Some("codex"),
            "reconcile 不应用旧 Claude banner 覆盖 Codex provider");
    }

    #[test]
    fn same_pane_enter_ai_clears_window_before_setting_provider() {
        // 验证 enter_ai 路径的时序：window 先清空，provider 后设置。
        // 即使 window 中有旧 Claude banner，codex 入场后 reconcile 也不会覆盖。
        let mgr = PtyManager::new();

        // 先注入旧 Claude banner（模拟上次会话残留，极端情况）
        mgr.inject_banner_output(1, "Welcome to Claude Code!\n");

        // 直接输入 codex → enter_ai 应先清 window，再设 provider
        mgr.track_input(1, "codex\r");
        assert_eq!(mgr.get_ai_provider(1).as_deref(), Some("codex"));

        // reconcile 此时 window 应为空（被 enter_ai 清过）
        mgr.try_reconcile_ai_from_banner(1);
        assert_eq!(mgr.get_ai_provider(1).as_deref(), Some("codex"),
            "enter_ai 应在设置 provider 前清空 window，防止旧 banner 竞态覆盖");
    }

    // ── OSC 序列剥离 ──────────────────────────────────────────────────────

    #[test]
    fn strip_ansi_codes_removes_osc_with_bel() {
        // macOS 终端标题：ESC ] 0;title BEL
        let input = "before\x1b]0;Claude Code v2.1.110\x07after";
        let result = strip_ansi_codes(input);
        assert_eq!(result, "beforeafter", "OSC (BEL 终结) 应被完整剥离");
    }

    #[test]
    fn strip_ansi_codes_removes_osc_with_st() {
        // OSC 以 ST (ESC \) 终结
        let input = "before\x1b]0;codex title\x1b\\after";
        let result = strip_ansi_codes(input);
        assert_eq!(result, "beforeafter", "OSC (ST 终结) 应被完整剥离");
    }

    #[test]
    fn osc_title_does_not_leak_into_banner_detection() {
        // 旧版 strip_ansi_codes 不处理 OSC，会导致 "0;Claude Code v2.x" 泄漏
        // 到 detect_provider_from_banner，可能产生误判
        let raw = "\x1b]0;codex session\x07real output here\n";
        let provider = detect_provider_from_banner(raw);
        assert_ne!(provider, Some("codex"),
            "OSC 标题中的 codex 不应被 banner 检测匹配");
    }
}
