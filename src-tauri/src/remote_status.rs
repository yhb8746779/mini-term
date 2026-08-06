//! MiniTerm 远端 AI 状态的 OSC 777 协议。
//!
//! 该模块只处理纯数据，不依赖 Tauri，供桌面端 parser 和 `miniterm-hook`
//! 共享。远端只发送有限的状态元数据，不携带终端正文或文件路径。

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use serde::{Deserialize, Serialize};

// 同一源码同时编进桌面端和轻量 helper，各 target 只使用其中一半 API。
#[allow(dead_code)]
pub const OSC_NUMBER: u16 = 777;
#[allow(dead_code)]
pub const OSC_PREFIX: &str = "miniterm;";
#[allow(dead_code)]
pub const MAX_PAYLOAD_BYTES: usize = 8 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RemoteStatusPayload {
    pub v: u8,
    pub event: String,
    pub status: String,
    pub provider: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    pub seq: Option<u64>,
    pub timestamp: Option<u64>,
}

impl RemoteStatusPayload {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.v != 1 {
            return Err("unsupported protocol version");
        }
        if !is_allowed_event(&self.event) {
            return Err("unsupported event");
        }
        if !is_allowed_status(&self.status) {
            return Err("unsupported status");
        }
        if !is_allowed_provider(&self.provider) {
            return Err("unsupported provider");
        }
        if let Some(session_id) = &self.session_id {
            if session_id.is_empty()
                || session_id.len() > 256
                || !session_id.bytes().all(|b| b.is_ascii_graphic())
            {
                return Err("invalid session id");
            }
        }
        Ok(())
    }
}

pub fn is_allowed_event(value: &str) -> bool {
    matches!(
        value,
        "SessionStart"
            | "SessionEnd"
            | "UserPromptSubmit"
            | "PreToolUse"
            | "PostToolUse"
            | "Stop"
            | "PermissionRequest"
            | "Elicitation"
            | "Notification"
            | "SubagentStart"
            | "SubagentStop"
            | "PreCompact"
            | "PostCompact"
            | "BeforeAgent"
            | "BeforeToolSelection"
            | "BeforeTool"
            | "AfterModel"
            | "AfterAgent"
    )
}

pub fn is_allowed_status(value: &str) -> bool {
    matches!(
        value,
        "idle" | "ai-complete" | "ai-thinking" | "ai-generating" | "ai-awaiting-input"
    )
}

pub fn is_allowed_provider(value: &str) -> bool {
    matches!(value, "claude" | "codex" | "gemini" | "grok")
}

#[allow(dead_code)]
pub fn encode_osc_status(payload: &RemoteStatusPayload) -> Result<String, &'static str> {
    payload.validate()?;
    let json = serde_json::to_vec(payload).map_err(|_| "serialize payload failed")?;
    if json.len() > MAX_PAYLOAD_BYTES {
        return Err("payload too large");
    }
    let encoded = URL_SAFE_NO_PAD.encode(json);
    Ok(format!("\x1b]{};{}{}\x07", OSC_NUMBER, OSC_PREFIX, encoded))
}

/// 解码 xterm OSC handler 收到的 `miniterm;<base64url>` 数据。
#[allow(dead_code)]
pub fn decode_osc_data(data: &str) -> Result<RemoteStatusPayload, &'static str> {
    let encoded = data.strip_prefix(OSC_PREFIX).ok_or("invalid osc prefix")?;
    if encoded.is_empty() || encoded.len() > MAX_PAYLOAD_BYTES * 2 {
        return Err("encoded payload too large");
    }
    let bytes = URL_SAFE_NO_PAD
        .decode(encoded)
        .map_err(|_| "invalid base64")?;
    if bytes.len() > MAX_PAYLOAD_BYTES {
        return Err("payload too large");
    }
    let payload: RemoteStatusPayload =
        serde_json::from_slice(&bytes).map_err(|_| "invalid json")?;
    payload.validate()?;
    Ok(payload)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn payload(seq: u64) -> RemoteStatusPayload {
        RemoteStatusPayload {
            v: 1,
            event: "PreToolUse".into(),
            status: "ai-thinking".into(),
            provider: "codex".into(),
            session_id: Some("s1".into()),
            seq: Some(seq),
            timestamp: Some(1),
        }
    }

    #[test]
    fn osc_round_trip() {
        let frame = encode_osc_status(&payload(4)).expect("frame");
        assert!(frame.starts_with("\x1b]777;miniterm;"));
        let data = frame
            .strip_prefix("\x1b]777;")
            .and_then(|v| v.strip_suffix('\x07'))
            .expect("osc data");
        assert_eq!(decode_osc_data(data).expect("payload"), payload(4));
    }

    #[test]
    fn rejects_unknown_provider_and_version() {
        let mut p = payload(1);
        p.provider = "evil".into();
        assert!(encode_osc_status(&p).is_err());
        let mut p = payload(1);
        p.v = 2;
        assert!(encode_osc_status(&p).is_err());
    }

    #[test]
    fn rejects_unknown_json_fields() {
        let raw = br#"{"v":1,"event":"PreToolUse","status":"ai-thinking","provider":"codex","command":"cat secret"}"#;
        let encoded = URL_SAFE_NO_PAD.encode(raw);
        assert!(decode_osc_data(&format!("{}{}", OSC_PREFIX, encoded)).is_err());
    }
}
