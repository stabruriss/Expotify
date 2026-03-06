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
