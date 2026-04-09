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
}

/// AI 正在输出文本的判断阈值：距上次输出不超过此时间视为"生成中"
const AI_GENERATING_TIMEOUT: Duration = Duration::from_millis(1500);

pub fn start_monitor(app: AppHandle, pty_manager: crate::pty::PtyManager) {
    thread::spawn(move || {
        let mut prev_statuses: HashMap<u32, String> = HashMap::new();

        loop {
            let pty_ids = pty_manager.get_pty_ids();

            for pty_id in &pty_ids {
                let status = if pty_manager.is_ai_session(*pty_id) {
                    if pty_manager.has_recent_output(*pty_id, AI_GENERATING_TIMEOUT) {
                        // AI 正在输出文本 → 紫色慢闪
                        "ai-generating"
                    } else {
                        // AI 等待用户操作/授权 → 黄色快闪
                        "ai-working"
                    }
                } else {
                    "idle"
                };

                let prev = prev_statuses.get(pty_id);
                if prev.map(|s| s.as_str()) != Some(status) {
                    let _ = app.emit("pty-status-change", PtyStatusChangePayload {
                        pty_id: *pty_id,
                        status: status.to_string(),
                    });
                    prev_statuses.insert(*pty_id, status.to_string());
                }
            }

            prev_statuses.retain(|id, _| pty_ids.contains(id));

            let sleep_ms = if pty_ids.is_empty() { 2000 } else { 500 };
            thread::sleep(Duration::from_millis(sleep_ms));
        }
    });
}
