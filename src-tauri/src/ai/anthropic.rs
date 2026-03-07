use anyhow::{Context, Result};
use reqwest::Client;
use serde::{Deserialize, Serialize};

use std::sync::Arc;

use super::cache::TrackInfoCache;
use super::AgentResponse;
use crate::auth::AnthropicAuth;
use crate::spotify::TrackInfo;

const MESSAGES_API: &str = "https://api.anthropic.com/v1/messages";
const API_VERSION: &str = "2023-06-01";
const MAX_TOKENS: u32 = 4096;

#[derive(Debug, Serialize)]
struct MessagesRequest {
    model: String,
    max_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<String>,
    messages: Vec<Message>,
}

#[derive(Debug, Serialize)]
struct Message {
    role: String,
    content: String,
}

#[derive(Debug, Deserialize)]
struct MessagesResponse {
    content: Vec<ContentBlock>,
}

#[derive(Debug, Deserialize)]
struct ContentBlock {
    #[serde(rename = "type")]
    block_type: Option<String>,
    text: Option<String>,
}

pub struct AnthropicService {
    client: Client,
    auth: Arc<AnthropicAuth>,
    cache: TrackInfoCache,
}

impl AnthropicService {
    pub fn new(auth: Arc<AnthropicAuth>) -> Self {
        Self {
            client: Client::new(),
            auth,
            cache: TrackInfoCache::default(),
        }
    }

    /// Generate track description using Claude.
    /// Returns (description, used_web_search). Web search is always false for Anthropic.
    pub async fn get_track_description(
        &self,
        track: &TrackInfo,
        model: &str,
        prompt_template: &str,
        _web_search: bool,
        force: bool,
        memories: &[String],
    ) -> Result<(String, bool)> {
        if force {
            self.cache.remove(&track.id).await;
        } else if let Some(cached) = self.cache.get(&track.id).await {
            return Ok((cached, false));
        }

        let api_key = self
            .auth
            .get_api_key()
            .context("Anthropic API key not available")?;

        let memories_str = if memories.is_empty() {
            String::new()
        } else {
            let items: Vec<String> = memories
                .iter()
                .enumerate()
                .map(|(i, m)| format!("{}. {}", i + 1, m))
                .collect();
            format!("User memories:\n{}", items.join("\n"))
        };

        let prompt = prompt_template
            .replace("{name}", &track.name)
            .replace("{artist}", &track.artist)
            .replace("{album}", &track.album)
            .replace("{memories}", &memories_str);

        let request = MessagesRequest {
            model: model.to_string(),
            max_tokens: MAX_TOKENS,
            system: Some("You are a music expert with deep knowledge of musical styles, genres, creators, music theory, music and art history, as well as fascinating stories and trivia. You excel at making music accessible and engaging, effectively conveying knowledge while sparking the listener's curiosity.".to_string()),
            messages: vec![Message {
                role: "user".to_string(),
                content: prompt,
            }],
        };

        let response: MessagesResponse = self
            .client
            .post(MESSAGES_API)
            .header("x-api-key", api_key)
            .header("anthropic-version", API_VERSION)
            .json(&request)
            .send()
            .await?
            .error_for_status()
            .map_err(|e| {
                log::error!("[anthropic] API error: {}", e);
                e
            })?
            .json()
            .await
            .context("Failed to parse Anthropic response")?;

        let description = response
            .content
            .iter()
            .filter(|b| b.block_type.as_deref() != Some("thinking"))
            .filter_map(|b| b.text.as_deref())
            .collect::<Vec<_>>()
            .join("");

        if description.is_empty() {
            anyhow::bail!("Empty response from Anthropic");
        }

        log::info!("[anthropic] AI description for '{}' generated", track.name);
        self.cache.set(track.id.clone(), description.clone()).await;

        Ok((description, false))
    }

    /// Execute agent chat via Claude Messages API.
    pub async fn agent_chat(
        &self,
        messages: &[super::ChatMessage],
        model: &str,
        prompt_template: &str,
        track_name: &str,
        artist: &str,
        album: &str,
        volume: u32,
        _web_search: bool,
        memories: &[String],
    ) -> Result<AgentResponse> {
        let api_key = self
            .auth
            .get_api_key()
            .context("Anthropic API key not available")?;

        let memories_str = if memories.is_empty() {
            String::new()
        } else {
            let items: Vec<String> = memories
                .iter()
                .enumerate()
                .map(|(i, m)| format!("{}. {}", i + 1, m))
                .collect();
            format!("User memories:\n{}", items.join("\n"))
        };

        let system_prompt = prompt_template
            .replace("{name}", track_name)
            .replace("{artist}", artist)
            .replace("{album}", album)
            .replace("{volume}", &volume.to_string())
            .replace("{memories}", &memories_str);

        let msgs: Vec<Message> = messages
            .iter()
            .map(|m| Message {
                role: m.role.clone(),
                content: m.content.clone(),
            })
            .collect();

        let request = MessagesRequest {
            model: model.to_string(),
            max_tokens: MAX_TOKENS,
            system: Some(system_prompt),
            messages: msgs,
        };

        let response: MessagesResponse = self
            .client
            .post(MESSAGES_API)
            .header("x-api-key", api_key)
            .header("anthropic-version", API_VERSION)
            .json(&request)
            .send()
            .await?
            .error_for_status()
            .map_err(|e| {
                log::error!("[anthropic] Chat API error: {}", e);
                e
            })?
            .json()
            .await
            .context("Failed to parse Anthropic chat response")?;

        let text = response
            .content
            .iter()
            .filter(|b| b.block_type.as_deref() != Some("thinking"))
            .filter_map(|b| b.text.as_deref())
            .collect::<Vec<_>>()
            .join("");

        Ok(super::parse_agent_response(&text))
    }
}
