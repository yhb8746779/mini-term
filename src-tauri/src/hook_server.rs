//! Hook HTTP 服务器模块
//!
//! 在后台线程监听 `127.0.0.1` 的 HTTP 请求，接收 Claude Code / Codex 的
//! hook 事件上报，并通过 Tauri event 通知前端。

use crate::process_monitor::PtyStatusChangePayload;
use crate::remote_status::{is_allowed_provider, is_allowed_status, RemoteStatusPayload};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager};

/// 默认监听端口
const DEFAULT_PORT: u16 = 23456;
/// 端口冲突时最多尝试的端口数
const MAX_PORT_ATTEMPTS: u16 = 5;
const HOOK_IDLE_LEASE: Duration = Duration::from_secs(6);
/// Hook 事件的 JSON payload
#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)] // 保留完整字段供未来 UI 细化使用
pub struct HookPayload {
    /// PTY ID（由 MINITERM_PTY_ID 环境变量传递）
    pub pty_id: Option<u32>,
    /// 事件名（如 UserPromptSubmit, PreToolUse 等）
    pub event: Option<String>,
    /// 来源 agent（claude-code / codex）
    pub agent: Option<String>,
    /// 会话 ID
    pub session_id: Option<String>,
    /// 工作目录
    pub cwd: Option<String>,
    /// 工具名称（PreToolUse/PostToolUse 时有值）
    pub tool_name: Option<String>,
}

/// Hook 状态信息，供前端查询
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HookStatusInfo {
    pub port: u16,
    pub running: bool,
}

/// Hook 状态管理器，记录每个 PTY 的最后 hook 状态
#[derive(Clone)]
pub struct HookState {
    last_hook_status: Arc<Mutex<HashMap<u32, String>>>,
    last_hook_provider: Arc<Mutex<HashMap<u32, String>>>,
    hook_seq: Arc<Mutex<HashMap<u32, u64>>>,
    hook_session: Arc<Mutex<HashMap<u32, String>>>,
    hook_idle_until: Arc<Mutex<HashMap<u32, Instant>>>,
    port: Arc<Mutex<u16>>,
    /// 保存 server 实例，供运行时停止（Arc 共享给监听线程）
    server: Arc<Mutex<Option<Arc<tiny_http::Server>>>>,
}

impl HookState {
    pub fn new() -> Self {
        Self {
            last_hook_status: Arc::new(Mutex::new(HashMap::new())),
            last_hook_provider: Arc::new(Mutex::new(HashMap::new())),
            hook_seq: Arc::new(Mutex::new(HashMap::new())),
            hook_session: Arc::new(Mutex::new(HashMap::new())),
            hook_idle_until: Arc::new(Mutex::new(HashMap::new())),
            port: Arc::new(Mutex::new(0)),
            server: Arc::new(Mutex::new(None)),
        }
    }

    /// 获取当前有效状态。working/permission/complete 由后续明确事件覆盖，只有
    /// SessionEnd 写入的 idle 会在短租约后交还给终端启发式。
    pub fn get_effective_status(&self, pty_id: u32) -> Option<(String, Option<String>)> {
        self.get_effective_status_at(pty_id, Instant::now())
    }

    fn get_effective_status_at(
        &self,
        pty_id: u32,
        now: Instant,
    ) -> Option<(String, Option<String>)> {
        let status = self
            .last_hook_status
            .lock()
            .unwrap()
            .get(&pty_id)
            .cloned()?;
        if status == "idle" {
            let until = self.hook_idle_until.lock().unwrap().get(&pty_id).copied()?;
            if now > until {
                return None;
            }
        }
        let provider = self
            .last_hook_provider
            .lock()
            .unwrap()
            .get(&pty_id)
            .cloned();
        Some((status, provider))
    }

