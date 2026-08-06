# MiniTerm 远端 AI 对话状态监控设计

> 日期：2026-08-06
> 状态：开发依据

## 1. 背景与目标

MiniTerm 当前可以通过本机 HTTP Hook 接收 Claude Code、Codex、Gemini CLI 的事件，但 WSL、SSH 或堡垒机里的进程通常无法访问宿主机 `127.0.0.1`。因此远端会话会退化成终端输出和进程树推断，状态不够准确。

本版本增加一个轻量远端传输通道：Hook helper 在当前 TTY 输出 MiniTerm 私有 OSC 控制帧，经过 WSL、单跳/多跳 SSH 和堡垒机 PTY 到达本地 xterm；MiniTerm 解析后按当前 pane 绑定到状态仲裁器。目标是让状态监控在本地、WSL、SSH、堡垒机链路上使用同一套事件语义，并在 Agent 不可用时保持现有降级行为。

## 2. 非目标

- 不通过 HTTP 暴露远端端口，不要求反向端口转发。
- 不传输命令正文、对话正文、代码、图片或文件路径。
- 堡垒机不解析语义、不保存会话内容，只转发 PTY 字节流。
- 第一版不创建常驻远端 daemon；现有 `miniterm-hook` 同时承担 HTTP 和 OSC 输出。
- 不改变终端的普通文本输入、Bracketed Paste 或图片粘贴行为。

## 3. 总体架构

```text
AI CLI Hook
    |
    v
miniterm-hook
    |  本地有 MINITERM_HOOK_PORT -> HTTP POST
    |  否则 -> 当前 TTY 输出 OSC 777 帧
    v
WSL / SSH / 堡垒机 PTY（只转发字节）
    v
xterm parser（拦截并隐藏 OSC 777）
    v
Tauri report_remote_ai_status(当前 ptyId)
    v
统一 AiStatusArbiter -> pty-status-change -> 前端状态点
```

本地 HTTP 和远端 OSC 必须调用同一个 `apply_event`，保证两条链路的状态、序号、生命周期和 SessionEnd 行为一致。

## 4. OSC 协议

### 4.1 帧格式

使用私有 OSC 编号 777，推荐 BEL 结束，也接受 ST 结束：

```text
ESC ] 777 ; miniterm ; <base64url(JSON UTF-8)> BEL
ESC ] 777 ; miniterm ; <base64url(JSON UTF-8)> ESC \\
```

payload 上限 8 KiB（解码前后均检查）。JSON 字段：

```json
{
  "v": 1,
  "event": "PreToolUse",
  "status": "ai-thinking",
  "provider": "codex",
  "sessionId": "session-id",
  "seq": 42,
  "timestamp": 1786000000000
}
```

`event`、`status`、`provider` 使用白名单；`sessionId` 只允许有限长度的可打印字符。远端不提供 `ptyId`，本地解析 OSC 的 pane 注入真实 `ptyId`，防止跨 pane 冒充。

### 4.2 事件语义

- working：`UserPromptSubmit`、`PreToolUse`、`PostToolUse`、`BeforeAgent` 等 -> `ai-thinking`。
- completion：`Stop`、`AfterAgent`、`Notification` 等 -> `ai-complete`。
- permission：`PermissionRequest`、`Elicitation` -> `ai-awaiting-input`。
- `SessionEnd` -> `idle`；PTY、SSH 传输或本地 AI 进程明确退出 -> 清理状态。

### 4.3 去重与状态生命周期

每个 pane/session 维护单调 `seq`；重复或乱序帧丢弃。OSC 没有 `seq` 时使用时间戳兜底，旧版本地 HTTP payload 没有序号时按到达顺序处理。working 不使用静默超时，持续到 `Stop`、`PermissionRequest`、`Elicitation`、`SessionEnd` 或明确的进程/连接退出。permission 和 completion 保持到下一轮事件或 SessionEnd。SessionEnd 写入的 idle 保留 6 秒后再交还给启发式，避免旧输出立即恢复已结束的会话。

## 5. 状态仲裁

优先级固定为：

```text
SessionEnd / 进程退出
  > PermissionRequest / Elicitation
  > 有效 Hook working / complete
  > 屏幕 generating / awaiting-input 细分
  > 进程树存在性
  > idle
```

有效 Hook/OSC working 状态不得因长时间没有新事件而失效，也不得被 500ms 轮询覆盖为 complete。轮询只能把 `ai-thinking` 细分为 `ai-generating` / `ai-awaiting-input`，或在未安装 Agent、SessionEnd 的 idle 短租约结束后提供启发式状态。Hook complete 不应被 spinner 或旧输出重新改成 working。

宿主机确认本地 AI 子进程或 SSH 传输退出时，必须同时清理 AI 会话和 Hook 状态。WSL 或堡垒机后的远端 Agent 若无法从宿主进程树观察，则以 `SessionEnd` 和 pane 关闭作为终止信号；不能为了处理 Agent 崩溃而恢复固定 working 超时，否则会重新引入长任务提前完成的问题。

## 6. 跨平台安装与配置

- Windows、macOS、Linux 统一编译 `miniterm-hook`；检测 TTY 使用 `/dev/tty`（Unix）或当前标准输出（Windows ConPTY）。
- 有 `MINITERM_HOOK_PORT` 时优先 HTTP；端口不可用或未设置时自动选择 OSC。
- MiniTerm 提供“复制远端 Hook 配置”入口，输出 Claude/Codex/Gemini 的命令片段；用户只需把 helper 放入远端 `PATH`。
- Codex 使用当前正式配置键 `features.hooks = true`，兼容迁移旧的 `codex_hooks` 键时不删除用户其他配置。
- OSC 帧写入失败必须静默退出，不阻塞 AI CLI；本地 HTTP 行为保持兼容。

## 7. 安全、兼容与失败处理

- 严格校验版本、字段长度、枚举值和 Base64；非法帧只丢弃并记录 debug 日志。
- 不接受远端传来的路径、命令和正文，不开放监听端口。
- parser handler 返回 `true`，控制帧不进入屏幕和 scrollback；非 MiniTerm OSC 原样交给 xterm。
- 解析异常、版本不支持、Agent 未安装时保持现有终端输出推断。
- 每 pane 独立维护序号和状态生命周期，关闭 pane 时清理所有状态。

## 8. 实施范围

1. 新增 Rust OSC 编解码、白名单校验和 `HookState::apply_event` 仲裁入口。
2. 扩展 `miniterm-hook`：HTTP fast path + TTY OSC fallback。
3. 前端 xterm 注册 OSC 777 handler，并调用新的 Tauri command。
4. 修改 `process_monitor` 只在 Hook 状态无效时覆盖。
5. 修正 Codex feature 键和配置片段。
6. 添加单元/集成测试及远端配置说明。

## 9. 验收标准

- 本地 HTTP Hook 的已有测试和行为不回归。
- 在 WSL -> SSH -> 堡垒机 -> 远端 CLI 链路中，事件帧可被本地 pane 接收且不显示控制字符。
- working、permission、complete、SessionEnd 的优先级和生命周期符合第 5 节；working 静默超过 15 秒仍保持有效。
- 重复/乱序/非法 payload 被拒绝；无 Agent 时仍使用旧启发式。
- `node --test tests/*.test.cjs`、`cargo test --quiet`、前端生产构建均通过；Windows、macOS、Linux 目标可编译。
