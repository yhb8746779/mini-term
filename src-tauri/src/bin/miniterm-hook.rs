//! miniterm-hook CLI 小工具
//!
//! 极简二进制，被 Claude Code / Codex 的 hook 系统调用。
//! 功能：读 stdin JSON payload -> 读环境变量 -> POST 到 miniterm hook 服务器。
//!
//! 依赖最小化：仅使用 serde_json + 标准库，不引入额外 HTTP 客户端。

use std::io::Read;
use std::io::Write;
use std::net::TcpStream;
use std::time::Duration;

#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;

#[path = "../remote_status.rs"]
mod remote_status;
use remote_status::{encode_osc_status, RemoteStatusPayload};

/// 从 stdin 读取的超时时间（毫秒）
const STDIN_TIMEOUT_MS: u64 = 400;

fn main() {
    // 1. 获取事件名（从命令行参数）
    let event_name = std::env::args().nth(1).unwrap_or_default();
    let provider_arg = std::env::args().nth(2);
    // 远端 SSH/WSL 明确启用时只向当前 TTY 写受限 OSC 状态帧。
    // 不探测端口、不读取端口文件，也不发送原始 Hook payload。
    let remote_osc_only = is_remote_osc_only();

    // 2. 从 stdin 读取 JSON payload（带超时）
    let stdin_payload = read_stdin_with_timeout();

    // 3. 读取环境变量
    let pty_id = std::env::var("MINITERM_PTY_ID").ok();
    let cwd = std::env::current_dir()
        .ok()
        .map(|p| p.to_string_lossy().to_string());

    // 4. 构造统一 payload，供本地 HTTP 和远端 OSC 两条传输路径复用。
    let mut body = if let Some(ref payload) = stdin_payload {
        serde_json::from_str::<serde_json::Value>(payload).unwrap_or_else(|_| serde_json::json!({}))
    } else {
        serde_json::json!({})
    };

    // 注入字段
    if let Some(ref pty_id_str) = pty_id {
        if let Ok(id) = pty_id_str.parse::<u32>() {
            body["pty_id"] = serde_json::json!(id);
        }
    }
    if !event_name.is_empty() {
        body["event"] = serde_json::json!(event_name);
    }
    if !remote_osc_only {
        if let Some(ref cwd_str) = cwd {
            // 仅在 payload 中没有 cwd 时注入
            if body.get("cwd").is_none() {
                body["cwd"] = serde_json::json!(cwd_str);
            }
        }
    }

    // 推断 agent 类型
    if let Some(provider) = provider_arg {
        body["agent"] = serde_json::json!(provider);
    } else if body.get("agent").is_none() {
        // 尝试从 stdin payload 的字段推断
        let agent = if body.get("transcript_path").is_some() {
            "codex"
        } else {
            "claude-code"
        };
        body["agent"] = serde_json::json!(agent);
    }

    // 5. 本地优先 HTTP；远端强制模式不触发任何本机 HTTP/端口探测。
    let delivered = if remote_osc_only {
        false
    } else {
        let body_str = serde_json::to_string(&body).unwrap_or_else(|_| "{}".to_string());
        get_server_port()
            .map(|port| send_http_post(port, &body_str))
            .unwrap_or(false)
    };
    if !delivered {
        let write_outcome = emit_remote_osc(&body, &event_name);
        write_diagnostic(
            &event_name,
            stdin_payload.is_some(),
            body.get("session_id")
                .or_else(|| body.get("sessionId"))
                .and_then(|value| value.as_str())
                .is_some(),
            write_outcome,
        );
    }
}

fn is_remote_osc_only() -> bool {
    matches!(
        std::env::var("MINITERM_REMOTE_OSC").as_deref(),
        Ok("1") | Ok("true") | Ok("TRUE")
    )
}

fn status_for_event(event: &str) -> Option<&'static str> {
    match event {
        "SessionEnd" => Some("idle"),
        "PermissionRequest" | "Elicitation" => Some("ai-awaiting-input"),
        // Notification / SubagentStop 可能发生在主会话仍在继续时，不能提前完成 pane。
        "Stop" | "AfterAgent" => Some("ai-complete"),
        // SessionStart 只表示 CLI 会话建立，不代表一轮回答已经完成。
        // 不发送状态帧，避免启动/恢复时序覆盖正在工作的状态。
        "SessionStart" => None,
        "UserPromptSubmit"
        | "PreToolUse"
        | "PostToolUse"
        | "SubagentStart"
        | "PreCompact"
        | "PostCompact"
        | "BeforeAgent"
        | "BeforeToolSelection"
        | "BeforeTool"
        | "AfterModel" => Some("ai-thinking"),
        _ => None,
    }
}

