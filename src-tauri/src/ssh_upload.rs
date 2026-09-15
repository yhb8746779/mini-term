//! SSH 远端图片投递。
//!
//! 背景：pane 里跑着 AI CLI 的 TUI 时，mini-term 只能往 pty 里发按键，
//! 没法在那个会话里执行命令。所以图片要送到远端，只能**另开一条 ssh 连接**。
//!
//! 为什么用 `ssh <flags> <target> "cat > /tmp/x.png"` 而不是 scp：
//!   scp 和 ssh 的参数语义不通用（ssh 的 `-p 端口` 在 scp 里是 `-P`，
//!   而 scp 的 `-p` 表示保留时间戳）。照搬用户 argv 调 scp 会静默连错机器。
//!   用同一个 ssh 二进制就没有这个问题：同一套参数、同一套 ~/.ssh/config 解析。
//!
//! 为什么不解析 HostName/Port/IdentityFile：
//!   用户敲的可能只是一个 config 别名（实测就是 `ssh 2`）。把别名原样传回给
//!   ssh，让它自己查 config，比在这里重新实现一遍解析可靠得多。

use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// 上传超时。大截图走慢速链路可能要几秒，但不能让 UI 无限等。
const UPLOAD_TIMEOUT: Duration = Duration::from_secs(30);
/// 远端落盘目录。选 /tmp 是因为它在任何 POSIX 远端都存在且通常可写。
const REMOTE_DIR: &str = "/tmp";
/// 远端旧图清理阈值（分钟）。跟本地 cleanup_old_images 的 24h 对齐。
const REMOTE_STALE_MINUTES: u32 = 24 * 60;

static UPLOAD_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// ssh 里"要吃掉下一个参数"的短选项。
const VALUE_FLAGS: &[char] = &[
    'B', 'b', 'c', 'D', 'E', 'e', 'F', 'I', 'i', 'J', 'L', 'l', 'm', 'O', 'o', 'p', 'Q', 'R', 'S',
    'W', 'w',
];

/// ssh 里不带值的开关选项。
const BOOL_FLAGS: &[char] = &[
    '4', '6', 'A', 'a', 'C', 'f', 'G', 'g', 'K', 'k', 'M', 'N', 'n', 'q', 's', 'T', 't', 'V', 'v',
    'X', 'x', 'Y', 'y',
];

/// 重放时必须丢掉的开关：它们会让"一条命令 + stdin 灌数据"这件事失败。
///   f/N —— 后台化 / 不执行远端命令，stdin 根本送不进去
///   M   —— master 模式
///   T/t —— tty 分配，我们要的是纯管道
///   s   —— subsystem
///   G/V —— 只打印配置/版本，不建立连接
const DROP_BOOL_FLAGS: &[char] = &['f', 'N', 'M', 'T', 't', 's', 'G', 'V'];

/// 重放时必须连值一起丢掉的选项：端口转发和控制命令与本次上传无关，
/// 且可能因端口占用直接失败。
const DROP_VALUE_FLAGS: &[char] = &['D', 'L', 'R', 'W', 'w', 'O', 'Q'];

