use anyhow::Result;
use reqwest::Client;
use serde::Serialize;
use serde_json::Value;
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
    let mut used_web_search = false;
    let mut completed_text = None;
    let mut streamed_text = String::new();

    for chunk in body.split("\n\n") {
        for line in chunk.lines() {
            if let Some(data) = line.strip_prefix("data: ") {
                if data == "[DONE]" {
                    continue;
                }

                let Ok(event) = serde_json::from_str::<Value>(data) else {
                    continue;
                };

                match event.get("type").and_then(Value::as_str) {
                    Some("response.output_text.delta") => {
                        if let Some(delta) = event.get("delta").and_then(Value::as_str) {
                            streamed_text.push_str(delta);
                        }
                    }
                    Some("response.output_text.done") => {
                        if let Some(text) = event.get("text").and_then(Value::as_str) {
                            streamed_text.push_str(text);
                        }
                    }
                    Some("response.content_part.done") => {
                        if let Some(part) = event.get("part") {
                            append_text_part(part, &mut streamed_text);
                        }
                    }
                    _ => {
                        let (text, event_used_web_search) = extract_completed_text(&event);
                        used_web_search |= event_used_web_search;
                        if !text.trim().is_empty() {
                            completed_text = Some(text);
                        }
                    }
                }
            }
        }
    }

    if let Some(text) = completed_text {
        return Ok((text, used_web_search));
    }

    if !streamed_text.trim().is_empty() {
        return Ok((streamed_text, used_web_search));
    }

    anyhow::bail!("No text output found in response")
}

fn extract_completed_text(event: &Value) -> (String, bool) {
    let mut used_web_search = false;
    let mut text = String::new();

    let Some(output) = event
        .get("response")
        .and_then(|response| response.get("output"))
        .and_then(Value::as_array)
    else {
        return (text, used_web_search);
    };

    for item in output {
        match item.get("type").and_then(Value::as_str) {
            Some("web_search_call") => {
                used_web_search = true;
            }
            Some("message") => {
                if let Some(content) = item.get("content").and_then(Value::as_array) {
                    for part in content {
                        append_text_part(part, &mut text);
                    }
                }
            }
            _ => {}
        }
    }

    (text, used_web_search)
}

fn append_text_part(part: &Value, target: &mut String) {
    match part.get("type").and_then(Value::as_str) {
        Some("output_text") | Some("text") => {
            if let Some(text) = part.get("text").and_then(Value::as_str) {
                target.push_str(text);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::parse_sse_response;

    #[test]
    fn parses_completed_output_text_content() {
        let body = concat!(
            "event: response.completed\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"output\":[",
            "{\"type\":\"reasoning\",\"content\":[],\"summary\":[]},",
            "{\"type\":\"message\",\"content\":[",
            "{\"type\":\"output_text\",\"text\":\"hello world\",\"annotations\":[]}",
            "]}]}}\n\n"
        );

        let (text, used_web_search) = parse_sse_response(body).unwrap();
        assert_eq!(text, "hello world");
        assert!(!used_web_search);
    }

    #[test]
    fn parses_completed_message_with_non_text_part_before_output_text() {
        let body = concat!(
            "data: {\"type\":\"response.completed\",\"response\":{\"output\":[",
            "{\"type\":\"message\",\"content\":[",
            "{\"type\":\"refusal\",\"refusal\":\"nope\"},",
            "{\"type\":\"output_text\",\"text\":\"usable text\",\"annotations\":[]}",
            "]}]}}\n\n"
        );

        let (text, used_web_search) = parse_sse_response(body).unwrap();
        assert_eq!(text, "usable text");
        assert!(!used_web_search);
    }

    #[test]
    fn falls_back_to_streamed_output_text_events() {
        let body = concat!(
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"stream \"}\n\n",
            "data: {\"type\":\"response.output_text.done\",\"text\":\"done\"}\n\n",
            "data: [DONE]\n\n"
        );

        let (text, used_web_search) = parse_sse_response(body).unwrap();
        assert_eq!(text, "stream done");
        assert!(!used_web_search);
    }

    #[test]
    fn reports_web_search_usage_from_completed_event() {
        let body = concat!(
            "data: {\"type\":\"response.completed\",\"response\":{\"output\":[",
            "{\"type\":\"web_search_call\"},",
            "{\"type\":\"message\",\"content\":[",
            "{\"type\":\"output_text\",\"text\":\"with search\",\"annotations\":[]}",
            "]}]}}\n\n"
        );

        let (text, used_web_search) = parse_sse_response(body).unwrap();
        assert_eq!(text, "with search");
        assert!(used_web_search);
    }
}