fn emit_remote_osc(body: &serde_json::Value, event: &str) -> TtyWriteOutcome {
    let status = match status_for_event(event) {
        Some(status) => status,
        None => return TtyWriteOutcome::NotApplicable,
    };
    let provider = body
        .get("agent")
        .and_then(|v| v.as_str())
        .map(|v| match v {
            "claude-code" | "claude" => "claude",
            "codex" => "codex",
            "gemini" | "gemini-cli" => "gemini",
            "grok" => "grok",
            _ => "claude",
        })
        .unwrap_or("claude");
    let session_id = body
        .get("session_id")
        .or_else(|| body.get("sessionId"))
        .and_then(|v| v.as_str())
        .map(ToOwned::to_owned);
    let timestamp = now_millis();
    let seq = std::env::var("MINITERM_HOOK_SEQ")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .or_else(|| now_micros());
    let payload = RemoteStatusPayload {
        v: 1,
        event: event.to_string(),
        status: status.to_string(),
        provider: provider.to_string(),
        session_id,
        seq,
        timestamp: Some(timestamp),
    };
    let frame = match encode_osc_status(&payload) {
        Ok(frame) => frame,
        Err(_) => return TtyWriteOutcome::EncodeFailed,
    };
    write_to_tty(frame.as_bytes())
}

fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn now_micros() -> Option<u64> {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|d| d.as_micros() as u64)
}

#[derive(Clone, Copy)]
enum TtyWriteOutcome {
    NotApplicable,
    EncodeFailed,
    DevTtyWritten,
    DevTtyWriteFailed,
    StdoutWritten,
    StdoutWriteFailed,
}

impl TtyWriteOutcome {
    fn as_str(self) -> &'static str {
        match self {
            Self::NotApplicable => "not-applicable",
            Self::EncodeFailed => "encode-failed",
            Self::DevTtyWritten => "dev-tty-written",
            Self::DevTtyWriteFailed => "dev-tty-write-failed",
            Self::StdoutWritten => "stdout-written",
            Self::StdoutWriteFailed => "stdout-write-failed",
        }
    }
}

fn write_to_tty(bytes: &[u8]) -> TtyWriteOutcome {
    #[cfg(unix)]
    {
        // Claude 的 Hook 子进程通常没有 controlling tty，stdout 也会被 Claude
        // 捕获。SSH_TTY 是 SSH 为当前会话注入的 PTY 路径，优先写它可以把
        // OSC 直接送回对应的 MiniTerm pane。只接受 Unix 的标准终端节点：
        // Linux /dev/pts/<number>、macOS /dev/ttys<number> 与 /dev/tty，
        // 避免把状态帧写入任意外部路径。
        for path in [
            std::env::var_os("MINITERM_TTY"),
            std::env::var_os("SSH_TTY"),
            std::env::var_os("TTY"),
        ]
        .into_iter()
        .flatten()
        .map(std::path::PathBuf::from)
        .filter(|path| is_allowed_tty_path(path))
        {
            match write_to_path(&path, bytes) {
                Ok(()) => return TtyWriteOutcome::DevTtyWritten,
                Err(_) => continue,
            }
        }
        if let Ok(mut tty) = std::fs::OpenOptions::new().write(true).open("/dev/tty") {
            if tty.write_all(bytes).and_then(|_| tty.flush()).is_ok() {
                return TtyWriteOutcome::DevTtyWritten;
            }
            return TtyWriteOutcome::DevTtyWriteFailed;
        }
    }
    #[cfg(windows)]
    {
        if let Ok(mut tty) = std::fs::OpenOptions::new().write(true).open("CONOUT$") {
            if tty.write_all(bytes).and_then(|_| tty.flush()).is_ok() {
                return TtyWriteOutcome::DevTtyWritten;
            }
            return TtyWriteOutcome::DevTtyWriteFailed;
        }
    }
    let mut stdout = std::io::stdout();
    if stdout.write_all(bytes).and_then(|_| stdout.flush()).is_ok() {
        TtyWriteOutcome::StdoutWritten
    } else {
        TtyWriteOutcome::StdoutWriteFailed
    }
}