/// 从原始 ssh argv 构造一条"上传用"的 ssh 命令。
///
/// 返回 None 表示 argv 里找不到目标主机（例如只有 `ssh` 没有参数）。
///
/// 保留用户原有的连接相关参数（-p/-i/-J/-o/-F/-l 等），追加：
///   BatchMode=yes    —— 密码认证的机器立刻失败，而不是挂在那儿等一个
///                       永远不会来的输入（没有 tty，用户也没处输）
///   ConnectTimeout=5 —— 网络不可达时快速失败
fn build_upload_argv(original: &[String], remote_command: &str) -> Option<Vec<String>> {
    let mut preserved: Vec<String> = Vec::new();
    let mut destination: Option<String> = None;

    let mut iter = original.iter().skip(1).peekable();
    while let Some(arg) = iter.next() {
        // 非选项 → 就是目标主机，它后面的全是远端命令，丢弃
        if !arg.starts_with('-') || arg == "-" {
            destination = Some(arg.clone());
            break;
        }
        if arg == "--" {
            if let Some(next) = iter.next() {
                destination = Some(next.clone());
            }
            break;
        }

        let chars: Vec<char> = arg.chars().skip(1).collect();
        let Some(&first) = chars.first() else { continue };

        if VALUE_FLAGS.contains(&first) {
            // 值可能贴在同一个 token 里（-p2222），也可能是下一个 token（-p 2222）
            let attached = chars.len() > 1;
            let value = if attached {
                Some(chars[1..].iter().collect::<String>())
            } else {
                iter.next().cloned()
            };
            if DROP_VALUE_FLAGS.contains(&first) {
                continue;
            }
            preserved.push(format!("-{first}"));
            if let Some(value) = value {
                preserved.push(value);
            }
            continue;
        }

        // 开关，可能是捆绑写法（-tt / -vv / -4C）
        if chars.iter().all(|c| BOOL_FLAGS.contains(c)) {
            let kept: String = chars
                .iter()
                .filter(|c| !DROP_BOOL_FLAGS.contains(c))
                .collect();
            if !kept.is_empty() {
                preserved.push(format!("-{kept}"));
            }
            continue;
        }
        // 无法识别的选项：原样保留，交给 ssh 自己报错，好过我们猜错
        preserved.push(arg.clone());
    }

    let destination = destination?;

    let mut argv = vec!["ssh".to_string()];
    argv.extend(preserved);
    argv.push("-o".into());
    argv.push("BatchMode=yes".into());
    argv.push("-o".into());
    argv.push("ConnectTimeout=5".into());
    argv.push(destination);
    argv.push(remote_command.to_string());
    Some(argv)
}

/// 生成远端文件名。只允许字母数字扩展名，避免拼进 shell 命令时出岔子。
fn remote_file_name(local_path: &std::path::Path) -> String {
    let ext: String = local_path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("png")
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect::<String>()
        .to_lowercase();
    let ext = if ext.is_empty() { "png".into() } else { ext };
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let sequence = UPLOAD_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    format!("miniterm-{nanos}-{}-{sequence}.{ext}", std::process::id())
}

/// 图片桥接通常按文件名前缀筛选，前缀由用户在 MiniTerm 设置里配置。
fn clipboard_bridge_file_name(local_path: &std::path::Path, prefix: &str) -> String {
    let name = remote_file_name(local_path);
    format!("{prefix}{name}")
}

/// 远端命令：接收 stdin 落盘，成功后顺手清掉过期的旧图。
/// 清理失败不影响本次结果（远端可能没有 find）。
fn remote_command_for(remote_path: &str) -> String {
    format!(
        "cat > '{remote_path}' && (find {REMOTE_DIR} -maxdepth 1 -name 'miniterm-*' -mmin +{REMOTE_STALE_MINUTES} -delete 2>/dev/null || true)"
    )
}

/// 从 ssh argv 取目标主机（支持 `ssh -p 22 alias` 和 config alias）。
fn ssh_destination(original: &[String]) -> Option<String> {
    let mut iter = original.iter().skip(1).peekable();
    while let Some(arg) = iter.next() {
        if !arg.starts_with('-') || arg == "-" {
            return Some(arg.clone());
        }
        if arg == "--" {
            return iter.next().cloned();
        }
        let chars: Vec<char> = arg.chars().skip(1).collect();
        let Some(&first) = chars.first() else { continue };
        if VALUE_FLAGS.contains(&first) && chars.len() == 1 {
            let _ = iter.next();
        }
    }
    None
}

fn validate_bridge_config(
    bridge: &crate::config::SshImageBridgeConfig,
) -> Result<(String, String, String, String), String> {
    let host = bridge.ssh_host.trim();
    if host.is_empty() || host.chars().any(char::is_whitespace) {
        return Err("SSH 图片桥接的目标主机不能为空且不能包含空格".into());
    }

    let directory = bridge.remote_directory.trim_end_matches('/');
    if !directory.starts_with('/')
        || directory.len() < 2
        || !directory
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '_' | '-' | '.'))
    {
        return Err("SSH 图片桥接目录必须是安全的绝对 POSIX 路径".into());
    }

    let prefix = bridge.file_prefix.trim();
    if prefix.is_empty()
        || !prefix
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-'))
    {
        return Err("SSH 图片桥接文件名前缀只能包含字母、数字、下划线和连字符".into());
    }

    let probe = bridge.probe_command.trim();
    if probe.is_empty() || probe.len() > 512 || probe.contains(['\r', '\n', '\0']) {
        return Err("SSH 图片桥接探测命令不能为空、不能换行且最长 512 字符".into());
    }

    Ok((
        host.into(),
        directory.into(),
        prefix.into(),
        probe.replace("{directory}", directory),
    ))
}

