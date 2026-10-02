pub mod anthropic;
pub mod cache;
pub mod events;
pub mod models;
pub mod openai;
pub mod tools;

use serde::{Deserialize, Serialize};

pub use anthropic::AnthropicService;
pub use cache::TrackInfoCache;
pub use openai::OpenAIService;

/// Agent chat message (user or assistant) — shared between providers.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
    /// Executor outcomes behind an assistant message, so later turns can see what
    /// actually happened (not what the model said happened).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_results: Vec<tools::ToolOutcome>,
}

/// Agent response from LLM — shared between providers.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentResponse {
    pub action: String,
    pub message: String,
    #[serde(default)]
    pub args: serde_json::Value,
    /// Which parser path produced this (`direct`, `extracted`, `fallback`). Internal
    /// metadata for the event log; never part of the wire format.
    #[serde(skip)]
    pub parse_via: Option<&'static str>,
}

/// Result of a native-protocol chat turn.
#[derive(Debug)]
pub struct NativeChatOutcome {
    pub text: String,
    pub turns: Option<u32>,
    pub tool_uses: u32,
}

pub fn render_chat_prompt(
    template: &str,
    track_name: &str,
    artist: &str,
    album: &str,
    volume: u32,
    memories: &[String],
) -> String {
    template
        .replace("{name}", track_name)
        .replace("{artist}", artist)
        .replace("{album}", album)
        .replace("{volume}", &volume.to_string())
        .replace("{memories}", &format_memories_block(memories))
}

pub const LEGACY_TOOL_SECTION_START: &str =
    "Available tools (reply with a single JSON object when using a tool):";
pub const LEGACY_TOOL_SECTION_END: &str =
    "For normal conversation, just reply with plain text — no JSON needed.";
pub const NATIVE_TOOL_GUIDANCE: &str = "You control Spotify through the provided tools (search_and_play, like_current, unlike_current, shuffle_liked, set_volume, save_memory, update_prompt). Call a tool when the user wants an action and reply in plain text otherwise. Never write a tool call as JSON text: only real tool calls are executed, and the tool result tells you what actually happened.";

/// The chat prompt is user-editable and, by default, still carries the legacy "reply with a
/// JSON object" instructions. For native tool calling that section is replaced with
/// guidance to use the registered tools; a customised prompt without the section gets the
/// guidance appended.
pub fn native_system_prompt(rendered: &str) -> String {
    if rendered.contains(NATIVE_TOOL_GUIDANCE) {
        return rendered.to_string();
    }
    if let (Some(start), Some(end_start)) = (
        rendered.find(LEGACY_TOOL_SECTION_START),
        rendered.find(LEGACY_TOOL_SECTION_END),
    ) {
        if end_start >= start {
            let end = end_start + LEGACY_TOOL_SECTION_END.len();
            return format!(
                "{}{}{}",
                &rendered[..start],
                NATIVE_TOOL_GUIDANCE,
                &rendered[end..]
            );
        }
    }
    format!("{}\n\n{}", rendered.trim_end(), NATIVE_TOOL_GUIDANCE)
}

/// Legacy text block that tells the model to answer with a JSON object. Kept so the legacy
/// protocol (no UI entry; `tool_protocol` in settings.json) still works with the native
/// default prompt.
pub const LEGACY_TOOL_SECTION: &str = r#"Available tools (reply with a single JSON object when using a tool):
- search_and_play(query): Search for a song and play the best match.
- like_current: Add current song to Liked Songs.
- unlike_current: Remove current song from Liked Songs.
- shuffle_liked: Randomly play a song from Liked Songs.
- set_volume(level): Set volume (0-100).
- save_memory(content): Save something about the user's preferences or interests.
- update_prompt(type, content): Update the AI Insight ("insight") or Chat ("chat") prompt.

Tool response format (JSON only, no markdown):
{"action": "<tool>", "args": {"<param>": <value>}, "message": "brief explanation"}

For normal conversation, just reply with plain text — no JSON needed."#;

/// Inverse of `native_system_prompt`: make sure a prompt carries the JSON convention the
/// legacy parser relies on.
pub fn legacy_system_prompt(rendered: &str) -> String {
    if rendered.contains(LEGACY_TOOL_SECTION_START) {
        return rendered.to_string();
    }
    if rendered.contains(NATIVE_TOOL_GUIDANCE) {
        return rendered.replace(NATIVE_TOOL_GUIDANCE, LEGACY_TOOL_SECTION);
    }
    const INTENT_MARKER: &str = "IMPORTANT — Music playback intent:";
    if let Some(index) = rendered.find(INTENT_MARKER) {
        return format!(
            "{}{}\n\n{}",
            &rendered[..index],
            LEGACY_TOOL_SECTION,
            &rendered[index..]
        );
    }
    format!("{}\n\n{}", rendered.trim_end(), LEGACY_TOOL_SECTION)
}

fn format_memories_block(memories: &[String]) -> String {
    if memories.is_empty() {
        return String::new();
    }
    let items: Vec<String> = memories
        .iter()
        .enumerate()
        .map(|(index, memory)| format!("{}. {}", index + 1, memory))
        .collect();
    format!("User memories:\n{}", items.join("\n"))
}

/// Parse AI text into AgentResponse (legacy text protocol).
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
    if let Ok(mut resp) = serde_json::from_str::<AgentResponse>(defenced) {
        resp.parse_via = Some("direct");
        return resp;
    }

    // 3. Extract first JSON object by matching braces
    if let Some(json_str) = extract_json_object(trimmed) {
        if let Ok(mut resp) = serde_json::from_str::<AgentResponse>(json_str) {
            resp.parse_via = Some("extracted");
            return resp;
        }
    }

    // 4. Fallback: plain text reply
    AgentResponse {
        action: "reply".to_string(),
        message: text.to_string(),
        args: serde_json::Value::Null,
        parse_via: Some("fallback"),
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