    /// 应用本地 HTTP 或远端 OSC 事件。返回 false 表示序号重复/乱序或字段非法。
    pub fn apply_event(
        &self,
        pty_id: u32,
        status: String,
        provider: Option<String>,
        session_id: Option<String>,
        seq: Option<u64>,
    ) -> bool {
        if !is_allowed_status(&status) {
            return false;
        }
        if let Some(ref value) = provider {
            if !is_allowed_provider(value) {
                return false;
            }
        }
        let previous_session = self.hook_session.lock().unwrap().get(&pty_id).cloned();
        let same_session =
            session_id.is_none() || previous_session.as_deref() == session_id.as_deref();
        if same_session {
            if let (Some(new_seq), Some(old_seq)) =
                (seq, self.hook_seq.lock().unwrap().get(&pty_id).copied())
            {
                if new_seq <= old_seq {
                    return false;
                }
            }
        } else {
            self.hook_seq.lock().unwrap().remove(&pty_id);
        }
        self.last_hook_status
            .lock()
            .unwrap()
            .insert(pty_id, status.clone());
        if status == "idle" {
            self.last_hook_provider.lock().unwrap().remove(&pty_id);
        } else if let Some(provider) = provider {
            self.last_hook_provider
                .lock()
                .unwrap()
                .insert(pty_id, provider);
        }
        if let Some(session_id) = session_id {
            self.hook_session.lock().unwrap().insert(pty_id, session_id);
        }
        if let Some(seq) = seq {
            self.hook_seq.lock().unwrap().insert(pty_id, seq);
        }
        if status == "idle" {
            self.hook_idle_until
                .lock()
                .unwrap()
                .insert(pty_id, Instant::now() + HOOK_IDLE_LEASE);
        } else {
            self.hook_idle_until.lock().unwrap().remove(&pty_id);
        }
        true
    }

    /// 更新本地 Hook 事件（旧 HTTP payload 没有序号时仍保持兼容）。
    fn update(
        &self,
        pty_id: u32,
        status: String,
        provider: Option<String>,
        session_id: Option<String>,
    ) -> bool {
        self.apply_event(pty_id, status, provider, session_id, None)
    }

    /// 移除指定 PTY 的 hook 状态。PTY、SSH 传输或本地 AI 进程明确退出时调用。
    /// SessionEnd 不调用此方法，而是保留短暂 idle，避免旧输出恢复会话。
    pub fn remove(&self, pty_id: u32) {
        self.last_hook_status.lock().unwrap().remove(&pty_id);
        self.last_hook_provider.lock().unwrap().remove(&pty_id);
        self.hook_seq.lock().unwrap().remove(&pty_id);
        self.hook_session.lock().unwrap().remove(&pty_id);
        self.hook_idle_until.lock().unwrap().remove(&pty_id);
    }

    /// 获取当前服务器端口
    pub fn get_port(&self) -> u16 {
        *self.port.lock().unwrap()
    }

    /// 设置服务器端口
    fn set_port(&self, port: u16) {
        *self.port.lock().unwrap() = port;
    }

    /// 保存 server 实例
    fn set_server(&self, server: Option<Arc<tiny_http::Server>>) {
        *self.server.lock().unwrap() = server;
    }

    /// 检查 server 是否正在运行
    pub fn is_server_running(&self) -> bool {
        self.server.lock().unwrap().is_some()
    }
}

/// 将 hook 事件名映射为本地 PTY 状态。
///
/// 本地 PaneStatus 用三层动画命名（ai-thinking / ai-generating / ai-complete / ai-awaiting-input）。
/// Hook 只能告诉我们"AI 开始处理"或"AI 已停止"，无法区分 thinking vs generating（需要 spinner + token 流），
/// 也无法判断 awaiting-input（需要屏幕文本），所以：
/// - working 类事件 → ai-thinking（保守起点，process_monitor 会用启发式细化为 generating/awaiting-input）
/// - 仅主会话明确完成事件 → ai-complete
/// - SessionEnd 单独处理（清除 hook 状态），不在此映射
///
/// 支持三家：
/// - Claude Code: SessionStart, UserPromptSubmit, PreToolUse, PostToolUse, Stop, SubagentStart/Stop,
///                PreCompact, PostCompact, PermissionRequest, Notification, Elicitation
/// - Codex:       SessionStart, UserPromptSubmit, PreToolUse, PostToolUse, Stop, PermissionRequest
/// - Gemini CLI:  SessionStart, BeforeAgent, BeforeToolSelection, BeforeTool, AfterModel, AfterAgent
fn map_event_to_status(event: &str) -> Option<&'static str> {
    match event {
        // AI 正在积极工作（thinking 是保守起点，启发式会升级到 generating）
        "UserPromptSubmit" | "PreToolUse" | "PostToolUse" | "SubagentStart" | "PreCompact"
        | "PostCompact"
        // Gemini 事件：AI 进入处理流程
        | "BeforeAgent" | "BeforeToolSelection" | "BeforeTool" | "AfterModel" => {
            Some("ai-thinking")
        }
        // 仅主会话明确结束事件可以覆盖 working。Claude 的 Notification、
        // SubagentStop 只表示一个中间节点结束，主 Agent 可能仍在工作。
        "Stop" | "AfterAgent" => Some("ai-complete"),
        // SessionStart 只表示 CLI 会话建立，不代表一轮回答已经完成。
        // 不改变当前 pane 状态，避免启动/恢复时序覆盖正在工作的状态。
        "SessionStart" => None,
        "PermissionRequest" | "Elicitation" => Some("ai-awaiting-input"),
        _ => None,
    }
}