/// 读取指定 pid 的完整 argv（不做小写化——config 别名和路径是大小写敏感的）。
#[cfg(windows)]
fn process_argv(pid: u32) -> Option<Vec<String>> {
    use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};

    let mut sys = System::new();
    let target = Pid::from_u32(pid);
    let refresh_kind = ProcessRefreshKind::new().with_cmd(UpdateKind::Always);
    sys.refresh_processes_specifics(ProcessesToUpdate::Some(&[target]), true, refresh_kind);
    let cmd = sys.process(target)?.cmd();
    if cmd.is_empty() {
        return None;
    }
    Some(cmd.iter().map(|s| s.to_string_lossy().into_owned()).collect())
}

/// Unix 版：进程快照里不带 argv（`ps -o comm=` 只有进程名），
/// 这里对目标 pid 单独查一次。
#[cfg(unix)]
fn process_argv(pid: u32) -> Option<Vec<String>> {
    let output = Command::new("ps")
        .args(["-o", "args=", "-p", &pid.to_string()])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let line = String::from_utf8_lossy(&output.stdout);
    let argv: Vec<String> = line.split_whitespace().map(|s| s.to_string()).collect();
    if argv.is_empty() {
        None
    } else {
        Some(argv)
    }
}

/// 把本地图片推到该 pane 所连的远端，返回远端绝对路径。
///
/// 调用前提：前端已确认该 pane 的 transport 是 ssh。
#[tauri::command]
fn upload_image_to_remote(
    pty_manager: &crate::pty::PtyManager,
    pty_id: u32,
    local_path: &str,
    remote_path: String,
    remote_command: String,
) -> Result<String, String> {
    let ssh_pid = crate::process_monitor::find_ssh_pid_for_pty(pty_manager, pty_id)
        .ok_or_else(|| "未找到该终端对应的 ssh 进程".to_string())?;
    let argv = process_argv(ssh_pid).ok_or_else(|| "无法读取 ssh 命令行".to_string())?;

    let local = std::path::PathBuf::from(local_path);
    let bytes = std::fs::read(&local).map_err(|e| format!("读取本地图片失败: {e}"))?;

    let upload_argv = build_upload_argv(&argv, &remote_command)
        .ok_or_else(|| "ssh 命令行里没有目标主机，无法复现连接".to_string())?;

    let mut command = Command::new(&upload_argv[0]);
    command
        .args(&upload_argv[1..])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        command.creation_flags(CREATE_NO_WINDOW);
    }

    let mut child = command.spawn().map_err(|e| format!("启动 ssh 失败: {e}"))?;

    // stdin 在独立线程里写：远端 cat 消费得慢时不阻塞这边的超时轮询。
    let mut stdin = child.stdin.take().ok_or_else(|| "无法写入 ssh stdin".to_string())?;
    let writer = std::thread::spawn(move || {
        let result = stdin.write_all(&bytes).and_then(|_| stdin.flush());
        drop(stdin); // 关闭管道，远端 cat 才会收到 EOF
        result
    });

    let deadline = Instant::now() + UPLOAD_TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(format!("上传超时（>{}s）", UPLOAD_TIMEOUT.as_secs()));
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(e) => return Err(format!("等待 ssh 结束失败: {e}")),
        }
    };

    // stdin 线程的错误只在 ssh 也失败时才有诊断价值：远端提前关闭管道会让
    // write_all 报 BrokenPipe，但真正的原因在 ssh 的 stderr 里。
    let write_failed = matches!(writer.join(), Ok(Err(_)) | Err(_));

    if !status.success() {
        let mut stderr = String::new();
        if let Some(mut pipe) = child.stderr.take() {
            use std::io::Read;
            let _ = pipe.read_to_string(&mut stderr);
        }
        let detail = stderr.trim();
        let hint = if detail.contains("Permission denied") || detail.contains("publickey") {
            "（该远端不是免密登录，图片上传需要密钥或 ssh-agent）"
        } else {
            ""
        };
        return Err(format!(
            "上传到远端失败{hint}: {}",
            if detail.is_empty() { "ssh 未返回错误信息" } else { detail }
        ));
    }
    if write_failed {
        return Err("图片数据未能完整写入 ssh".into());
    }

    Ok(remote_path)
}

