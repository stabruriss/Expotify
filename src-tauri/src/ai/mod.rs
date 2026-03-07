pub mod anthropic;
pub mod cache;
pub mod openai;

use serde::{Deserialize, Serialize};

pub use anthropic::AnthropicService;
pub use cache::TrackInfoCache;
pub use openai::OpenAIService;

/// Agent chat message (user or assistant) — shared between providers.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

/// Agent response from LLM — shared between providers.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentResponse {
    pub action: String,
    pub message: String,
    #[serde(default)]
    pub args: serde_json::Value,
}

/// Parse AI text into AgentResponse.
/// Handles: pure JSON, markdown-fenced JSON, and JSON embedded in surrounding text.
pub fn parse_agent_response(text: &str) -> AgentResponse {
    let trimmed = text.trim();

    // 1. Strip markdown code fences
    let defenced = if trimmed.starts_with("```") {
        trimmed
            .trim_start_matches("```json")
            .trim_start_matches("```")
            .trim_end_matches("```")
            .trim()
    } else {
        trimmed
    };

    // 2. Try direct parse
    if let Ok(resp) = serde_json::from_str::<AgentResponse>(defenced) {
        return resp;
    }

    // 3. Extract first JSON object by matching braces
    if let Some(json_str) = extract_json_object(trimmed) {
        if let Ok(resp) = serde_json::from_str::<AgentResponse>(json_str) {
            return resp;
        }
    }

    // 4. Fallback: plain text reply
    AgentResponse {
        action: "reply".to_string(),
        message: text.to_string(),
        args: serde_json::Value::Null,
    }
}

/// Extract the first top-level `{...}` JSON object from text, handling nested braces.
fn extract_json_object(text: &str) -> Option<&str> {
    let start = text.find('{')?;
    let mut depth = 0;
    let mut in_string = false;
    let mut escape_next = false;

    for (i, ch) in text[start..].char_indices() {
        if escape_next {
            escape_next = false;
            continue;
        }
        match ch {
            '\\' if in_string => escape_next = true,
            '"' => in_string = !in_string,
            '{' if !in_string => depth += 1,
            '}' if !in_string => {
                depth -= 1;
                if depth == 0 {
                    return Some(&text[start..start + i + 1]);
                }
            }
            _ => {}
        }
    }
    None
}