fn normalize_provider(agent: Option<&str>) -> Option<String> {
    match agent? {
        "claude-code" | "claude" => Some("claude".into()),
        "codex" => Some("codex".into()),
        "gemini" | "gemini-cli" => Some("gemini".into()),
        "grok" => Some("grok".into()),
        _ => None,
    }
}

fn provider_static(provider: &str) -> &'static str {
    match provider {
        "claude" => "claude",
        "codex" => "codex",
        "gemini" => "gemini",
        "grok" => "grok",
        _ => "unknown",
    }
}

fn emit_status(app: &AppHandle, pty_id: u32, status: String, provider: Option<String>) {
    let _ = app.emit(
        "pty-status-change",
        PtyStatusChangePayload {
            pty_id,
            status,
            provider,
        },
    );
}

/// xterm OSC 777 handler 的可信入口。pty_id 来自本地 pane，不接受远端指定。
#[tauri::command]
pub fn report_remote_ai_status(
    app: AppHandle,
    hook_state: tauri::State<'_, HookState>,
    pty_manager: tauri::State<'_, crate::pty::PtyManager>,
    pty_id: u32,
    payload: RemoteStatusPayload,
) -> Result<bool, String> {
    payload.validate().map_err(ToString::to_string)?;
    // 仅记录协议元数据，用于排查远端 Hook 与本地状态仲裁的时序；不记录原始
    // Hook payload、终端正文、cwd 或任何命令内容。
    let event = payload.event.clone();
    let reported_status = payload.status.clone();
    let sequence = payload.seq.or(payload.timestamp);
    if payload.event == "SessionEnd" || payload.status == "idle" {
        let accepted = hook_state.apply_event(
            pty_id,
            "idle".into(),
            None,
            payload.session_id,
            sequence,
        );
        if accepted {
            pty_manager.clear_ai_session(pty_id);
            emit_status(&app, pty_id, "idle".into(), None);
        }
        eprintln!(
            "[remote-status] pty_id={} event={} status={} seq={:?} accepted={}",
            pty_id, event, reported_status, sequence, accepted
        );
        return Ok(accepted);
    }
    let accepted = hook_state.apply_event(
        pty_id,
        payload.status.clone(),
        Some(payload.provider.clone()),
        payload.session_id,
        sequence,
    );
    if accepted {
        pty_manager.force_ai_session(pty_id, provider_static(&payload.provider));
        emit_status(&app, pty_id, payload.status, Some(payload.provider));
    }
    eprintln!(
        "[remote-status] pty_id={} event={} status={} seq={:?} accepted={}",
        pty_id, event, reported_status, sequence, accepted
    );
    Ok(accepted)
}