/// 把本地图片推到该 pane 所连的任意远端，返回远端 /tmp 路径。
#[tauri::command]
pub fn upload_image_over_ssh(
    state: tauri::State<'_, crate::pty::PtyManager>,
    pty_id: u32,
    local_path: String,
) -> Result<String, String> {
    let local = std::path::Path::new(&local_path);
    let remote_path = format!("{REMOTE_DIR}/{}", remote_file_name(local));
    let remote_command = remote_command_for(&remote_path);
    upload_image_to_remote(state.inner(), pty_id, &local_path, remote_path, remote_command)
}

/// 把图片放入 Code Sandbox 的 xclip bridge，供随后 Alt+V 生成原生图片附件。
///
/// bridge 缺失时命令会失败，调用方必须回退到通用的 /tmp 路径投递。
#[tauri::command]
pub fn upload_image_to_ssh_clipboard_bridge(
    state: tauri::State<'_, crate::pty::PtyManager>,
    pty_id: u32,
    local_path: String,
    bridges: Vec<crate::config::SshImageBridgeConfig>,
) -> Result<(), String> {
    let ssh_pid = crate::process_monitor::find_ssh_pid_for_pty(state.inner(), pty_id)
        .ok_or_else(|| "未找到该终端对应的 ssh 进程".to_string())?;
    let argv = process_argv(ssh_pid).ok_or_else(|| "无法读取 ssh 命令行".to_string())?;
    let destination = ssh_destination(&argv)
        .ok_or_else(|| "ssh 命令行里没有目标主机，无法匹配图片桥接规则".to_string())?;
    let bridge = bridges
        .iter()
        .find(|bridge| bridge.ssh_host.trim() == destination)
        .ok_or_else(|| format!("SSH 主机 `{destination}` 未配置原生图片桥接"))?;
    let (_, directory, prefix, probe_command) = validate_bridge_config(bridge)?;

    let local = std::path::Path::new(&local_path);
    let remote_path = format!("{directory}/{}", clipboard_bridge_file_name(local, &prefix));
    // probe_command 是用户在设置页主动配置的远端命令；只有它返回成功才接收图片。
    let remote_command = format!("({probe_command}) && cat > '{remote_path}'");
    upload_image_to_remote(state.inner(), pty_id, &local_path, remote_path, remote_command)
        .map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| s.to_string()).collect()
    }

    /// 用户实测形态：`ssh 2`，目标是一个 ~/.ssh/config 别名。
    /// 别名必须原样传回去，不能试图解析成 host/port。
    #[test]
    fn keeps_config_alias_as_destination() {
        let built = build_upload_argv(&argv(&["ssh", "2"]), "cat > /tmp/a.png").unwrap();

        assert_eq!(
            built,
            argv(&[
                "ssh",
                "-o",
                "BatchMode=yes",
                "-o",
                "ConnectTimeout=5",
                "2",
                "cat > /tmp/a.png",
            ])
        );
    }

    #[test]
    fn preserves_connection_flags_with_separate_values() {
        let built = build_upload_argv(
            &argv(&["ssh", "-p", "2222", "-i", "C:\\keys\\Id_Rsa", "user@host"]),
            "cat > /tmp/a.png",
        )
        .unwrap();

        // 大小写必须原样保留：identity 路径和别名都是大小写敏感的
        assert!(built.contains(&"C:\\keys\\Id_Rsa".to_string()));
        assert!(built.windows(2).any(|w| w == ["-p", "2222"]));
        assert_eq!(built[built.len() - 2], "user@host");
    }

    #[test]
    fn preserves_attached_flag_values() {
        let built =
            build_upload_argv(&argv(&["ssh", "-p2222", "host"]), "cmd").unwrap();

        assert!(built.windows(2).any(|w| w == ["-p", "2222"]));
    }

    #[test]
    fn drops_remote_command_from_original_argv() {
        let built = build_upload_argv(
            &argv(&["ssh", "host", "claude", "--resume"]),
            "cat > /tmp/a.png",
        )
        .unwrap();

        assert!(!built.contains(&"claude".to_string()));
        assert!(!built.contains(&"--resume".to_string()));
        assert_eq!(built.last().unwrap(), "cat > /tmp/a.png");
    }

    /// -t/-N/-f 会让"一条命令 + stdin 灌数据"直接失效，必须丢掉。
    #[test]
    fn drops_flags_that_break_piped_command() {
        let built =
            build_upload_argv(&argv(&["ssh", "-t", "-N", "-f", "host"]), "cmd").unwrap();

        assert!(!built.iter().any(|a| a == "-t" || a == "-N" || a == "-f"));
    }

    #[test]
    fn drops_bundled_tty_flags_but_keeps_others() {
        let built = build_upload_argv(&argv(&["ssh", "-tt", "-4C", "host"]), "cmd").unwrap();

        assert!(!built.iter().any(|a| a == "-tt" || a == "-t"));
        assert!(built.contains(&"-4C".to_string()));
    }

    #[test]
    fn drops_port_forwarding_together_with_its_value() {
        let built =
            build_upload_argv(&argv(&["ssh", "-L", "8080:localhost:80", "host"]), "cmd").unwrap();

        assert!(!built.contains(&"-L".to_string()));
        assert!(!built.contains(&"8080:localhost:80".to_string()));
    }

    #[test]
    fn keeps_jump_host() {
        let built =
            build_upload_argv(&argv(&["ssh", "-J", "bastion", "host"]), "cmd").unwrap();

        assert!(built.windows(2).any(|w| w == ["-J", "bastion"]));
    }

    #[test]
    fn returns_none_without_destination() {
        assert!(build_upload_argv(&argv(&["ssh"]), "cmd").is_none());
        assert!(build_upload_argv(&argv(&["ssh", "-p", "22"]), "cmd").is_none());
    }

    #[test]
    fn remote_file_name_sanitises_extension() {
        let name = remote_file_name(std::path::Path::new("/tmp/a.PNG"));
        assert!(name.ends_with(".png"));

        let weird = remote_file_name(std::path::Path::new("/tmp/a.p'n;g"));
        assert!(weird.ends_with(".png"));
        assert!(!weird.contains('\''));
        assert!(!weird.contains(';'));
    }

    #[test]
    fn clipboard_bridge_file_name_uses_configured_prefix() {
        let name = clipboard_bridge_file_name(std::path::Path::new("/tmp/a.PNG"), "clipboard-");

        assert!(name.starts_with("clipboard-miniterm-"));
        assert!(name.ends_with(".png"));
    }

    #[test]
    fn remote_command_writes_then_cleans_up() {
        let cmd = remote_command_for("/tmp/miniterm-1.png");

        assert!(cmd.starts_with("cat > '/tmp/miniterm-1.png'"));
        assert!(cmd.contains("-delete"));
    }

    #[test]
    fn bridge_config_substitutes_directory_in_probe() {
        let bridge = crate::config::SshImageBridgeConfig {
            id: "test".into(),
            name: "test".into(),
            ssh_host: "sandbox".into(),
            remote_directory: "/bridge/images".into(),
            probe_command: "test -x {directory}/bin/xclip".into(),
            file_prefix: "clipboard-".into(),
        };
        let (_, directory, prefix, probe) = validate_bridge_config(&bridge).unwrap();

        assert_eq!(directory, "/bridge/images");
        assert_eq!(prefix, "clipboard-");
        assert_eq!(probe, "test -x /bridge/images/bin/xclip");
    }

    #[test]
    fn ssh_destination_skips_connection_options() {
        let destination = ssh_destination(&argv(&["ssh", "-p", "2222", "-i", "key", "sandbox"]));

        assert_eq!(destination.as_deref(), Some("sandbox"));
    }
}
