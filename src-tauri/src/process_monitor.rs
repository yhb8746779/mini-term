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

/// AI 输出活跃超时阈值
const AI_ACTIVE_TIMEOUT: Duration = Duration::from_secs(3);

pub fn start_monitor(app: AppHandle, pty_manager: crate::pty::PtyManager) {
    thread::spawn(move || {
        let mut prev_statuses: HashMap<u32, String> = HashMap::new();

        loop {
            let pty_ids = pty_manager.get_pty_ids();

            for pty_id in &pty_ids {
                let status = if pty_manager.is_ai_session(*pty_id) {
                    if pty_manager.has_recent_output(*pty_id, AI_ACTIVE_TIMEOUT) {
                        "ai-working"
                    } else {
                        "ai-idle"
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
