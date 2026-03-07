use anyhow::{Context, Result};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use super::cache::TrackInfoCache;
use super::{AgentResponse, ChatMessage};
use crate::auth::OpenAIAuth;
use crate::spotify::TrackInfo;

const CODEX_API_ENDPOINT: &str = "https://chatgpt.com/backend-api/codex/responses";

#[derive(Debug, Serialize)]
struct CodexRequest {
    model: String,
    input: Vec<InputMessage>,
    instructions: String,
    store: bool,
    stream: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<Tool>,
}

#[derive(Debug, Serialize)]
struct Tool {
    r#type: String,
}

#[derive(Debug, Serialize)]
struct InputMessage {
    role: String,
    content: String,
}

// SSE response parsing types
#[derive(Debug, Deserialize)]
struct CompletedEvent {
    response: CompletedResponse,
}

#[derive(Debug, Deserialize)]
struct CompletedResponse {
    output: Vec<OutputItem>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
enum OutputItem {
    #[serde(rename = "reasoning")]
    Reasoning {},
    #[serde(rename = "message")]
    Message { content: Vec<ContentPart> },
    #[serde(rename = "web_search_call")]
    WebSearchCall {},
    #[serde(other)]
    Other,
}

#[derive(Debug, Deserialize)]
struct ContentPart {
    text: String,
}

pub struct OpenAIService {
    client: Client,
    auth: Arc<OpenAIAuth>,
    cache: TrackInfoCache,
}

impl OpenAIService {
    pub fn new(auth: Arc<OpenAIAuth>) -> Self {
        Self {
            client: Client::new(),
            auth,
            cache: TrackInfoCache::default(),
        }
    }

    /// Generate track description using AI
    /// Returns (description, used_web_search)
    /// If `force` is true, bypass the cache and re-generate.
    pub async fn get_track_description(
        &self,
        track: &TrackInfo,
        model: &str,
        prompt_template: &str,
        web_search: bool,
        force: bool,
        memories: &[String],
    ) -> Result<(String, bool)> {
        if force {
            self.cache.remove(&track.id).await;
        } else if let Some(cached) = self.cache.get(&track.id).await {
            return Ok((cached, false));
        }

        let token = self.auth.get_access_token().await?;

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

        let tools = if web_search {
            vec![Tool {
                r#type: "web_search".to_string(),
            }]
        } else {
            vec![]
        };

        let request = CodexRequest {
            model: model.to_string(),
            input: vec![InputMessage {
                role: "user".to_string(),
                content: prompt,
            }],
            instructions: "You are a music expert with deep knowledge of musical styles, genres, creators, music theory, music and art history, as well as fascinating stories and trivia. You excel at making music accessible and engaging, effectively conveying knowledge while sparking the listener's curiosity.".to_string(),
            store: false,
            stream: true,
            tools,
        };

        let response = self
            .client
            .post(CODEX_API_ENDPOINT)
            .bearer_auth(&token)
            .json(&request)
            .send()
            .await?
            .error_for_status()?
            .text()
            .await?;

        // Parse SSE stream to extract text from response.completed event
        let (description, used_web_search) = parse_sse_response(&response)?;

        log::info!(
            "AI description for '{}': web_search={}",
            track.name,
            used_web_search
        );

        // Cache the result
        self.cache.set(track.id.clone(), description.clone()).await;

        Ok((description, used_web_search))
    }

    /// Execute agent chat: send conversation history with system prompt, get structured action response
    pub async fn agent_chat(
        &self,
        messages: &[ChatMessage],
        model: &str,
        prompt_template: &str,
        track_name: &str,
        artist: &str,
        album: &str,
        volume: u32,
        web_search: bool,
        memories: &[String],
    ) -> Result<AgentResponse> {
        let token = self.auth.get_access_token().await?;

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

        let mut input: Vec<InputMessage> = Vec::new();
        for msg in messages {
            input.push(InputMessage {
                role: msg.role.clone(),
                content: msg.content.clone(),
            });
        }

        let tools = if web_search {
            vec![Tool {
                r#type: "web_search".to_string(),
            }]
        } else {
            vec![]
        };

        let request = CodexRequest {
            model: model.to_string(),
            input,
            instructions: system_prompt,
            store: false,
            stream: true,
            tools,
        };

        let response = self
            .client
            .post(CODEX_API_ENDPOINT)
            .bearer_auth(&token)
            .json(&request)
            .send()
            .await?
            .error_for_status()?
            .text()
            .await?;

        let (text, _) = parse_sse_response(&response)?;

        Ok(super::parse_agent_response(&text))
    }
}

/// Parse SSE response to extract the final text output and whether web search was used.
/// Returns (text, used_web_search).
fn parse_sse_response(body: &str) -> Result<(String, bool)> {
    for chunk in body.split("\n\n") {
        for line in chunk.lines() {
            if let Some(data) = line.strip_prefix("data: ") {
                if let Ok(event) = serde_json::from_str::<CompletedEvent>(data) {
                    let mut text = None;
                    let mut used_web_search = false;
                    for item in &event.response.output {
                        match item {
                            OutputItem::WebSearchCall {} => {
                                used_web_search = true;
                            }
                            OutputItem::Message { content } => {
                                if let Some(part) = content.first() {
                                    text = Some(part.text.clone());
                                }
                            }
                            _ => {}
                        }
                    }
                    if let Some(t) = text {
                        return Ok((t, used_web_search));
                    }
                }
            }
        }
    }
    anyhow::bail!("No text output found in response")
}
