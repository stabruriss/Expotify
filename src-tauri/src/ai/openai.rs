use anyhow::{Context, Result};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use super::cache::TrackInfoCache;
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
    pub async fn get_track_description(
        &self,
        track: &TrackInfo,
        model: &str,
        prompt_template: &str,
        web_search: bool,
    ) -> Result<(String, bool)> {
        // Check cache first
        if let Some(cached) = self.cache.get(&track.id).await {
            return Ok((cached, false));
        }

        let token = self.auth.get_access_token().await?;

        let prompt = prompt_template
            .replace("{name}", &track.name)
            .replace("{artist}", &track.artist)
            .replace("{album}", &track.album);

        let tools = if web_search {
            vec![Tool { r#type: "web_search".to_string() }]
        } else {
            vec![]
        };

        let request = CodexRequest {
            model: model.to_string(),
            input: vec![InputMessage {
                role: "user".to_string(),
                content: prompt,
            }],
            instructions: "你是一个音乐专家，擅长简洁地介绍歌曲。".to_string(),
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