/// 启动 hook HTTP 服务器
///
/// 在后台线程监听，接收 hook 事件后通过 Tauri event 通知前端。
/// 端口从 DEFAULT_PORT 开始尝试，冲突时自动递增。
/// 返回 `Err` 表示无法绑定端口，调用方应将错误提示给用户。
pub fn start_hook_server(app: AppHandle, hook_state: HookState) -> Result<(), String> {
    // 如果已经在运行，不重复启动
    if hook_state.is_server_running() {
        eprintln!("[hook-server] 服务器已在运行，跳过启动");
        return Ok(());
    }

    // 在当前线程绑定端口，以便同步获取 server 实例
    let bound = {
        let mut result = None;
        for offset in 0..MAX_PORT_ATTEMPTS {
            let port = DEFAULT_PORT + offset;
            let addr = format!("127.0.0.1:{}", port);
            match tiny_http::Server::http(&addr) {
                Ok(s) => {
                    eprintln!("[hook-server] 监听 {}", addr);
                    hook_state.set_port(port);
                    result = Some((s, port));
                    break;
                }
                Err(e) => {
                    eprintln!("[hook-server] 端口 {} 被占用: {}", port, e);
                }
            }
        }
        result
    };

    let (server, port) = match bound {
        Some(s) => s,
        None => {
            eprintln!("[hook-server] 无法绑定任何端口，hook 服务器未启动");
            return Err("无法绑定端口 (23456-23460)，hook 服务器启动失败".to_string());
        }
    };

    // 用 Arc 包装 server，共享给 HookState 和监听线程
    let server = Arc::new(server);
    hook_state.set_server(Some(server.clone()));

    // 写入端口文件
    write_port_file(&app, port);

    std::thread::spawn(move || {
        // 处理请求
        for mut request in server.incoming_requests() {
            if request.method() != &tiny_http::Method::Post {
                let response =
                    tiny_http::Response::from_string("Method Not Allowed").with_status_code(405);
                let _ = request.respond(response);
                continue;
            }

            let url = request.url().to_string();
            if url != "/hook" {
                let response = tiny_http::Response::from_string("Not Found").with_status_code(404);
                let _ = request.respond(response);
                continue;
            }

            // 读取 body
            let mut body = String::new();
            if request.as_reader().read_to_string(&mut body).is_err() {
                let response =
                    tiny_http::Response::from_string("Bad Request").with_status_code(400);
                let _ = request.respond(response);
                continue;
            }

            // 解析 JSON payload
            let payload: HookPayload = match serde_json::from_str(&body) {
                Ok(p) => p,
                Err(e) => {
                    eprintln!("[hook-server] JSON 解析失败: {}", e);
                    let response =
                        tiny_http::Response::from_string("Bad Request").with_status_code(400);
                    let _ = request.respond(response);
                    continue;
                }
            };

            // 立即响应 200，不阻塞 hook 脚本
            let response = tiny_http::Response::from_string("OK").with_status_code(200);
            let _ = request.respond(response);

            // 处理事件
            if let (Some(pty_id), Some(ref event)) = (payload.pty_id, &payload.event) {
                if event == "SessionEnd" {
                    // 会话结束保留 idle 终态，下一轮轮询不能重新覆盖。
                    hook_state.apply_event(
                        pty_id,
                        "idle".into(),
                        None,
                        payload.session_id.clone(),
                        None,
                    );
                    if let Some(pty_manager) = app.try_state::<crate::pty::PtyManager>() {
                        pty_manager.clear_ai_session(pty_id);
                    }
                    emit_status(&app, pty_id, "idle".into(), None);
                    eprintln!("[hook-server] pty_id={} event=SessionEnd -> idle", pty_id);
                } else if let Some(status) = map_event_to_status(event) {
                    let provider = normalize_provider(payload.agent.as_deref());
                    let accepted = hook_state.update(
                        pty_id,
                        status.to_string(),
                        provider.clone(),
                        payload.session_id.clone(),
                    );
                    if !accepted {
                        continue;
                    }
                    if let (Some(provider), Some(pty_manager)) = (
                        provider.as_deref(),
                        app.try_state::<crate::pty::PtyManager>(),
                    ) {
                        pty_manager.force_ai_session(pty_id, provider_static(provider));
                    }

                    // 通过 Tauri event 通知前端（复用现有 pty-status-change 事件）
                    let _ = app.emit(
                        "pty-status-change",
                        PtyStatusChangePayload {
                            pty_id,
                            status: status.to_string(),
                            provider,
                        },
                    );

                    eprintln!(
                        "[hook-server] pty_id={} event={} -> status={}",
                        pty_id, event, status
                    );
                }
            }
        }
    });

    Ok(())
}

/// 停止 hook HTTP 服务器
///
/// 取出保存的 server 实例，调用 `unblock()` 中断阻塞循环，
/// 清理端口文件并重置端口。
pub fn stop_hook_server(hook_state: &HookState, app: &AppHandle) {
    let server = hook_state.server.lock().unwrap().take();
    if let Some(s) = server {
        s.unblock();
        eprintln!("[hook-server] 服务器已停止");
    }
    hook_state.set_port(0);
    // 清理端口文件
    delete_port_file(app);
}

/// 运行时切换 hook server 开关
#[tauri::command]
pub fn toggle_hook_server(
    app: AppHandle,
    hook_state: tauri::State<'_, HookState>,
    enabled: bool,
) -> Result<(), String> {
    if enabled {
        if !hook_state.is_server_running() {
            start_hook_server(app, hook_state.inner().clone())?;
        }
    } else if hook_state.is_server_running() {
        stop_hook_server(hook_state.inner(), &app);
    }
    Ok(())
}

/// 将端口信息写入 app_data_dir/hook-server.json
fn write_port_file(app: &AppHandle, port: u16) {
    if let Ok(dir) = app.path().app_data_dir() {
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("hook-server.json");
        let content = format!("{{\"port\":{}}}", port);
        if let Err(e) = std::fs::write(&path, &content) {
            eprintln!("[hook-server] 写入端口文件失败 {}: {}", path.display(), e);
        } else {
            eprintln!("[hook-server] 端口文件已写入 {}", path.display());
        }
    }
}