fn is_allowed_tty_path(path: &std::path::Path) -> bool {
    if path == std::path::Path::new("/dev/tty") {
        return true;
    }

    let Some(value) = path.to_str() else {
        return false;
    };
    let linux_pts = value
        .strip_prefix("/dev/pts/")
        .is_some_and(|suffix| !suffix.is_empty() && suffix.bytes().all(|byte| byte.is_ascii_digit()));
    let macos_ttys = value
        .strip_prefix("/dev/ttys")
        .is_some_and(|suffix| !suffix.is_empty() && suffix.bytes().all(|byte| byte.is_ascii_digit()));

    linux_pts || macos_ttys
}

#[cfg(unix)]
fn write_to_path(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut file = std::fs::OpenOptions::new().write(true).open(path)?;
    file.write_all(bytes)?;
    file.flush()
}

/// 仅在显式设置 MINITERM_HOOK_DEBUG=1 时写入最小诊断。
/// 日志不包含 stdin 原文、会话 ID、命令、cwd 或终端内容。
fn write_diagnostic(
    event: &str,
    stdin_received: bool,
    session_id_present: bool,
    write_outcome: TtyWriteOutcome,
) {
    if std::env::var("MINITERM_HOOK_DEBUG").as_deref() != Ok("1") {
        return;
    }
    let line = format!(
        "event={} stdin={} session_id={} tty={}\n",
        event,
        u8::from(stdin_received),
        u8::from(session_id_present),
        write_outcome.as_str(),
    );
    #[cfg(unix)]
    {
        let mut options = std::fs::OpenOptions::new();
        options.create(true).append(true).mode(0o600);
        if let Ok(mut file) = options.open("/tmp/miniterm-hook-debug.log") {
            let _ = file.write_all(line.as_bytes());
        }
    }
    #[cfg(not(unix))]
    {
        let _ = line;
    }
}

/// 从 stdin 读取 JSON，带超时保护
fn read_stdin_with_timeout() -> Option<String> {
    // 使用线程实现超时读取
    let (tx, rx) = std::sync::mpsc::channel();

    std::thread::spawn(move || {
        let mut input = String::new();
        let _ = std::io::stdin().read_to_string(&mut input);
        let _ = tx.send(input);
    });

    match rx.recv_timeout(Duration::from_millis(STDIN_TIMEOUT_MS)) {
        Ok(input) if !input.trim().is_empty() => Some(input),
        _ => None,
    }
}

/// 获取 hook 服务器端口
///
/// 优先从环境变量 MINITERM_HOOK_PORT 读取，然后从标准路径查找 hook-server.json
fn get_server_port() -> Option<u16> {
    // 优先从环境变量获取
    if let Ok(port_str) = std::env::var("MINITERM_HOOK_PORT") {
        if let Ok(port) = port_str.parse::<u16>() {
            return Some(port);
        }
    }

    // 从 hook-server.json 文件获取
    let port_file = get_port_file_path()?;
    let content = std::fs::read_to_string(port_file).ok()?;
    let json: serde_json::Value = serde_json::from_str(&content).ok()?;
    json.get("port")?.as_u64().map(|p| p as u16)
}

/// 获取 hook-server.json 的平台特定路径
///
/// ⚠️ APP_IDENTIFIER 必须与 src-tauri/tauri.conf.json 的 `identifier` 字段保持一致，
/// 否则 hook server 写端口文件用一个目录、helper fallback 读用另一个目录，
/// 会导致 MINITERM_HOOK_PORT 环境变量丢失（tmux/screen/外部 shell 启动 AI 等场景）
/// 时连不上 server。当前 tauri.conf.json identifier = "com.tauri-app.tauri-app"。
///
/// 注：环境变量 fast path（MINITERM_HOOK_PORT）仍是首选，此路径仅作 fallback。
const APP_IDENTIFIER: &str = "com.tauri-app.tauri-app";

