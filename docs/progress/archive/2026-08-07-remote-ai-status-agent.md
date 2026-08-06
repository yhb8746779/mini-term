# 远端 AI 状态监控修复

> 更新时间: 2026-08-07
> 状态: 已完成

## 已完成

- [x] 定位远端 OSC 被前端丢弃的原因：`sessionId: null` 不符合旧解析器的字符串校验。
- [x] 前端协议解析允许缺失或 `null` 的 sessionId；Rust 序列化在 session ID 缺失时省略该字段。
- [x] 验证沙盒 Claude 会话元数据存在 `sessionId` 字段，未读取或记录任何会话正文。
- [x] 验证 helper 能把输入中的 `session_id` 原样写入 OSC 777 payload。
- [x] 运行远端状态协议测试和独立 helper 测试，均通过。
- [x] 更新沙盒 `/usr/local/bin/miniterm-hook` 到当前源码构建的 Linux 版本；缺失会话 ID 时已确认省略 `sessionId` 字段。
- [x] 为一次性链路排查部署受限 helper 诊断：仅记录 Hook 事件、stdin 是否到达与 TTY 写入结果。
- [x] 根因确认：Hook 有 stdin 和真实 session ID，但无 controlling tty；OSC 曾写到被 Claude 捕获的 stdout。
- [x] helper 改为优先写受限的 `MINITERM_TTY` / `SSH_TTY` / `TTY`（仅 `/dev/tty` 或 `/dev/pts/*`），并已重新安装到沙盒。
- [x] 在实际 MiniTerm SSH pane 验证：`UserPromptSubmit` 与 `Stop` 均以 OSC 到达，`Stop` 被接受并将 pane 仲裁为 `source=hook` 的 `ai-complete`。
- [x] 已移除全部 13 条 Hook 命令中的临时 `MINITERM_HOOK_DEBUG=1`，并清理沙盒诊断日志。

## 待完成

- [x] 用超过 20 秒的真实任务复验：`pty_id=11` 在约 118 秒后收到真实 `Stop`，并以 `source=hook` 切为 `ai-complete`。

## 关键决策

- 优先使用 Claude Hook stdin 提供的真实 `session_id` / `sessionId`，不为每个 Hook 随机生成 ID。
- ID 缺失时省略字段即可：本地以 MiniTerm pane 的 `ptyId` 关联，使用 seq/timestamp 做同 pane 时序保护。
- OSC 只携带协议状态元数据，绝不携带 Hook 原文、cwd、命令、提示词或文件内容。
