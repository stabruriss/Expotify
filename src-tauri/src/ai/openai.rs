use anyhow::Result;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use super::cache::TrackInfoCache;
use crate::auth::OpenAIAuth;
use crate::spotify::TrackInfo;

const OPENAI_API_BASE: &str = "https://api.openai.com/v1";

#[derive(Debug, Serialize)]
struct ChatRequest {
    model: String,
    messages: Vec<Message>,
    max_tokens: u32,
    temperature: f32,
}

#[derive(Debug, Serialize, Deserialize)]
struct Message {
    role: String,
    content: String,
}

#[derive(Debug, Deserialize)]
struct ChatResponse {
    choices: Vec<Choice>,
}

#[derive(Debug, Deserialize)]
struct Choice {
    message: Message,
}

pub struct OpenAIService {
    client: Client,
    auth: Arc<OpenAIAuth>,
    cache: TrackInfoCache,
    model: String,
}

impl OpenAIService {
    pub fn new(auth: Arc<OpenAIAuth>) -> Self {
        Self {
            client: Client::new(),
            auth,
            cache: TrackInfoCache::default(),
            model: "gpt-4o-mini".to_string(),
        }
    }

    /// Set the model to use
    pub fn set_model(&mut self, model: String) {
        self.model = model;
    }

    /// Generate track description using AI
    pub async fn get_track_description(&self, track: &TrackInfo) -> Result<String> {
        // Check cache first
        if let Some(cached) = self.cache.get(&track.id).await {
            return Ok(cached);
        }

        let token = self.auth.get_access_token().await?;

        let prompt = format!(
            r#"请用中文简洁地介绍这首歌曲（100字以内）：

歌曲: {}
艺术家: {}
专辑: {}

介绍应包含：歌曲的风格/流派、创作背景或有趣的故事（如果知道的话）。不要重复歌曲名和艺术家名。直接给出介绍，不需要开头语。"#,
            track.name, track.artist, track.album
        );

        let request = ChatRequest {
            model: self.model.clone(),
            messages: vec![Message {
                role: "user".to_string(),
                content: prompt,
            }],
            max_tokens: 200,
            temperature: 0.7,
        };

        let response = self
            .client
            .post(format!("{}/chat/completions", OPENAI_API_BASE))
            .bearer_auth(&token)
            .json(&request)
            .send()
            .await?
            .error_for_status()?
            .json::<ChatResponse>()
            .await?;

        let description = response
            .choices
            .first()
            .map(|c| c.message.content.clone())
            .unwrap_or_else(|| "无法获取歌曲信息".to_string());

        // Cache the result
        self.cache.set(track.id.clone(), description.clone()).await;

        Ok(description)
    }

    /// Clear the cache
    pub async fn clear_cache(&self) {
        self.cache.clear().await;
    }
}