fn get_port_file_path() -> Option<std::path::PathBuf> {
    #[cfg(target_os = "windows")]
    {
        // Windows: %APPDATA%/<identifier>/hook-server.json
        std::env::var("APPDATA").ok().map(|appdata| {
            std::path::PathBuf::from(appdata)
                .join(APP_IDENTIFIER)
                .join("hook-server.json")
        })
    }

    #[cfg(target_os = "macos")]
    {
        // macOS: ~/Library/Application Support/<identifier>/hook-server.json
        std::env::var_os("HOME").map(|home| {
            std::path::PathBuf::from(home)
                .join("Library")
                .join("Application Support")
                .join(APP_IDENTIFIER)
                .join("hook-server.json")
        })
    }

    #[cfg(target_os = "linux")]
    {
        // Linux: $XDG_DATA_HOME/<identifier>/hook-server.json
        // 或 ~/.local/share/<identifier>/hook-server.json
        let data_dir = std::env::var("XDG_DATA_HOME")
            .ok()
            .map(std::path::PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME")
                    .map(std::path::PathBuf::from)
                    .map(|h| h.join(".local").join("share"))
            });
        data_dir.map(|d| d.join(APP_IDENTIFIER).join("hook-server.json"))
    }

    #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
    {
        None
    }
}

/// 使用原始 HTTP 发送 POST 请求到本地 hook 服务器
///
/// 不等待响应，尽快退出以不阻塞 AI 工具
fn send_http_post(port: u16, body: &str) -> bool {
    let addr = format!("127.0.0.1:{}", port);

    // 连接超时 100ms
    let sock_addr = match addr.parse() {
        Ok(a) => a,
        Err(_) => return false,
    };
    let stream = match TcpStream::connect_timeout(&sock_addr, Duration::from_millis(100)) {
        Ok(s) => s,
        Err(_) => return false, // 连接失败时允许回退到 OSC
    };

    // 设置写超时
    let _ = stream.set_write_timeout(Some(Duration::from_millis(100)));

    let request = format!(
        "POST /hook HTTP/1.1\r\n\
         Host: 127.0.0.1:{}\r\n\
         Content-Type: application/json\r\n\
         Content-Length: {}\r\n\
         Connection: close\r\n\
         \r\n\
         {}",
        port,
        body.len(),
        body
    );

    let mut stream = stream;
    if stream.write_all(request.as_bytes()).is_err() {
        return false;
    }
    if stream.flush().is_err() {
        return false;
    }
    // 不读取响应，立即退出
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_claude_events_to_expected_statuses() {
        assert_eq!(status_for_event("PreToolUse"), Some("ai-thinking"));
        assert_eq!(status_for_event("PermissionRequest"), Some("ai-awaiting-input"));
        assert_eq!(status_for_event("Stop"), Some("ai-complete"));
        assert_eq!(status_for_event("SessionStart"), None);
        assert_eq!(status_for_event("Notification"), None);
        assert_eq!(status_for_event("SubagentStop"), None);
        assert_eq!(status_for_event("SessionEnd"), Some("idle"));
        assert_eq!(status_for_event("unknown"), None);
    }

    #[test]
    fn remote_mode_accepts_only_explicit_values() {
        std::env::remove_var("MINITERM_REMOTE_OSC");
        assert!(!is_remote_osc_only());
        std::env::set_var("MINITERM_REMOTE_OSC", "1");
        assert!(is_remote_osc_only());
        std::env::set_var("MINITERM_REMOTE_OSC", "false");
        assert!(!is_remote_osc_only());
        std::env::remove_var("MINITERM_REMOTE_OSC");
    }

    #[test]
    fn accepts_only_known_unix_tty_nodes() {
        assert!(is_allowed_tty_path(std::path::Path::new("/dev/tty")));
        assert!(is_allowed_tty_path(std::path::Path::new("/dev/pts/9")));
        assert!(is_allowed_tty_path(std::path::Path::new("/dev/ttys001")));
        assert!(!is_allowed_tty_path(std::path::Path::new("/dev/pts/not-a-tty")));
        assert!(!is_allowed_tty_path(std::path::Path::new("/dev/ttys/1")));
        assert!(!is_allowed_tty_path(std::path::Path::new("/tmp/miniterm.log")));
    }
}
