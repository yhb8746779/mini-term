use serde::Serialize;
use std::collections::HashMap;
use std::thread;
use std::time::Duration;
use tauri::{AppHandle, Emitter};

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PtyStatusChangePayload {
    pub pty_id: u32,
    pub status: String,
    pub provider: Option<String>,
}

/// AI 流式输出的活跃窗口：距上次输出不超过此时间视为"正在输出"
const AI_GENERATING_WINDOW: Duration = Duration::from_secs(2);
/// AI 静止超过此时间才视为"已完成"（避免将思考/工具调用/网络等待误判为完成）
const AI_COMPLETE_TIMEOUT: Duration = Duration::from_secs(30);

/// 强交互短语：出现即触发 ai-awaiting-input
///
/// 设计原则：
/// 1. 必须是"出现在当前行末尾/疑问句"的交互提示，不能是普通说明文字中会出现的词
/// 2. 单个短词（allow/approve/confirm）太宽泛，用更具体的短语替代
/// 3. 已有更长、更精确的短语覆盖的词不重复加（如 "do you want to allow" 已覆盖 "allow?"）
const AWAITING_STRONG: &[&str] = &[
    // 明确询问用户的短语
    "do you want to allow",
    "do you want to",
    "are you sure",
    "requires approval",
    "requesting approval",
    "grant access",
    // 带问号的确认短语（避免 "confirmed" / "configuration" 误判）
    "continue?",
    "confirm?",
    "authorize?",
    // 按键/选项提示（出现即代表等待用户操作）
    "press enter",
    "press any key",
    "hit enter",
    "choose an option",
    "select an option",
    "pick one",
    // "use arrow keys" 已移除：TUI 导航菜单中频繁出现，会导致 awaiting-input 误判
    "space to preview",
    "esc to cancel",
    "ctrl+a to",
    "ctrl+b to",
    // 布尔选择提示
    "y/n",
    "[y/n]",
    "(y/n)",
    "yes/no",
];

/// 排除短语：包含这些内容的行不触发 awaiting-input，即使也含有强交互短语
const AWAITING_EXCLUSIONS: &[&str] = &[
    "input tokens",
    "output tokens",
    "select * from",
    "approval policy",
    "permissionmode",
    "errorcode",
    // codex / claude 启动时的权限说明行（非交互）
    "allowed tools",
    "allowed:",
    "not allowed",
    "allowed operations",
    "approval mode",
];

/// 简单 ANSI strip：去掉 ESC[…m / OSC 等转义，保留可读文本
fn strip_ansi_simple(s: &str) -> String {
    let mut result = String::new();
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            match chars.peek() {
                Some(&'[') | Some(&'O') => {
                    chars.next();
                    for c2 in chars.by_ref() {
                        if ('\x40'..='\x7e').contains(&c2) {
                            break;
                        }
                    }
                }
                Some(&']') => {
                    // OSC: ESC ] ... BEL/ST — 终端标题等，完整消费避免文本泄漏
                    chars.next(); // consume ']'
                    loop {
                        match chars.next() {
                            None => break,
                            Some('\x07') => break,
                            Some('\x1b') => {
                                if chars.peek() == Some(&'\\') { chars.next(); }
                                break;
                            }
                            Some(_) => {}
                        }
                    }
                }
                _ => { chars.next(); }
            }
        } else {
            result.push(c);
        }
    }
    result
}

/// 检测近期输出是否包含明确的用户交互提示
/// 只检查强短语，排除技术性文本行
fn detect_awaiting_input(raw_output: &str) -> bool {
    let stripped = strip_ansi_simple(raw_output).replace('\r', "\n");
    for line in stripped.lines().rev().take(30) {
        let lower = line.trim().to_lowercase();
        if lower.is_empty() {
            continue;
        }
        // 若行内含排除短语，跳过此行
        if AWAITING_EXCLUSIONS.iter().any(|ex| lower.contains(ex)) {
            continue;
        }
        // 检查强交互短语
        if AWAITING_STRONG.iter().any(|phrase| lower.contains(phrase)) {
            return true;
        }
    }
    false
}

pub fn start_monitor(app: AppHandle, pty_manager: crate::pty::PtyManager) {
    thread::spawn(move || {
        // 存储上一次发送的 (status, provider) 对，避免重复 emit 相同状态
        let mut prev_states: HashMap<u32, (String, Option<String>)> = HashMap::new();

        loop {
            let pty_ids = pty_manager.get_pty_ids();

            for pty_id in &pty_ids {
                // ── Layer 2：进程级 banner 兜底检测 ──────────────────────────
                //
                // AI 会话检测采用双层架构：
                //   Layer 1（fast path）：pty.rs 中的命令 echo / output_since_enter 解析。
                //     优点：快（毫秒级），能在 AI 响应前就更新状态。
                //     弱点：依赖 shell echo 可被解析，上箭头/右箭头历史/自动补全
                //           场景下 echo 格式可能无法命中任何已知模式。
                //
                //   Layer 2（durable fallback）：此处扫描 recent_output_window
                //     中的 AI CLI 启动 banner（"Welcome to Claude Code" 等）。
                //     优点：不依赖命令 echo，AI CLI 只要成功启动并输出 banner 就能被捕获。
                //     代价：最多滞后 500ms（monitor 轮询间隔）。
                //
                // 两层缺一不可：Layer 1 保证快响应，Layer 2 保证最终一致性。
                pty_manager.try_reconcile_ai_from_banner(*pty_id);

                let (is_ai, prov) = pty_manager.get_ai_session_info(*pty_id);
                let (status, provider) = if is_ai {
                    let raw_window = pty_manager.get_recent_output_window(*pty_id);

                    let status = if detect_awaiting_input(&raw_window) {
                        // 明确的用户交互提示 → awaiting-input（优先于 generating）
                        "ai-awaiting-input"
                    } else if pty_manager.has_recent_output(*pty_id, AI_GENERATING_WINDOW) {
                        // 2s 内有输出 → 正在流式输出
                        "ai-generating"
                    } else if pty_manager.has_recent_output(*pty_id, AI_COMPLETE_TIMEOUT) {
                        // 2~30s 静默 → 思考/工具调用/网络等待，仍在工作
                        "ai-thinking"
                    } else {
                        // 超过 30s 无输出 → AI 完成一轮，等待下一条指令
                        "ai-complete"
                    };
                    (status, prov)
                } else {
                    ("idle", None)
                };

                let prev = prev_states.get(pty_id);
                let same = prev.map_or(false, |(ps, pp)| ps.as_str() == status && pp.as_deref() == provider.as_deref());
                if !same {
                    let _ = app.emit("pty-status-change", PtyStatusChangePayload {
                        pty_id: *pty_id,
                        status: status.to_string(),
                        provider: provider.clone(),
                    });
                    prev_states.insert(*pty_id, (status.to_string(), provider));
                }
            }

            prev_states.retain(|id, _| pty_ids.contains(id));

            let sleep_ms = if pty_ids.is_empty() { 2000 } else { 500 };
            thread::sleep(Duration::from_millis(sleep_ms));
        }
    });
}
