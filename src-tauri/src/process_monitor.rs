use serde::Serialize;
use std::collections::HashMap;
use std::thread;
use std::time::{Duration, Instant};
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

/// AI 在产出 token 的活跃窗口：与 busy signal 共同满足才视为 ai-generating
const AI_GENERATING_WINDOW: Duration = Duration::from_secs(2);
/// AI TUI busy 信号（spinner 时长括号 / `esc to xxx`）的有效窗口。
/// pty.rs reader 在每个 chunk 里实时扫，命中即刷新时间戳。
/// 三家 spinner 计时器都至少每秒刷新一次（codex `Working (Ns)`、claude
/// `Contemplating (Ns)`、gemini `(esc to cancel, Ns)` 的 N 都是秒级递增），
/// 5s 窗口足够维持"AI 仍在工作"判定；spinner 一旦从屏幕消失就立即视为 complete。
const AI_BUSY_SIGNAL_WINDOW: Duration = Duration::from_secs(5);
/// Layer 3 反向裁定的宽限期：连续 N 秒在 PTY 子进程树中看不到 AI CLI
/// 进程时，视为 AI 已退出（含 codex MCP 启动失败自退、claude 异常 crash、
/// 用户从 IDE 终止进程等不经过 keyboard exit_ai 路径的场景），清除会话标记。
///
/// 5s 宽限的取舍：
/// - 用户键入 `claude<Enter>` 后，shell 到 fork 出 claude 进程通常 200~500ms，
///   慢机器/冷启动最多 ~2s；5s 留有充裕余量，避免误清刚启动的会话。
/// - 太长会让"AI 已退出但状态点继续闪"的窗口期太久（用户可见的 bug 时长）。
const AI_SUBPROCESS_GRACE: Duration = Duration::from_secs(5);

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
    // "esc to cancel" 已移除：gemini 在 working 态会显示 "(esc to cancel, 10s)"
    // 表示"按 esc 中断当前思考"（与 codex 的 esc to interrupt 同义），
    // 是 busy 提示而非等待用户输入；交给 chunk_has_busy_signal 处理为 ai-thinking。
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

/// 系统进程快照条目：(pid, ppid, 去路径的 comm 名, 可选的小写命令行)
/// - 命令行：Windows 用 sysinfo 填充（小写化，argv 以空格 join），用于识别
///   node.exe 这类启动器进程里跑的具体 AI CLI（gemini-cli / codex.js 等）；
///   Unix 上保持 None，进程名本身就足够精确（gemini / codex 都是原生二进制）。
type ProcEntry = (u32, u32, String, Option<String>);

#[derive(Debug, Default, PartialEq, Eq)]
struct SubtreeObservation {
    ai_provider: Option<&'static str>,
    has_ssh: bool,
}

/// 抓一次系统进程列表。Unix 调 `ps -A`，Windows 用 sysinfo（含命令行）。
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
        result.push((pid, ppid, base, None));
    }
    Some(result)
}

/// Windows 实现：用 sysinfo 枚举所有进程，并收集每个进程的命令行。
///
/// 为什么不用 ToolHelp32：ToolHelp32 只给 szExeFile（进程名），无法识别
/// `node.exe` 这种启动器进程里跑的具体脚本（如 gemini-cli）。gemini CLI
/// 是纯 npm 包，Windows 上进程表里只有 node.exe，必须读命令行 argv 才能
/// 判断它跑的是 gemini 还是其他任意 node 程序。
///
/// 代价：sysinfo 内部对每个进程读 PEB + ReadProcessMemory 取命令行，
/// 大约 10-30ms 一次全量扫描；每 500ms 一次，对 CPU 压力很小。
#[cfg(windows)]
fn snapshot_processes() -> Option<Vec<ProcEntry>> {
    use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};

    let mut sys = System::new();
    // 显式启用 cmd / exe 字段刷新。sysinfo 0.32 默认 refresh_processes 不一定拉命令行
    // （实测 Windows 下 process.cmd() 返回空），必须用 specifics + UpdateKind::Always。
    let refresh_kind = ProcessRefreshKind::new()
        .with_cmd(UpdateKind::Always)
        .with_exe(UpdateKind::Always);
    sys.refresh_processes_specifics(ProcessesToUpdate::All, true, refresh_kind);

    let mut result: Vec<ProcEntry> = Vec::with_capacity(sys.processes().len());
    for (pid, process) in sys.processes() {
        let pid_u32 = pid.as_u32();
        let ppid = process.parent().map(|p| p.as_u32()).unwrap_or(0);

        let name = process.name().to_string_lossy();
        if name.is_empty() {
            continue;
        }
        // 取 basename，对齐 Unix 版
        let base = name.rsplit(&['/', '\\'][..]).next().unwrap_or(&name).to_string();

        // 收集命令行：argv 以空格 join，整体转小写便于匹配
        let cmd = process.cmd();
        let cmdline = if cmd.is_empty() {
            None
        } else {
            let joined: String = cmd
                .iter()
                .map(|s| s.to_string_lossy())
                .collect::<Vec<_>>()
                .join(" ")
                .to_lowercase();
            Some(joined)
        };

        result.push((pid_u32, ppid, base, cmdline));
    }
    Some(result)
}

