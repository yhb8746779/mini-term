# 远端 AI 对话状态监控
> 更新时间: 2026-08-06 19:10
> 状态: 已完成

## 任务目标
按 `docs/superpowers/specs/2026-08-06-remote-ai-status-agent-design.md` 实现跨本地、WSL、SSH、堡垒机的 AI 会话状态监控，并保持无 Agent 时的终端启发式降级。

## 已完成
- [x] 盘点现有 Hook HTTP、PTY、进程轮询和前端状态链路。
- [x] 编写 OSC 777 协议、状态仲裁、跨平台安装和验收设计文档。
- [x] 实现 Rust OSC 编解码、字段白名单、大小限制和未知字段拒绝。
- [x] 扩展 `miniterm-hook`：本地 HTTP 优先，端口不可用时向当前 TTY 输出 OSC。
- [x] xterm 拦截并隐藏 OSC 帧，绑定当前 pane 的 `ptyId` 后上报 Tauri。
- [x] 实现 Hook/OSC 状态生命周期、序号去重、SessionEnd 清理和进程轮询仲裁。
- [x] 移除 working 的 15 秒租约，改为持续到明确 Hook 或进程/连接终止信号。
- [x] 修正 Codex `features.hooks = true`，增加远端 SSH/WSL 配置片段。
- [x] 修复 sidecar 构建脚本复用旧二进制的问题。
- [x] 版本统一升级到 `0.2.47`。
- [x] Rust 129 个测试、Node 2 个测试、前端生产构建全部通过。
- [x] 启动 Windows Tauri 测试版，新 helper 与目标产物 SHA-256 一致。

## 关键决策
- 使用 OSC 控制序列穿透 SSH/堡垒机 PTY，不开放远端 HTTP 端口。
- 远端不传 `ptyId`，由接收 OSC 的本地 pane 绑定。
- Hook/OSC 优先；working 不因静默超时降级，轮询只做状态细分和无 Agent 时的启发式判断。
- SessionEnd 保留短暂 idle 租约并清空 AI 会话，防止旧输出把状态恢复。
- 本地 AI 子进程、SSH 传输或 pane 明确退出时同步清除 sticky Hook；不可观测的 WSL/堡垒机链路依赖 SessionEnd。
- 远端 payload 只接受版本、事件、状态、provider、会话、序号和时间戳字段。

## 修改文件清单
- `docs/superpowers/specs/2026-08-06-remote-ai-status-agent-design.md` - 设计与验收标准。
- `src-tauri/src/remote_status.rs` - OSC 协议和校验。
- `src-tauri/src/bin/miniterm-hook.rs` - HTTP/OSC 双传输 helper。
- `src-tauri/src/hook_server.rs` - 统一事件入口和状态租约。
- `src-tauri/src/process_monitor.rs` - Hook 优先状态仲裁。
- `src/utils/terminalCache.ts` - xterm OSC handler。
- `src-tauri/src/hook_registry.rs` - provider 参数、Codex feature、远端配置片段。
- `src/components/SettingsModal.tsx`、`src/types.ts` - 远端配置 UI 和类型。
- `src-tauri/scripts/prepare-sidecar.mjs` - 强制增量重编 helper。
- `package.json`、`package-lock.json`、`src-tauri/Cargo.toml`、`src-tauri/Cargo.lock`、`src-tauri/tauri.conf.json` - 版本 0.2.47。
