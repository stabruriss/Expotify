use anyhow::Result;
use chrono::{DateTime, Utc};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use super::cache::TrackInfoCache;
use super::models::{CatalogCache, ModelInfo, ModelProvider, ModelSelection, ProviderCatalog};
use super::tools::{self, ChatCancellation, ToolCall, ToolContext, ToolRunner};
use super::{
    legacy_system_prompt, native_system_prompt, render_chat_prompt, AgentResponse, ChatMessage,
    NativeChatOutcome,
};
use crate::auth::OpenAIAuth;
use crate::spotify::TrackInfo;
use serde_json::json;

const CODEX_API_ENDPOINT: &str = "https://chatgpt.com/backend-api/codex/responses";
const CODEX_MODELS_ENDPOINT: &str =
    "https://chatgpt.com/backend-api/codex/models?client_version=1.0.0";

#[derive(Deserialize)]
struct CodexModel {
    slug: String,
    display_name: String,
    visibility: String,
    priority: i32,
    upgrade: Option<CodexUpgrade>,
}

#[derive(Deserialize)]
struct CodexUpgrade {
    retirement_at: Option<DateTime<Utc>>,
}

#[derive(Deserialize)]
struct CodexModelsResponse {
    models: Vec<CodexModel>,
}

fn parse_catalog(body: &str, now: DateTime<Utc>) -> Result<ProviderCatalog> {
    let response: CodexModelsResponse = serde_json::from_str(body)?;
    let mut visible: Vec<_> = response
        .models
        .into_iter()
        .filter(|model| {
            model.visibility == "list"
                && !model.slug.is_empty()
                && model
                    .upgrade
                    .as_ref()
                    .and_then(|upgrade| upgrade.retirement_at)
                    .map(|date| date > now)
                    .unwrap_or(true)
        })
        .collect();
    // This HTTP catalog exposes the provider's recommendation as ascending priority.
    visible.sort_by_key(|model| model.priority);
    let default = visible
        .first()
        .ok_or_else(|| anyhow::anyhow!("ChatGPT returned no available models"))?
        .slug
        .clone();
    let models = visible
        .into_iter()
        .map(|model| ModelInfo {
            id: model.slug,
            name: model.display_name,
        })
        .collect();
    ProviderCatalog::new(ModelProvider::Openai, models, default)
}

async fn response_body(response: reqwest::Response) -> Result<String> {
    let status = response.status();
    let body = response.text().await?;
    if !status.is_success() {
        let json = serde_json::from_str::<Value>(&body).unwrap_or(Value::Null);
        let detail = json
            .get("detail")
            .and_then(Value::as_str)
            .or_else(|| json.pointer("/error/message").and_then(Value::as_str))
            .or_else(|| json.get("message").and_then(Value::as_str))
            .unwrap_or(
                "The provider rejected the request. Check the selected model and connection.",
            );
        anyhow::bail!(
            "ChatGPT request failed ({status}): {}",
            detail.chars().take(600).collect::<String>()
        );
    }
    Ok(body)
}

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
    catalog: CatalogCache,
}

impl OpenAIService {
    pub fn new(auth: Arc<OpenAIAuth>) -> Self {
        Self {
            client: Client::builder()
                .timeout(Duration::from_secs(180))
                .build()
                .expect("HTTP client configuration"),
            auth,
            cache: TrackInfoCache::default(),
            catalog: CatalogCache::default(),
        }
    }

