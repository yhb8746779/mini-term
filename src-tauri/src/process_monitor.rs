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
const AWAITING_STRONG: &[&str] = &[
    "allow",
    "approve",
    "authorization",
    "authorize",
    "grant access",
    "requires approval",
    "requesting approval",
    "do you want to allow",
    "continue?",
    "confirm",
    "are you sure",
    "press enter",
    "press any key",
    "hit enter",
    "choose an option",
    "select an option",
    "pick one",
    "use arrow keys",
    "space to preview",
    "esc to cancel",
    "ctrl+a to",
    "ctrl+b to",
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
];

/// 简单 ANSI strip：去掉 ESC[…m 类转义，保留可读文本
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
                let (status, provider) = if pty_manager.is_ai_session(*pty_id) {
                    let prov = pty_manager.get_ai_provider(*pty_id);
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
