use anyhow::{Context, Result};
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;

use super::cache::TrackInfoCache;
use super::models::{CatalogCache, ModelInfo, ModelProvider, ModelSelection, ProviderCatalog};
use super::AgentResponse;
use crate::auth::AnthropicAuth;
use crate::spotify::TrackInfo;

const TRACK_SYSTEM_PROMPT: &str = "You are a music expert with deep knowledge of musical styles, genres, creators, music theory, music and art history, as well as fascinating stories and trivia. You excel at making music accessible and engaging, effectively conveying knowledge while sparking the listener's curiosity.";

pub struct AnthropicService {
    auth: Arc<AnthropicAuth>,
    cache: TrackInfoCache,
    catalog: CatalogCache,
}

impl AnthropicService {
    pub fn new(auth: Arc<AnthropicAuth>) -> Self {
        Self {
            auth,
            cache: TrackInfoCache::default(),
            catalog: CatalogCache::default(),
        }
    }

    pub async fn list_models(&self, force: bool) -> ProviderCatalog {
        self.catalog
            .get(ModelProvider::Anthropic, force, || async {
                #[derive(Deserialize)]
                struct Catalog {
                    models: Vec<ModelInfo>,
                    default_model: String,
                    fetched_at: Option<chrono::DateTime<chrono::Utc>>,
                    stale: bool,
                    error: Option<String>,
                }
                let data = self
                    .runtime_call(json!({"action":"catalog"}), Duration::from_secs(40))
                    .await?;
                let catalog: Catalog =
                    serde_json::from_value(data).context("Invalid Claude model catalog")?;
                if catalog.fetched_at.is_none() {
                    anyhow::bail!(
                        "Claude model list has not finished loading. Refresh the model list."
                    );
                }
                let mut result = ProviderCatalog::new(
                    ModelProvider::Anthropic,
                    catalog.models,
                    catalog.default_model,
                )?;
                result.fetched_at = catalog.fetched_at;
                result.stale = catalog.stale;
                result.error = catalog.error;
                Ok(result)
            })
            .await
    }

    pub async fn resolve_model(&self, selection: &ModelSelection) -> Result<String> {
        selection.validate()?;
        match selection {
            ModelSelection::Default { .. } => self.list_models(false).await.resolve_default(),
            ModelSelection::Fixed { model, .. } => Ok(model.clone()),
        }
    }

    async fn runtime_call(
        &self,
        request: serde_json::Value,
        timeout: Duration,
    ) -> Result<serde_json::Value> {
        let result = self.auth.runtime.call(request, timeout).await;
        if result
            .as_ref()
            .err()
            .is_some_and(|error| error.is::<crate::claude_runtime::ClaudeAuthenticationError>())
        {
            self.auth.invalidate().await;
        }
        result
    }

    pub async fn get_track_description(
        &self,
        track: &TrackInfo,
        model: &str,
        prompt_template: &str,
        _web_search: bool,
        force: bool,
        memories: &[String],
    ) -> Result<(String, bool)> {
        let cache_key = format!("{}:{model}", track.id);
        if force {
            self.cache.remove(&cache_key).await;
        } else if let Some(cached) = self.cache.get(&cache_key).await {
            return Ok((cached, false));
        }
        let prompt = prompt_template
            .replace("{name}", &track.name)
            .replace("{artist}", &track.artist)
            .replace("{album}", &track.album)
            .replace("{memories}", &format_memories(memories));
        let description = self.run_prompt(model, TRACK_SYSTEM_PROMPT, &prompt).await?;
        self.cache.set(cache_key, description.clone()).await;
        Ok((description, false))
    }

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
        let system_prompt = prompt_template
            .replace("{name}", track_name)
            .replace("{artist}", artist)
            .replace("{album}", album)
            .replace("{volume}", &volume.to_string())
            .replace("{memories}", &format_memories(memories));
        let text = self
            .run_prompt(model, &system_prompt, &format_chat_history(messages))
            .await?;
        Ok(super::parse_agent_response(&text))
    }

    async fn run_prompt(&self, model: &str, system_prompt: &str, prompt: &str) -> Result<String> {
        let data = self
            .runtime_call(
                json!({
                    "action":"prompt", "model":model, "systemPrompt":system_prompt, "prompt":prompt,
                }),
                Duration::from_secs(180),
            )
            .await?;
        data["text"]
            .as_str()
            .filter(|text| !text.trim().is_empty())
            .map(str::to_owned)
            .context("Empty response from Claude")
    }
}

fn format_memories(memories: &[String]) -> String {
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

fn format_chat_history(messages: &[super::ChatMessage]) -> String {
    if messages.is_empty() {
        return "User:".to_string();
    }
    let mut transcript = String::from("Conversation so far:\n\nReply to the latest user message. If you decide to call a tool, return only the JSON object requested in the system prompt.\n\n");
    for message in messages {
        let speaker = if message.role == "assistant" {
            "Assistant"
        } else {
            "User"
        };
        transcript.push_str(&format!("{speaker}: {}\n\n", message.content));
    }
    transcript
}