    pub async fn list_models(&self, force: bool) -> ProviderCatalog {
        self.catalog
            .get(ModelProvider::Openai, force, || async {
                let token = self.auth.get_access_token().await?;
                let response = self
                    .client
                    .get(CODEX_MODELS_ENDPOINT)
                    .bearer_auth(token)
                    .timeout(Duration::from_secs(15))
                    .send()
                    .await?;
                parse_catalog(&response_body(response).await?, Utc::now())
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
        let cache_key = format!("{}:{model}", track.id);
        if force {
            self.cache.remove(&cache_key).await;
        } else if let Some(cached) = self.cache.get(&cache_key).await {
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

        let response = response_body(
            self.client
                .post(CODEX_API_ENDPOINT)
                .bearer_auth(&token)
                .json(&request)
                .send()
                .await?,
        )
        .await?;

        // Parse SSE stream to extract text from response.completed event
        let (description, used_web_search) = parse_sse_response(&response)?;

        log::info!(
            "AI description for '{}': web_search={}",
            track.name,
            used_web_search
        );

        // Cache the result
        self.cache.set(cache_key, description.clone()).await;

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

        let system_prompt = legacy_system_prompt(
            &prompt_template
                .replace("{name}", track_name)
                .replace("{artist}", artist)
                .replace("{album}", album)
                .replace("{volume}", &volume.to_string())
                .replace("{memories}", &memories_str),
        );

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

        let response = response_body(
            self.client
                .post(CODEX_API_ENDPOINT)
                .bearer_auth(&token)
                .json(&request)
                .send()
                .await?,
        )
        .await?;

        let (text, _) = parse_sse_response(&response)?;

        Ok(super::parse_agent_response(&text))
    }
}

impl OpenAIService {
    /// Native tool calling over the Responses protocol (same Codex endpoint and login as
    /// today). Tools are registered as strict function definitions; each `function_call`
    /// item is executed by `runner`, and its `function_call_output` is fed back together
    /// with the turn's own output items (including reasoning items, which `store:false`
    /// requires us to carry) until the model answers without calling a tool.
    #[allow(clippy::too_many_arguments)]
    pub async fn agent_chat_native(
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
        ctx: &ToolContext<'_>,
        runner: &mut ToolRunner,
        cancellation: &ChatCancellation,
    ) -> Result<NativeChatOutcome> {
        let instructions = native_system_prompt(&render_chat_prompt(
            prompt_template,
            track_name,
            artist,
            album,
            volume,
            memories,
        ));
        let tool_definitions = native_tools(web_search);
        let mut input = history_items(messages);
        let mut tool_uses = 0u32;
        let mut rounds = 0u32;
        loop {
            rounds += 1;
            let token = self.auth.get_access_token().await?;
            let request = native_request(model, &instructions, &input, &tool_definitions);
            // Cancellation drops the HTTP request; a tool that is already running is never
            // interrupted, and its outcome stays in `runner` for the caller to report.
            let body = tokio::select! {
                body = async {
                    let response = self
                        .client
                        .post(CODEX_API_ENDPOINT)
                        .bearer_auth(&token)
                        .json(&request)
                        .send()
                        .await?;
                    response_body(response).await
                } => body?,
                _ = cancellation.cancelled() => anyhow::bail!(tools::CANCELLED),
            };
            let turn = parse_native_sse(&body)?;
            if turn.function_calls.is_empty() {
                let text = turn.text.trim().to_string();
                if text.is_empty() && tool_uses == 0 {
                    anyhow::bail!("ChatGPT returned an empty response");
                }
                return Ok(NativeChatOutcome {
                    text,
                    turns: Some(rounds),
                    tool_uses,
                });
            }
            if rounds >= MAX_TOOL_ROUNDS {
                anyhow::bail!(
                    "ChatGPT kept requesting tools after {MAX_TOOL_ROUNDS} rounds; stopped without a final reply."
                );
            }
            // Carry this turn's output items back verbatim (reasoning + function_call ...),
            // then answer each call in order.
            input.extend(turn.output_items.iter().cloned());
            for call in &turn.function_calls {
                tool_uses += 1;
                let tool_call = ToolCall {
                    id: call.call_id.clone(),
                    name: call.name.clone(),
                    args: parse_arguments(&call.arguments),
                };
                let outcome = runner.run(ctx, &tool_call).await;
                if cancellation.is_cancelled() {
                    anyhow::bail!(tools::CANCELLED);
                }
                input.push(json!({
                    "type": "function_call_output",
                    "call_id": call.call_id,
                    "output": outcome.output,
                }));
            }
        }
    }
}

const MAX_TOOL_ROUNDS: u32 = 3;

/// Registry tools as strict Responses function definitions, plus the server-side web search.
fn native_tools(web_search: bool) -> Vec<Value> {
    let mut list: Vec<Value> = tools::definitions()
        .into_iter()
        .map(|def| {
            json!({
                "type": "function",
                "name": def.name,
                "description": def.description,
                "strict": true,
                "parameters": def.parameters,
            })
        })
        .collect();
    if web_search {
        list.push(json!({ "type": "web_search" }));
    }
    list
}

fn native_request(
    model: &str,
    instructions: &str,
    input: &[Value],
    tool_definitions: &[Value],
) -> Value {
    json!({
        "model": model,
        "instructions": instructions,
        "input": input,
        "store": false,
        "stream": true,
        "tools": tool_definitions,
        "tool_choice": "auto",
        "parallel_tool_calls": false,
        "include": ["reasoning.encrypted_content"],
    })
}

/// Prior turns as input messages; an assistant message carries what its tools really did.
fn history_items(messages: &[ChatMessage]) -> Vec<Value> {
    messages
        .iter()
        .map(|message| {
            let mut content = message.content.clone();
            if message.role == "assistant" && !message.tool_results.is_empty() {
                content.push('\n');
                for result in &message.tool_results {
                    let status = if result.ok { "ok" } else { "failed" };
                    content.push_str(&format!(
                        "[tool {} → {status}: {}]\n",
                        result.name, result.output
                    ));
                }
            }
            json!({ "type": "message", "role": message.role, "content": content })
        })
        .collect()
}

/// Function-call arguments arrive as a JSON string. Anything that is not an object reaches
/// the validator as-is and is rejected there with an explicit error.
fn parse_arguments(raw: &str) -> Value {
    if raw.trim().is_empty() {
        return Value::Object(Default::default());
    }
    serde_json::from_str(raw).unwrap_or_else(|_| Value::String(raw.to_string()))
}

#[derive(Debug, Clone, PartialEq)]
struct FunctionCallItem {
    call_id: String,
    name: String,
    arguments: String,
}

#[derive(Debug, Default)]
struct NativeTurn {
    text: String,
    /// Every output item of the turn, in order, to pass back on the next request.
    output_items: Vec<Value>,
    function_calls: Vec<FunctionCallItem>,
    used_web_search: bool,
}

/// Parse a native-protocol stream: output items come from `response.output_item.done`
/// (arguments filled from `response.function_call_arguments.done` when the item lacks
/// them) and are replaced by the authoritative `response.completed` output when present.
fn parse_native_sse(body: &str) -> Result<NativeTurn> {
    let mut done_items: BTreeMap<u64, Value> = BTreeMap::new();
    let mut argument_snapshots: BTreeMap<u64, String> = BTreeMap::new();
    let mut completed_output: Option<Vec<Value>> = None;
    for event in parse_sse_events(body)? {
        let index = event
            .get("output_index")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        match event.get("type").and_then(Value::as_str) {
            Some("error" | "response.failed" | "response.incomplete") => {
                return Err(stream_failure(&event));
            }
            Some("response.output_item.done") => {
                if let Some(item) = event.get("item") {
                    done_items.insert(index, item.clone());
                }
            }
            Some("response.function_call_arguments.done") => {
                if let Some(arguments) = event.get("arguments").and_then(Value::as_str) {
                    argument_snapshots.insert(index, arguments.to_string());
                }
            }
            Some("response.completed") => {
                if matches!(
                    event.pointer("/response/status").and_then(Value::as_str),
                    Some("failed" | "incomplete" | "cancelled")
                ) {
                    return Err(stream_failure(&event));
                }
                completed_output = event
                    .pointer("/response/output")
                    .and_then(Value::as_array)
                    .cloned();
            }
            _ => {}
        }
    }
    let output_items: Vec<Value> = match completed_output {
        Some(items) if !items.is_empty() => items,
        _ => done_items
            .into_iter()
            .map(|(index, mut item)| {
                let missing_arguments = item["type"] == "function_call"
                    && item["arguments"].as_str().map_or(true, str::is_empty);
                if missing_arguments {
                    if let Some(arguments) = argument_snapshots.get(&index) {
                        item["arguments"] = json!(arguments);
                    }
                }
                item
            })
            .collect(),
    };
    if output_items.is_empty() {
        // No item-level events: a text-only stream in the older shape.
        let (text, used_web_search) = parse_sse_response(body)?;
        return Ok(NativeTurn {
            text,
            used_web_search,
            ..Default::default()
        });
    }
    let mut turn = NativeTurn::default();
    for item in &output_items {
        match item["type"].as_str() {
            Some("message") => {
                if let Some(content) = item["content"].as_array() {
                    for part in content {
                        append_text_part(part, &mut turn.text);
                    }
                }
            }
            Some("function_call") => {
                let call_id = item["call_id"].as_str().unwrap_or_default();
                let name = item["name"].as_str().unwrap_or_default();
                if call_id.is_empty() || name.is_empty() {
                    anyhow::bail!("ChatGPT returned a malformed tool call");
                }
                turn.function_calls.push(FunctionCallItem {
                    call_id: call_id.to_string(),
                    name: name.to_string(),
                    arguments: item["arguments"].as_str().unwrap_or("").to_string(),
                });
            }
            Some("web_search_call") => turn.used_web_search = true,
            _ => {}
        }
    }
    turn.output_items = output_items;
    Ok(turn)
}

/// Parse SSE response to extract the final text output and whether web search was used.
/// Returns (text, used_web_search).
fn parse_sse_response(body: &str) -> Result<(String, bool)> {
    let mut used_web_search = false;
    let mut completed_text = None;
    let mut parts = BTreeMap::<(u64, u64), String>::new();

    for event in parse_sse_events(body)? {
        let output_index = event
            .get("output_index")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let content_index = event
            .get("content_index")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let key = (output_index, content_index);
        match event.get("type").and_then(Value::as_str) {
            Some("error" | "response.failed" | "response.incomplete") => {
                return Err(stream_failure(&event));
            }
            Some("response.output_text.delta") => {
                if let Some(delta) = event.get("delta").and_then(Value::as_str) {
                    parts.entry(key).or_default().push_str(delta);
                }
            }
            // Both done events contain full snapshots of the same content part.
            Some("response.output_text.done") => {
                if let Some(text) = event.get("text").and_then(Value::as_str) {
                    parts.insert(key, text.to_owned());
                }
            }
            Some("response.content_part.done") => {
                if let Some(part) = event.get("part") {
                    if let Some(text) = text_part(part) {
                        parts.insert(key, text.to_owned());
                    } else {
                        parts.remove(&key);
                    }
                }
            }
            Some(kind @ ("response.output_item.added" | "response.output_item.done")) => {
                if let Some(item) = event.get("item") {
                    used_web_search |=
                        item.get("type").and_then(Value::as_str) == Some("web_search_call");
                    if kind == "response.output_item.done"
                        && item.get("type").and_then(Value::as_str) == Some("message")
                    {
                        if let Some(content) = item.get("content").and_then(Value::as_array) {
                            parts.retain(|(index, _), _| *index != output_index);
                            for (index, part) in content.iter().enumerate() {
                                if let Some(text) = text_part(part) {
                                    parts.insert((output_index, index as u64), text.to_owned());
                                }
                            }
                        }
                    }
                }
            }
            Some("response.completed") => {
                if matches!(
                    event.pointer("/response/status").and_then(Value::as_str),
                    Some("failed" | "incomplete" | "cancelled")
                ) {
                    return Err(stream_failure(&event));
                }
                let (text, event_used_web_search) = extract_completed_text(&event);
                used_web_search |= event_used_web_search;
                if !text.trim().is_empty() {
                    completed_text = Some(text);
                }
            }
            _ => {}
        }
    }

    if let Some(text) = completed_text {
        return Ok((text, used_web_search));
    }

    let text: String = parts.into_values().collect();
    if !text.trim().is_empty() {
        return Ok((text, used_web_search));
    }

    anyhow::bail!("No text output found in response")
}

fn parse_sse_events(body: &str) -> Result<Vec<Value>> {
    let mut events = Vec::new();
    let mut data = String::new();
    // lines() accepts LF and CRLF; multiple data fields form one JSON payload.
    for line in body.lines().chain(std::iter::once("")) {
        if line.is_empty() {
            if !data.is_empty() && data.trim() != "[DONE]" {
                events
                    .push(serde_json::from_str(&data).map_err(|_| {
                        anyhow::anyhow!("Invalid event in ChatGPT response stream")
                    })?);
            }
            data.clear();
        } else if let Some(value) = line.strip_prefix("data:") {
            if !data.is_empty() {
                data.push('\n');
            }
            data.push_str(value.strip_prefix(' ').unwrap_or(value));
        }
    }
    Ok(events)
}

fn stream_failure(event: &Value) -> anyhow::Error {
    let code = event
        .pointer("/response/error/code")
        .or_else(|| event.pointer("/error/code"))
        .or_else(|| event.get("code"))
        .and_then(Value::as_str)
        .or_else(|| event.get("type").and_then(Value::as_str))
        .unwrap_or("provider_error");
    let detail = event
        .pointer("/response/error/message")
        .or_else(|| event.pointer("/error/message"))
        .or_else(|| event.get("message"))
        .or_else(|| event.pointer("/response/incomplete_details/reason"))
        .and_then(Value::as_str)
        .unwrap_or("The provider did not complete the response. Please retry.");
    let detail: String = format!("{code}: {detail}").chars().take(600).collect();
    anyhow::anyhow!("ChatGPT response failed: {detail}")
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
    if let Some(text) = text_part(part) {
        target.push_str(text);
    }
}

fn text_part(part: &Value) -> Option<&str> {
    match part.get("type").and_then(Value::as_str) {
        Some("output_text" | "text") => part.get("text").and_then(Value::as_str),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        history_items, native_request, native_tools, parse_arguments, parse_catalog,
        parse_native_sse, parse_sse_response, FunctionCallItem,
    };
    use crate::ai::tools::ToolOutcome;
    use crate::ai::ChatMessage;
    use serde_json::{json, Value};

    #[test]
    fn native_turn_collects_function_calls_and_keeps_output_items_for_the_next_request() {
        let body = sse_body(&[
            json!({"type":"response.output_item.added","output_index":0,"item":{"type":"reasoning","id":"rs_1","summary":[]}}),
            json!({"type":"response.output_item.done","output_index":0,"item":{"type":"reasoning","id":"rs_1","summary":[],"encrypted_content":"enc"}}),
            json!({"type":"response.output_item.added","output_index":1,"item":{"type":"function_call","id":"fc_1","call_id":"call_1","name":"set_volume","arguments":""}}),
            json!({"type":"response.function_call_arguments.delta","output_index":1,"item_id":"fc_1","delta":"{\"level\":"}),
            json!({"type":"response.function_call_arguments.delta","output_index":1,"item_id":"fc_1","delta":"30}"}),
            json!({"type":"response.function_call_arguments.done","output_index":1,"item_id":"fc_1","arguments":"{\"level\":30}"}),
            json!({"type":"response.output_item.done","output_index":1,"item":{"type":"function_call","id":"fc_1","call_id":"call_1","name":"set_volume","arguments":"{\"level\":30}"}}),
            json!({"type":"response.completed","response":{"status":"completed","output":[
                {"type":"reasoning","id":"rs_1","summary":[],"encrypted_content":"enc"},
                {"type":"function_call","id":"fc_1","call_id":"call_1","name":"set_volume","arguments":"{\"level\":30}"}
            ]}}),
        ]);
        let turn = parse_native_sse(&body).unwrap();
        assert_eq!(
            turn.function_calls,
            vec![FunctionCallItem {
                call_id: "call_1".into(),
                name: "set_volume".into(),
                arguments: "{\"level\":30}".into()
            }]
        );
        assert_eq!(turn.output_items.len(), 2);
        assert_eq!(turn.output_items[0]["encrypted_content"], "enc");
        assert_eq!(turn.text, "");
        assert_eq!(
            parse_arguments(&turn.function_calls[0].arguments),
            json!({"level": 30})
        );
    }

    #[test]
    fn native_turn_fills_arguments_from_the_done_snapshot_when_completed_is_missing() {
        let body = sse_body(&[
            json!({"type":"response.output_item.done","output_index":0,"item":{"type":"function_call","call_id":"call_2","name":"like_current","arguments":""}}),
            json!({"type":"response.output_item.done","output_index":1,"item":{"type":"function_call","call_id":"call_3","name":"set_volume","arguments":""}}),
            json!({"type":"response.function_call_arguments.done","output_index":1,"arguments":"{\"level\":55}"}),
        ]);
        let turn = parse_native_sse(&body).unwrap();
        assert_eq!(turn.function_calls.len(), 2);
        assert_eq!(
            parse_arguments(&turn.function_calls[0].arguments),
            json!({})
        );
        assert_eq!(
            parse_arguments(&turn.function_calls[1].arguments),
            json!({"level": 55})
        );
        assert_eq!(turn.output_items[1]["arguments"], "{\"level\":55}");
    }

    #[test]
    fn native_turn_without_tool_calls_yields_text_and_web_search_flag() {
        let body = sse_body(&[
            json!({"type":"response.output_item.done","output_index":0,"item":{"type":"web_search_call","status":"completed"}}),
            json!({"type":"response.output_item.done","output_index":1,"item":{"type":"message","content":[{"type":"output_text","text":"Released in 1905."}]}}),
            json!({"type":"response.completed","response":{"output":[
                {"type":"web_search_call","status":"completed"},
                {"type":"message","content":[{"type":"output_text","text":"Released in 1905."}]}
            ]}}),
        ]);
        let turn = parse_native_sse(&body).unwrap();
        assert!(turn.function_calls.is_empty());
        assert_eq!(turn.text, "Released in 1905.");
        assert!(turn.used_web_search);
        let text_only = sse_body(&[json!({"type":"response.output_text.delta","delta":"hi"})])
            + "data:[DONE]\n\n";
        assert_eq!(parse_native_sse(&text_only).unwrap().text, "hi");
    }

    #[test]
    fn native_turn_errors_and_malformed_calls_are_rejected() {
        let failed = sse_body(&[
            json!({"type":"response.failed","response":{"error":{"code":"test_error","message":"nope"}}}),
        ]);
        assert!(parse_native_sse(&failed)
            .unwrap_err()
            .to_string()
            .contains("test_error"));
        let malformed = sse_body(&[
            json!({"type":"response.output_item.done","output_index":0,"item":{"type":"function_call","call_id":"","name":"set_volume","arguments":"{}"}}),
        ]);
        assert!(parse_native_sse(&malformed)
            .unwrap_err()
            .to_string()
            .contains("malformed"));
        assert_eq!(
            parse_arguments("not json"),
            Value::String("not json".into())
        );
    }

    #[test]
    fn native_request_declares_strict_function_tools_and_carries_reasoning() {
        let tools = native_tools(true);
        assert_eq!(tools.len(), 8);
        assert!(tools
            .iter()
            .filter(|t| t["type"] == "function")
            .all(|t| t["strict"] == true && t["parameters"]["additionalProperties"] == false));
        assert_eq!(tools.last().unwrap()["type"], "web_search");
        let request = native_request(
            "gpt-test",
            "sys",
            &[json!({"type":"message","role":"user","content":"hi"})],
            &tools,
        );
        assert_eq!(request["tool_choice"], "auto");
        assert_eq!(request["parallel_tool_calls"], false);
        assert_eq!(request["store"], false);
        assert_eq!(request["include"], json!(["reasoning.encrypted_content"]));
        assert_eq!(native_tools(false).len(), 7);
    }

    #[test]
    fn history_items_render_executor_results_into_assistant_messages() {
        let items = history_items(&[
            ChatMessage {
                role: "user".into(),
                content: "音量30".into(),
                tool_results: vec![],
            },
            ChatMessage {
                role: "assistant".into(),
                content: "好的".into(),
                tool_results: vec![ToolOutcome {
                    call_id: "c".into(),
                    name: "set_volume".into(),
                    ok: false,
                    output: "Could not change the volume".into(),
                    error_code: Some("volume_failed".into()),
                    track_name: None,
                }],
            },
        ]);
        assert_eq!(items[0]["role"], "user");
        assert_eq!(items[0]["content"], "音量30");
        assert!(items[1]["content"]
            .as_str()
            .unwrap()
            .contains("[tool set_volume → failed: Could not change the volume]"));
    }

    fn sse_body(events: &[Value]) -> String {
        events
            .iter()
            .map(|event| format!("data: {event}\n\n"))
            .collect()
    }

    #[test]
    fn does_not_duplicate_text_across_snapshots() {
        let body = sse_body(&[
            json!({"type":"response.output_text.delta","output_index":0,"content_index":0,"delta":"ready"}),
            json!({"type":"response.output_text.done","output_index":0,"content_index":0,"text":"ready"}),
            json!({"type":"response.content_part.done","output_index":0,"content_index":0,"part":{"type":"output_text","text":"ready"}}),
        ]);
        assert_eq!(parse_sse_response(&body).unwrap().0, "ready");
    }

    #[test]
    fn completed_output_is_authoritative_and_empty_output_uses_parts() {
        for (output, expected) in [
            (json!([]), "ready"),
            (
                json!([{"type":"message","content":[{"type":"output_text","text":"final"}]}]),
                "final",
            ),
        ] {
            let body = sse_body(&[
                json!({"type":"response.output_text.delta","delta":"ready"}),
                json!({"type":"response.output_text.done","text":"ready"}),
                json!({"type":"response.content_part.done","part":{"type":"output_text","text":"ready"}}),
                json!({"type":"response.completed","response":{"output":output}}),
            ]);
            assert_eq!(parse_sse_response(&body).unwrap().0, expected);
        }
    }

    #[test]
    fn orders_interleaved_items_and_parts_without_deduplicating_real_repetition() {
        let body = sse_body(&[
            json!({"type":"response.output_text.done","output_index":1,"content_index":0,"text":"B"}),
            json!({"type":"response.output_text.done","output_index":0,"content_index":1,"text":"A"}),
            json!({"type":"response.output_text.delta","output_index":0,"content_index":0,"delta":"A"}),
            json!({"type":"response.content_part.done","output_index":0,"content_index":0,"part":{"type":"output_text","text":"A"}}),
        ]);
        assert_eq!(parse_sse_response(&body).unwrap().0, "AAB");
    }

    #[test]
    fn output_item_snapshot_replaces_all_its_parts_and_tracks_search() {
        let body = sse_body(&[
            json!({"type":"response.output_text.delta","output_index":0,"content_index":0,"delta":"draft"}),
            json!({"type":"response.output_text.delta","output_index":0,"content_index":1,"delta":"stale"}),
            json!({"type":"response.output_item.added","output_index":2,"item":{"type":"web_search_call"}}),
            json!({"type":"response.output_item.done","output_index":0,"item":{"type":"message","content":[{"type":"output_text","text":"fixed"}]}}),
            json!({"type":"response.output_text.done","output_index":1,"content_index":0,"text":" tail"}),
        ]);
        assert_eq!(
            parse_sse_response(&body).unwrap(),
            ("fixed tail".to_owned(), true)
        );
    }

    #[test]
    fn partial_lifecycle_snapshots_do_not_override_text_parts() {
        let body = sse_body(&[
            json!({"type":"response.in_progress","response":{"output":[{"type":"message","content":[{"type":"output_text","text":"draft"}]}]}}),
            json!({"type":"response.output_text.done","text":"final"}),
        ]);
        assert_eq!(parse_sse_response(&body).unwrap().0, "final");
    }

    #[test]
    fn failed_and_incomplete_streams_never_return_partial_action_json() {
        for failure in [
            json!({"type":"error","code":"test_error","message":"failed"}),
            json!({"type":"response.failed","response":{"error":{"code":"test_error","message":"failed"}}}),
            json!({"type":"response.incomplete","response":{"incomplete_details":{"reason":"test_error"},"output":[{"type":"message","content":[{"type":"output_text","text":"partial"}]}]}}),
            json!({"type":"response.completed","response":{"status":"failed","error":{"code":"test_error","message":"failed"}}}),
        ] {
            let body = sse_body(&[
                json!({"type":"response.output_text.done","text":"{\"action\":\"play\"}"}),
                failure,
            ]);
            let error = parse_sse_response(&body).unwrap_err().to_string();
            assert!(error.contains("test_error"), "{error}");
        }
    }

    #[test]
    fn stream_errors_are_bounded() {
        let body =
            sse_body(&[json!({"type":"error","code":"test_error","message":"x".repeat(1000)})]);
        let error = parse_sse_response(&body).unwrap_err().to_string();
        assert!(error.chars().count() <= "ChatGPT response failed: ".len() + 600);
    }

    #[test]
    fn accepts_crlf_multiline_data_and_data_without_a_space() {
        let body = concat!(
            ": heartbeat\r\n\r\n",
            "event: response.output_text.done\r\n",
            "data:{\"type\":\"response.output_text.done\",\r\n",
            "data:\"text\":\"ready\"}\r\n\r\n",
            "data:[DONE]\r\n\r\n",
        );
        assert_eq!(parse_sse_response(body).unwrap().0, "ready");
    }

    #[test]
    fn malformed_stream_events_do_not_return_partial_text() {
        let body = concat!(
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"partial\"}\n\n",
            "data: {invalid}\n\n",
        );
        assert!(parse_sse_response(body).is_err());
    }

    #[test]
    fn can_fall_back_to_deltas_when_no_snapshots_are_available() {
        let body = sse_body(&[
            json!({"type":"response.output_text.delta","delta":"hello "}),
            json!({"type":"response.output_text.delta","delta":"world"}),
        ]) + "data:[DONE]\n\n";
        assert_eq!(parse_sse_response(&body).unwrap().0, "hello world");
    }

    #[test]
    fn repeated_snapshots_preserve_one_valid_action_object() {
        let action = r#"{"action":"test","message":"ready"}"#;
        let body = sse_body(&[
            json!({"type":"response.output_text.delta","delta":action}),
            json!({"type":"response.output_text.done","text":action}),
            json!({"type":"response.content_part.done","part":{"type":"output_text","text":action}}),
        ]);
        let parsed: Value = serde_json::from_str(&parse_sse_response(&body).unwrap().0).unwrap();
        assert_eq!(parsed["action"], "test");
    }

    #[test]
    fn catalog_keeps_default_as_a_concrete_option_and_excludes_retired_models() {
        let catalog = parse_catalog(r#"{"models":[
            {"slug":"new","display_name":"New","visibility":"list","priority":2},
            {"slug":"old","display_name":"Old","visibility":"list","priority":1,"upgrade":{"retirement_at":"2020-01-01T00:00:00Z"}},
            {"slug":"hidden","display_name":"Hidden","visibility":"hide","priority":0},
            {"slug":"codex-special","display_name":"Special","visibility":"list","priority":3}
        ]}"#, chrono::Utc::now()).unwrap();
        assert_eq!(catalog.default_model.as_deref(), Some("new"));
        assert_eq!(
            catalog
                .models
                .iter()
                .map(|model| model.id.as_str())
                .collect::<Vec<_>>(),
            vec!["new", "codex-special"]
        );
    }

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
    fn full_text_done_replaces_partial_deltas_without_indices() {
        let body = concat!(
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"stream \"}\n\n",
            "data: {\"type\":\"response.output_text.done\",\"text\":\"done\"}\n\n",
            "data: [DONE]\n\n"
        );

        let (text, used_web_search) = parse_sse_response(body).unwrap();
        assert_eq!(text, "done");
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
