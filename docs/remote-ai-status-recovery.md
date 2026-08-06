# 远端 AI 状态监控重建指南

## 适用范围

本指南用于重建 Linux、WSL、Code Sandbox 或远端 macOS 中的 MiniTerm AI 状态上报。

状态链路为：

```text
AI CLI Hook -> miniterm-hook -> 当前 SSH TTY 的 OSC 777 -> MiniTerm 当前 pane
```

该链路不使用端口转发、Host Bridge 或 `code_sandbox_host`。远端帧只包含协议版本、事件、状态、provider、可选 session ID 与时序字段；不包含提示词、命令、cwd、终端正文、文件内容或宿主机信息。

## 运行前提

- 在 MiniTerm 中创建的交互式 SSH pane，而不是无 TTY 的批处理 SSH。
- 远端有 MiniTerm 对应版本源码，或有该版本同架构的 `miniterm-hook` 二进制。
- Hook 子进程应继承 `SSH_TTY`。Claude Code 常会捕获 Hook stdout，因此不能依赖 stdout 返回 OSC。

## Linux、WSL 与 Code Sandbox 重建

1. 找到 MiniTerm 源码。优先使用用户给出的路径；Code Sandbox 的典型映射路径为 `/workspace/h-workspace/self/mini-term`。不要扫描整个 home 或宿主机路径。
2. 仅构建独立 helper crate，而非完整 Tauri 应用：

```bash
workspace=/workspace/h-workspace/self/mini-term
export CARGO_HOME="$workspace/.cache/miniterm-hook/cargo-home"
export CARGO_TARGET_DIR="$workspace/.cache/miniterm-hook/target"
cd "$workspace"
cargo build --release --manifest-path src-tauri/miniterm-hook/Cargo.toml
sudo install -m 0755 "$CARGO_TARGET_DIR/release/miniterm-hook" /usr/local/bin/miniterm-hook
```

3. 将远端模板中的各段分别合并到 `~/.claude/settings.json`、`~/.codex/hooks.json`、`~/.codex/config.toml`、`~/.gemini/settings.json`。不要把四段模板粘入同一个文件。
4. 远端 Hook 命令必须包含：

```text
MINITERM_REMOTE_OSC=1 /usr/local/bin/miniterm-hook <Event> <provider>
```

5. Codex 的既有 `[features]` 段只增加 `hooks = true`，不要创建第二个 `[features]`。
6. 新开一次 AI CLI 会话后验证。已有会话不会回放此前的 Hook 事件。

首次缺少构建环境时，在用户确认后安装 Linux 的 `build-essential pkg-config libssl-dev` 与 Rust。Cargo 下载和目标产物应放在 workspace 的 `.cache/miniterm-hook/`，避免每次重建重复下载。

## macOS 远端

远端 macOS 必须构建或安装 macOS 对应架构的 Mach-O helper，不能复制 Windows `.exe` 或 Linux ELF。需要 Xcode Command Line Tools 与 Rust toolchain。安装路径同样使用 `/usr/local/bin/miniterm-hook`。

SSH 在 macOS 通常设置 `SSH_TTY=/dev/ttys###`。helper 会安全接受 `/dev/ttys<数字>`、Linux 的 `/dev/pts/<数字>` 与 `/dev/tty`，并优先把 OSC 写入这些 TTY；这避免 Claude 捕获 stdout 后状态帧无法回到 MiniTerm。

## 多 pane 与重启行为

- helper 与 Hook 配置是远端用户级资源，多个项目和多个 SSH pane 共用。
- 每个交互式 SSH pane 有自己的 `SSH_TTY`，MiniTerm 以本地 pane 的 `ptyId` 绑定状态，因此 pane 之间不会串状态。
- tmux/screen 中多个逻辑 pane 仍只对应一个外层 MiniTerm pane。需要独立状态时，使用独立 SSH pane。
- 普通重开 MiniTerm、电脑重启、新开项目或新对话不需要重装。完整重建沙盒、删除远端 home 配置、替换 helper 或切换 CPU/操作系统后，重新执行本指南。

## 验收

在真实 MiniTerm SSH pane 中运行超过 30 秒的任务，并在运行时切换窗口。它必须一直保持 working；结束后必须由真实 Hook 的 `Stop` 切为 DONE。开发日志应依次有类似：

```text
[remote-status] ... event=UserPromptSubmit ... accepted=true
[remote-status] ... event=PreToolUse ... accepted=true
[remote-status] ... event=Stop status=ai-complete ... accepted=true
[status-arbiter] ... source=hook status=ai-complete
```

`Sautéed`、`Brewed` 等终端文本不是完成协议，不能作为 DONE 判断依据。

## 排错

若日志只有 `source=heuristic`、没有 `[remote-status]`：

1. 检查 Hook 配置是否仍含 `MINITERM_REMOTE_OSC=1` 与 `/usr/local/bin/miniterm-hook`。
2. 确认 helper 是当前源码构建的同平台版本。
3. 确认使用交互式 SSH pane，并检查 Hook 子进程继承 `SSH_TTY`。
4. 仅短期开启 `MINITERM_HOOK_DEBUG=1`。它只能记录事件名、stdin 是否到达、是否有 session ID、TTY 写入结果；诊断完成后立即移除该变量并删除日志。

不要以新增状态文案词表、端口映射或读取 Host Bridge 替代上述 PTY 通道。
