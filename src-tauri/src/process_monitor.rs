use serde::Serialize;
use std::collections::HashMap;
use std::thread;
use std::time::Duration;
use tauri::{AppHandle, Emitter};

/// Layer 3 检测的 AI CLI 命令名（与 pty.rs AI_COMMANDS 保持同步）
const AI_SUBPROCESS_NAMES: &[&str] = &["claude", "codex", "gemini", "grok"];

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

/// 系统进程快照条目：(pid, ppid, 去路径的 comm 名)
type ProcEntry = (u32, u32, String);

/// 抓一次系统进程列表。Unix 调 `ps`，Windows 暂不实现（返回 None，Layer 3 退化为 no-op）。
#[cfg(unix)]
fn snapshot_processes() -> Option<Vec<ProcEntry>> {
    use std::process::Command;
    let output = Command::new("ps")
        .args(["-A", "-o", "pid=,ppid=,comm="])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut result = Vec::with_capacity(256);
    for line in stdout.lines() {
        let line = line.trim_start();
        if line.is_empty() {
            continue;
        }
        let mut iter = line.split_whitespace();
        let Some(pid_s) = iter.next() else { continue };
        let Some(ppid_s) = iter.next() else { continue };
        let Ok(pid) = pid_s.parse::<u32>() else { continue };
        let Ok(ppid) = ppid_s.parse::<u32>() else { continue };
        // comm 可能含空格（"Google Chrome Helper"），把剩余部分合并
        let comm: String = iter.collect::<Vec<_>>().join(" ");
        if comm.is_empty() {
            continue;
        }
        // ps 在部分系统上输出含路径（如 "/usr/bin/node"），取 basename
        let base = comm.rsplit(&['/', '\\'][..]).next().unwrap_or(&comm).to_string();
        result.push((pid, ppid, base));
    }
    Some(result)
}

#[cfg(not(unix))]
fn snapshot_processes() -> Option<Vec<ProcEntry>> {
    None
}

/// 在进程快照中，从 root_pid 做 BFS，找到任意 comm 匹配 AI CLI 名字的后代。
/// 返回第一个匹配的 provider 名（稳定字符串切片）。
fn detect_ai_in_subtree(snapshot: &[ProcEntry], root_pid: u32) -> Option<&'static str> {
    // 构建 ppid -> children 索引
    let mut by_ppid: HashMap<u32, Vec<usize>> = HashMap::new();
    for (idx, entry) in snapshot.iter().enumerate() {
        by_ppid.entry(entry.1).or_default().push(idx);
    }

    let mut queue: Vec<u32> = vec![root_pid];
    // 深度保护：终端进程树一般很浅（shell → AI CLI → maybe node/python helper）
    let mut visited: std::collections::HashSet<u32> = std::collections::HashSet::new();
    while let Some(pid) = queue.pop() {
        if !visited.insert(pid) {
            continue;
        }
        if let Some(child_idxs) = by_ppid.get(&pid) {
            for &idx in child_idxs {
                let (child_pid, _, comm) = &snapshot[idx];
                let lower = comm.to_lowercase();
                // 去掉常见扩展名，如 windows 上的 .exe
                let stem = lower.trim_end_matches(".exe");
                for &ai in AI_SUBPROCESS_NAMES {
                    if stem == ai {
                        return Some(ai_to_static(ai));
                    }
                }
                queue.push(*child_pid);
            }
        }
    }
    None
}

/// 把动态 &str 映射到固定 &'static str，避免 lifetime 泄漏
fn ai_to_static(name: &str) -> &'static str {
    match name {
        "claude" => "claude",
        "codex" => "codex",
        "gemini" => "gemini",
        "grok" => "grok",
        _ => "unknown",
    }
}

pub fn start_monitor(app: AppHandle, pty_manager: crate::pty::PtyManager) {
    thread::spawn(move || {
        // 存储上一次发送的 (status, provider) 对，避免重复 emit 相同状态
        let mut prev_states: HashMap<u32, (String, Option<String>)> = HashMap::new();

        loop {
            let pty_ids = pty_manager.get_pty_ids();

            // ── Layer 3：系统进程快照（每轮一次，供所有 pty 共用） ─────────
            //
            // 不依赖终端输出，直接读 OS 进程表，扫 PTY 子进程树中是否跑着
            // claude / codex / gemini / grok。对以下场景免疫：
            //   - codex 启动时 MCP 错误刷屏把 banner 挤出窗口
            //   - claude --resume / codex --resume 长历史回放冲掉 shell echo
            //   - 用户通过 shell wrapper、history 召回等路径启动 AI，
            //     Layer 1 keystroke 缓冲未命中
            //
            // 代价：每 500ms 一次 `ps -A`，在典型桌面环境几 ms 级。
            let proc_snapshot = snapshot_processes();

            for pty_id in &pty_ids {
                // Layer 3：子进程名兜底（优先级最高，覆盖所有输出依赖场景）
                if let Some(ref snapshot) = proc_snapshot {
                    if let Some(shell_pid) = pty_manager.get_child_pid(*pty_id) {
                        if let Some(provider) = detect_ai_in_subtree(snapshot, shell_pid) {
                            pty_manager.force_ai_session(*pty_id, provider);
                        }
                    }
                }

                // ── Layer 2：进程级 banner 兜底检测 ──────────────────────────
                //
                // AI 会话检测采用三层架构：
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
                //   Layer 3（process truth，见上）：子进程树扫描。最稳，但仅 Unix。
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