/// 删除端口文件 app_data_dir/hook-server.json
fn delete_port_file(app: &AppHandle) {
    if let Ok(dir) = app.path().app_data_dir() {
        let path = dir.join("hook-server.json");
        if path.exists() {
            if let Err(e) = std::fs::remove_file(&path) {
                eprintln!("[hook-server] 删除端口文件失败 {}: {}", path.display(), e);
            } else {
                eprintln!("[hook-server] 端口文件已删除 {}", path.display());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_duplicate_and_out_of_order_remote_events() {
        let state = HookState::new();
        assert!(state.apply_event(
            7,
            "ai-thinking".into(),
            Some("codex".into()),
            Some("s".into()),
            Some(10)
        ));
        assert!(!state.apply_event(
            7,
            "ai-generating".into(),
            Some("codex".into()),
            Some("s".into()),
            Some(10)
        ));
        assert!(!state.apply_event(
            7,
            "ai-generating".into(),
            Some("codex".into()),
            Some("s".into()),
            Some(9)
        ));
        assert_eq!(
            state.get_effective_status(7).map(|v| v.0),
            Some("ai-thinking".into())
        );
    }

    #[test]
    fn a_new_session_resets_sequence() {
        let state = HookState::new();
        assert!(state.apply_event(
            8,
            "ai-thinking".into(),
            Some("claude".into()),
            Some("old".into()),
            Some(99)
        ));
        assert!(state.apply_event(
            8,
            "ai-thinking".into(),
            Some("claude".into()),
            Some("new".into()),
            Some(1)
        ));
        assert_eq!(
            state.get_effective_status(8).map(|v| v.0),
            Some("ai-thinking".into())
        );
    }

    #[test]
    fn session_end_keeps_a_short_idle_lease() {
        let state = HookState::new();
        assert!(state.apply_event(
            9,
            "ai-thinking".into(),
            Some("gemini".into()),
            None,
            Some(1)
        ));
        assert!(state.apply_event(9, "idle".into(), None, None, Some(2)));
        assert_eq!(state.get_effective_status(9), Some(("idle".into(), None)));
        assert_eq!(
            state.get_effective_status_at(
                9,
                Instant::now() + HOOK_IDLE_LEASE + Duration::from_secs(1)
            ),
            None
        );
    }

    #[test]
    fn working_hook_stays_effective_until_an_explicit_event() {
        let state = HookState::new();
        assert!(state.apply_event(
            10,
            "ai-thinking".into(),
            Some("codex".into()),
            None,
            Some(1)
        ));
        assert_eq!(
            state.get_effective_status_at(10, Instant::now() + Duration::from_secs(24 * 60 * 60)),
            Some(("ai-thinking".into(), Some("codex".into())))
        );
    }

    #[test]
    fn stop_overrides_sticky_working_status() {
        let state = HookState::new();
        assert!(state.apply_event(
            11,
            "ai-thinking".into(),
            Some("claude".into()),
            Some("session".into()),
            Some(1)
        ));
        assert!(state.apply_event(
            11,
            "ai-complete".into(),
            Some("claude".into()),
            Some("session".into()),
            Some(2)
        ));
        assert_eq!(
            state.get_effective_status(11),
            Some(("ai-complete".into(), Some("claude".into())))
        );
    }

    #[test]
    fn permission_request_overrides_sticky_working_status() {
        let state = HookState::new();
        assert!(state.apply_event(
            12,
            "ai-thinking".into(),
            Some("codex".into()),
            Some("session".into()),
            Some(1)
        ));
        assert!(state.apply_event(
            12,
            "ai-awaiting-input".into(),
            Some("codex".into()),
            Some("session".into()),
            Some(2)
        ));
        assert_eq!(
            state.get_effective_status(12),
            Some(("ai-awaiting-input".into(), Some("codex".into())))
        );
    }

    #[test]
    fn removing_a_pty_clears_sticky_status() {
        let state = HookState::new();
        assert!(state.apply_event(
            13,
            "ai-generating".into(),
            Some("gemini".into()),
            None,
            Some(1)
        ));
        state.remove(13);
        assert_eq!(state.get_effective_status(13), None);
    }

    #[test]
    fn intermediate_claude_events_do_not_mark_the_main_session_complete() {
        assert_eq!(map_event_to_status("Notification"), None);
        assert_eq!(map_event_to_status("SubagentStop"), None);
        assert_eq!(map_event_to_status("Stop"), Some("ai-complete"));
        assert_eq!(map_event_to_status("SessionStart"), None);
    }
}