#[cfg(not(any(unix, windows)))]
fn snapshot_processes() -> Option<Vec<ProcEntry>> {
    None
}

/// 从 node.exe / cmd.exe / pwsh.exe 等启动器进程的命令行中识别跑的是哪家 AI CLI。
/// 参数 `cmdline` 必须已是小写（snapshot 已做），用裸 `contains` 匹配即可。
///
/// 匹配的是 npm 包路径特征，而非随意字符串，避免 "我提到 gemini" 之类的误触发。
fn detect_ai_from_cmdline(cmdline: &str) -> Option<&'static str> {
    // Gemini：Windows 下只有这条路径可达（无原生 gemini.exe）
    if cmdline.contains("@google/gemini-cli")
        || cmdline.contains("/gemini-cli/")
        || cmdline.contains("\\gemini-cli\\")
        || cmdline.contains("/gemini.js")
        || cmdline.contains("\\gemini.js")
    {
        return Some("gemini");
    }
    // Codex：虽然 Windows 有原生 codex.exe，但启动链中的 node wrapper 阶段仍需兜底
    if cmdline.contains("@openai/codex")
        || cmdline.contains("/codex.js")
        || cmdline.contains("\\codex.js")
    {
        return Some("codex");
    }
    // Claude Code：Windows 有原生 claude.exe shim，但 npm 全局某些版本仍走 node 路径
    if cmdline.contains("@anthropic-ai/claude-code")
        || cmdline.contains("/claude-code/")
        || cmdline.contains("\\claude-code\\")
    {
        return Some("claude");
    }
    // Grok 预留：目前 grok CLI 形态未定，先留命令行兜底钩子
    if cmdline.contains("/grok-cli")
        || cmdline.contains("\\grok-cli")
        || cmdline.contains("/grok.js")
        || cmdline.contains("\\grok.js")
    {
        return Some("grok");
    }
    None
}

