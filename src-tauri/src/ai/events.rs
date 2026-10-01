//! Redacted, structured tool-call event log.
//!
//! One JSON line per event, appended to `tool-events.jsonl` next to `settings.json`.
//! Records what happened to a tool call (protocol, stage, tool name, outcome, error
//! code, timing) and never the conversation text, prompts, queries or arguments.

use serde::Serialize;
use std::io::Write;
use std::path::PathBuf;

const MAX_BYTES: u64 = 2 * 1024 * 1024;

#[derive(Debug, Clone, Serialize)]
pub struct ToolEvent {
    pub ts: String,
    /// Content-free id shared by every event of one chat request.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    pub provider: &'static str,
    pub model: String,
    pub protocol: &'static str,
    /// `parse` (legacy text parsed), `exec` (executor result, written as soon as the call
    /// finished), `turn` (one chat request ended: completed, failed or cancelled).
    pub stage: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    /// Provider call id of an `exec` event; matches `call_id` in the UI's `tool_results`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub call_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ok: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
    /// Legacy parser path (`direct`, `extracted`, `fallback`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parse_via: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub turns: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
}

impl ToolEvent {
    pub fn new(provider: &'static str, model: &str, protocol: &'static str, stage: &'static str) -> Self {
        Self {
            ts: chrono::Utc::now().to_rfc3339(),
            request_id: None,
            provider,
            model: model.to_string(),
            protocol,
            stage,
            tool: None,
            call_id: None,
            ok: None,
            error_code: None,
            parse_via: None,
            turns: None,
            duration_ms: None,
        }
    }
}

/// Identity of one chat request; every event it records carries the same `request_id`.
#[derive(Debug, Clone)]
pub struct EventContext {
    pub provider: &'static str,
    pub model: String,
    pub protocol: &'static str,
    pub request_id: String,
}

impl EventContext {
    pub fn new(provider: &'static str, model: &str, protocol: &'static str) -> Self {
        Self {
            provider,
            model: model.to_string(),
            protocol,
            request_id: new_request_id(),
        }
    }

    pub fn event(&self, stage: &'static str) -> ToolEvent {
        let mut event = ToolEvent::new(self.provider, &self.model, self.protocol, stage);
        event.request_id = Some(self.request_id.clone());
        event
    }
}

/// Random, content-free id used to correlate a request's events and reports.
pub fn new_request_id() -> String {
    format!("{:08x}", rand::random::<u32>())
}

fn log_path() -> Option<PathBuf> {
    let dir = dirs::config_dir()?.join("expotify");
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir.join("tool-events.jsonl"))
}

fn rotate_if_needed(path: &PathBuf) {
    if let Ok(meta) = std::fs::metadata(path) {
        if meta.len() > MAX_BYTES {
            let _ = std::fs::rename(path, path.with_extension("jsonl.1"));
        }
    }
}

/// Append one event. Failures are logged and otherwise ignored; logging must never
/// affect the chat request.
pub fn record(event: ToolEvent) {
    let Some(path) = log_path() else { return };
    rotate_if_needed(&path);
    let line = match serde_json::to_string(&event) {
        Ok(line) => line,
        Err(e) => {
            log::warn!("[events] serialize failed: {e}");
            return;
        }
    };
    let result = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .and_then(|mut file| writeln!(file, "{line}"));
    if let Err(e) = result {
        log::warn!("[events] write failed: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_carry_no_free_text_fields() {
        let context = EventContext::new("openai", "gpt-test", "legacy");
        let mut event = context.event("exec");
        event.tool = Some("set_volume".into());
        event.call_id = Some("call_1".into());
        event.ok = Some(false);
        event.error_code = Some("invalid_argument".into());
        let json = serde_json::to_value(&event).unwrap();
        let keys: Vec<&str> = json.as_object().unwrap().keys().map(String::as_str).collect();
        for key in keys {
            assert!(
                ["ts", "request_id", "provider", "model", "protocol", "stage", "tool", "call_id", "ok", "error_code", "parse_via", "turns", "duration_ms"].contains(&key),
                "unexpected field {key}"
            );
        }
        assert!(json.get("message").is_none());
        assert_eq!(json["request_id"], context.request_id);
        assert_eq!(context.request_id.len(), 8);
    }
}