/// 在进程快照中，从 root_pid 做 BFS，同时识别本地 AI CLI 和交互式 SSH 传输。
/// root 本身也参与识别，以覆盖直接把 ssh.exe 作为 PTY 子进程启动的场景。
///
/// 两级匹配：
/// 1. 进程名直接匹配（claude.exe / codex.exe / gemini / grok）—— 覆盖原生二进制
/// 2. node.exe / cmd.exe / pwsh.exe 等启动器进程 + 命令行参数匹配 npm 包名 ——
///    覆盖 Windows 下 gemini-cli 这类"只有 node.exe 没有原生 exe"的场景
fn inspect_process_subtree(snapshot: &[ProcEntry], root_pid: u32) -> SubtreeObservation {
    // 构建 ppid -> children 索引
    let mut by_ppid: HashMap<u32, Vec<usize>> = HashMap::new();
    let mut by_pid: HashMap<u32, usize> = HashMap::new();
    for (idx, entry) in snapshot.iter().enumerate() {
        by_pid.insert(entry.0, idx);
        by_ppid.entry(entry.1).or_default().push(idx);
    }

    let mut queue: Vec<u32> = vec![root_pid];
    let mut observation = SubtreeObservation::default();
    // 深度保护：终端进程树一般很浅（shell → AI CLI → maybe node/python helper）
    let mut visited: std::collections::HashSet<u32> = std::collections::HashSet::new();
    while let Some(pid) = queue.pop() {
        if !visited.insert(pid) {
            continue;
        }
        if let Some(&idx) = by_pid.get(&pid) {
            let (_, _, comm, cmdline) = &snapshot[idx];
            let lower = comm.to_lowercase();
            let stem = lower.trim_end_matches(".exe");
            if stem == "ssh" {
                observation.has_ssh = true;
            }
            if observation.ai_provider.is_none() {
                observation.ai_provider = AI_SUBPROCESS_NAMES
                    .iter()
                    .find(|&&ai| stem == ai)
                    .map(|&ai| ai_to_static(ai))
                    .or_else(|| {
                        // ssh.exe 的 argv 可能包含远端命令或路径，不能据此认定
                        // 主机存在 AI 子进程；远端状态只能由 PTY 数据流推断。
                        if stem == "ssh" {
                            None
                        } else {
                            cmdline.as_deref().and_then(detect_ai_from_cmdline)
                        }
                    });
            }
        }
        if let Some(child_idxs) = by_ppid.get(&pid) {
            for &idx in child_idxs {
                queue.push(snapshot[idx].0);
            }
        }
    }
    observation
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
        // Layer 3 反向裁定用：每个 pty 最近一次在子进程树中观察到 AI CLI 的时间。
        // 连续 AI_SUBPROCESS_GRACE 都未观察到则清除会话标记。
        let mut last_seen_ai_subproc: HashMap<u32, Instant> = HashMap::new();
        // SSH 内的 AI 进程不在主机进程表中。这里只记录本地进程树是否处于 SSH
        // 传输，不向远端安装 hook、转发环境变量或读写任何远端配置文件。
        let mut ssh_transport_ptys: std::collections::HashSet<u32> =
            std::collections::HashSet::new();

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
            // 代价：每 500ms 一次快照（Unix `ps -A` / Windows ToolHelp32），几 ms 级。
            let proc_snapshot = snapshot_processes();

            for pty_id in &pty_ids {
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
                //     弱点：长历史/大输出里若出现其他 provider 的关键词可能误判。
                //
                //   Layer 3（process truth，下方）：PTY 子进程树扫 AI 可执行名。
                //     最强真相源：不依赖任何终端输出。Unix 用 `ps -A`，Windows 用 ToolHelp32。
                //
                // 顺序：Layer 2 先跑，Layer 3 后跑并覆盖。 Layer 3 永远是最终裁判——
                // Layer 2 若被长历史里的异构 provider 关键词误判（例如 claude --resume
                // 回放到提及 grok/xAI 的旧对话），Layer 3 看到实际子进程是 claude 后
                // 立刻把 provider 纠正回来。
                pty_manager.try_reconcile_ai_from_banner(*pty_id);

                // Layer 3：子进程名真相源，最后生效，覆盖 Layer 1/2 可能的误判
                let mut layer3_saw_ai = false;
                let mut layer3_saw_ssh = false;
                if let Some(ref snapshot) = proc_snapshot {
                    if let Some(shell_pid) = pty_manager.get_child_pid(*pty_id) {
                        let observation = inspect_process_subtree(snapshot, shell_pid);
                        layer3_saw_ssh = observation.has_ssh;
                        if let Some(provider) = observation.ai_provider {
                            pty_manager.force_ai_session(*pty_id, provider);
                            layer3_saw_ai = true;
                        }
                    }
                }

                let (mut is_ai, mut prov) = pty_manager.get_ai_session_info(*pty_id);

                // ── Layer 3 反向裁定：AI 子进程消失则撤销会话标记 ────────────
                //
                // Layer 1（keyboard）的 exit_ai 仅覆盖 /exit、Ctrl+D、双 Ctrl+C
                // 等"用户主动退出"路径。AI CLI 自身 crash/exit（如 codex 因
                // MCP 启动失败立即返回到 shell、claude 异常退出）时这些键盘
                // 信号不会触发，会话标记会一直留着导致状态点持续闪烁。
                //
                // Layer 3 在子进程快照可用时，看见 AI CLI 就刷新时间戳，
                // 看不见则计时；超过宽限期即调 clear_ai_session 把会话清掉。
                // proc_snapshot 失败（极少数权限/IO 错误）时跳过本轮不冒进清除。
                if proc_snapshot.is_some() {
                    if is_ai {
                        let now = Instant::now();
                        if layer3_saw_ai {
                            last_seen_ai_subproc.insert(*pty_id, now);
                            ssh_transport_ptys.remove(pty_id);
                        } else if layer3_saw_ssh {
                            // 远端 AI 无法出现在主机进程表中，不能把"未看到本地 AI"
                            // 当成退出证据。保留 Layer 1/2 建立的会话，并继续按 PTY
                            // 输出推断 thinking/generating/complete/awaiting-input。
                            ssh_transport_ptys.insert(*pty_id);
                            last_seen_ai_subproc.remove(pty_id);
                        } else if ssh_transport_ptys.remove(pty_id) {
                            // SSH 传输已经退出，远端 AI 必然不再连接到当前 PTY。
                            pty_manager.clear_ai_session(*pty_id);
                            is_ai = false;
                            prov = None;
                        } else {
                            let last = *last_seen_ai_subproc.entry(*pty_id).or_insert(now);
                            if now.duration_since(last) >= AI_SUBPROCESS_GRACE {
                                pty_manager.clear_ai_session(*pty_id);
                                last_seen_ai_subproc.remove(pty_id);
                                is_ai = false;
                                prov = None;
                            }
                        }
                    } else {
                        last_seen_ai_subproc.remove(pty_id);
                        ssh_transport_ptys.remove(pty_id);
                    }
                }
                let (status, provider) = if is_ai {
                    let raw_window = pty_manager.get_recent_output_window(*pty_id);

                    // 状态判定：以"屏幕上的 spinner busy 信号"为唯一 working 真相源，
                    // 不再用单纯的字节流活跃度兜底。原因：gemini 等 CLI 在 idle 时
                    // 仍会持续每秒重绘整个屏幕（cursor blink + footer 状态栏），
                    // 让 has_recent_output(30s) 永远命中导致状态点常驻慢呼吸。
                    let busy = pty_manager.has_recent_busy_signal(*pty_id, AI_BUSY_SIGNAL_WINDOW);
                    let active = pty_manager.has_recent_output(*pty_id, AI_GENERATING_WINDOW);
                    let status = if detect_awaiting_input(&raw_window) {
                        "ai-awaiting-input"
                    } else if busy && active {
                        // spinner 在屏幕上 + 2s 内 PTY 还在吐字节 → 真在输出 token
                        "ai-generating"
                    } else if busy {
                        // spinner 在屏幕上 + 字节流暂停 → 思考/工具调用/网络等待
                        "ai-thinking"
                    } else {
                        // spinner 已不在屏幕（或从未出现）→ 完成一轮，等待下一条指令
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
            last_seen_ai_subproc.retain(|id, _| pty_ids.contains(id));
            ssh_transport_ptys.retain(|id| pty_ids.contains(id));

            let sleep_ms = if pty_ids.is_empty() { 2000 } else { 500 };
            thread::sleep(Duration::from_millis(sleep_ms));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn proc_entry(pid: u32, ppid: u32, name: &str) -> ProcEntry {
        (pid, ppid, name.to_string(), None)
    }

    #[test]
    fn subtree_detects_local_ai_provider() {
        let snapshot = vec![
            proc_entry(10, 0, "powershell.exe"),
            proc_entry(11, 10, "claude.exe"),
        ];

        let observation = inspect_process_subtree(&snapshot, 10);

        assert_eq!(observation.ai_provider, Some("claude"));
        assert!(!observation.has_ssh);
    }

    #[test]
    fn subtree_detects_ssh_below_local_shell() {
        let snapshot = vec![
            proc_entry(20, 0, "powershell.exe"),
            proc_entry(21, 20, "ssh.exe"),
        ];

        let observation = inspect_process_subtree(&snapshot, 20);

        assert_eq!(observation.ai_provider, None);
        assert!(observation.has_ssh);
    }

    #[test]
    fn subtree_detects_ssh_when_it_is_the_pty_root() {
        let snapshot = vec![proc_entry(30, 0, "ssh.exe")];

        let observation = inspect_process_subtree(&snapshot, 30);

        assert_eq!(observation.ai_provider, None);
        assert!(observation.has_ssh);
    }

    #[test]
    fn ssh_remote_command_is_not_mistaken_for_local_ai_process() {
        let snapshot = vec![(
            35,
            0,
            "ssh.exe".to_string(),
            Some("ssh vm /usr/local/lib/claude-code/bin/claude".to_string()),
        )];

        let observation = inspect_process_subtree(&snapshot, 35);

        assert_eq!(observation.ai_provider, None);
        assert!(observation.has_ssh);
    }

    #[test]
    fn ssh_agent_is_not_mistaken_for_interactive_ssh() {
        let snapshot = vec![
            proc_entry(40, 0, "powershell.exe"),
            proc_entry(41, 40, "ssh-agent.exe"),
        ];

        let observation = inspect_process_subtree(&snapshot, 40);

        assert!(!observation.has_ssh);
    }

    #[test]
    fn subtree_reports_local_ai_even_when_ssh_also_exists() {
        let snapshot = vec![
            proc_entry(50, 0, "powershell.exe"),
            proc_entry(51, 50, "claude.exe"),
            proc_entry(52, 51, "ssh.exe"),
        ];

        let observation = inspect_process_subtree(&snapshot, 50);

        assert_eq!(observation.ai_provider, Some("claude"));
        assert!(observation.has_ssh);
    }
}
