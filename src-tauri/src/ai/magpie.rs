//! The user's local Magpie gateway (https://github.com/yetone/magpie) as a model provider:
//! discovery, the fixed-model catalog, Insight and native tool calling over Chat Completions.
//!
//! The user installs, signs in to and runs Magpie; Expotify never starts it and keeps no
//! credential for it. Requests go straight from Rust to 127.0.0.1 (no proxy, no redirects) on
//! the port Magpie's own settings file names. Discovery runs only when the UI asks (`probe`);
//! status and model lists read its last result. See docs/magpie.md.
//!
//! Magpie may hand a request to one of the provider's fallback models before the first byte,
//! and there is no per-request way to turn that off. A successful reply is therefore used only
//! when its `X-Magpie-Model` header names exactly the requested model; otherwise the reply and
//! every action it asks for are discarded. That rejects substituted output; it cannot stop
//! Magpie from having sent the request to another model first.

use anyhow::{anyhow, bail, Result};
use reqwest::header::{HeaderMap, RETRY_AFTER};
use reqwest::{Client, StatusCode};
use serde_json::{json, Value};
use std::collections::HashSet;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

use super::anthropic::TRACK_SYSTEM_PROMPT;
use super::cache::TrackInfoCache;
use super::models::{ModelInfo, ModelProvider, ModelSelection, ProviderCatalog};
use super::tools::{self, ChatCancellation, ToolCall, ToolContext, ToolRunner};
use super::{
    format_memories_block, native_system_prompt, render_chat_prompt, ChatMessage, NativeChatOutcome,
};
use crate::spotify::TrackInfo;

/// Magpie reads a `magpie-<app>` key as the caller's name (usage attribution, per-app model
/// visibility and order). On loopback it checks no key, so this is a name, not a secret.
const CALLER_KEY: &str = "magpie-expotify";
const DEFAULT_PORT: u16 = 3425;
/// One budget for a whole discovery (hello and model list), and for the hello before a request.
const PROBE_TIMEOUT: Duration = Duration::from_secs(10);
/// One non-streaming generation request; Magpie sends no keepalive without streaming.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(240);
const MAX_TOOL_ROUNDS: u32 = 3;
const UNREADABLE_LIST: &str = "Magpie returned a model list Expotify could not read.";
const MALFORMED_CALLS: &str =
    "Magpie returned a malformed tool call, so no action from this reply was run.";

/// Why a discovery failed: Magpie is absent (nothing answers, or the port belongs to another
/// service), or Magpie answered but its model list cannot be used.
#[derive(Debug, PartialEq)]
enum ProbeError {
    Unreachable,
    NotMagpie,
    Failed(String),
}

#[derive(Default)]
struct Discovered {
    /// What the model list shows; `None` while Magpie is absent.
    catalog: Option<ProviderCatalog>,
    /// The last successful catalog, shown as stale when Magpie answers but fails.
    last_good: Option<ProviderCatalog>,
}

pub struct MagpieService {
    client: Client,
    /// Magpie's settings file; `None` without a home directory (port 3425 then).
    settings_path: Option<PathBuf>,
    /// Serialises discovery and records when the last one finished, so callers that arrive
    /// while one runs share its result.
    probing: Mutex<Option<Instant>>,
    /// The last discovery result, read by status and model lists without waiting on a probe.
    discovered: std::sync::Mutex<Discovered>,
    probe_timeout: Duration,
    cache: TrackInfoCache,
}

impl Default for MagpieService {
    fn default() -> Self {
        Self::new()
    }
}

impl MagpieService {
    pub fn new() -> Self {
        Self::with_settings_path(settings_path())
    }

    fn with_settings_path(settings_path: Option<PathBuf>) -> Self {
        Self {
            client: Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .expect("HTTP client configuration"),
            settings_path,
            probing: Mutex::new(None),
            discovered: std::sync::Mutex::new(Discovered::default()),
            probe_timeout: PROBE_TIMEOUT,
            cache: TrackInfoCache::default(),
        }
    }

    /// Discover the gateway now. The UI calls this on opening Settings or the model picker, on
    /// Refresh, and at launch when a saved choice is Magpie; simultaneous calls share one
    /// discovery. True when Magpie answered with at least one fixed model.
    pub async fn probe(&self) -> bool {
        let requested = Instant::now();
        let mut finished = self.probing.lock().await;
        if !finished.is_some_and(|at| at >= requested) {
            let result = self.discover().await;
            self.record(result);
            *finished = Some(Instant::now());
        }
        drop(finished);
        self.available()
    }

    /// Whether the last discovery found Magpie with at least one fixed model. No network.
    pub fn available(&self) -> bool {
        self.discovered()
            .catalog
            .as_ref()
            .is_some_and(|catalog| catalog.error.is_none() && !catalog.models.is_empty())
    }

    /// The model list as last discovered; `None` while Magpie is absent. No network.
    pub fn catalog(&self) -> Option<ProviderCatalog> {
        self.discovered().catalog.clone()
    }

    fn discovered(&self) -> std::sync::MutexGuard<'_, Discovered> {
        self.discovered
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn record(&self, result: Result<Vec<ModelInfo>, ProbeError>) {
        let mut discovered = self.discovered();
        discovered.catalog = match result {
            Ok(models) => {
                let catalog = ProviderCatalog::without_default(ModelProvider::Magpie, models);
                discovered.last_good = Some(catalog.clone());
                Some(catalog)
            }
            // Magpie answered but its list is unusable: keep the last list, marked stale.
            Err(ProbeError::Failed(reason)) => {
                let mut catalog = discovered.last_good.clone().unwrap_or(ProviderCatalog {
                    provider: ModelProvider::Magpie,
                    models: vec![],
                    default_model: None,
                    fetched_at: None,
                    stale: true,
                    error: None,
                });
                catalog.stale = true;
                catalog.error = Some(reason);
                Some(catalog)
            }
            // Nothing answers, or it is not Magpie: nothing to show and no error for the user.
            Err(ProbeError::Unreachable | ProbeError::NotMagpie) => None,
        };
    }

    async fn discover(&self) -> Result<Vec<ModelInfo>, ProbeError> {
        let base = base_url(gateway_port(self.settings_path.as_deref()));
        let deadline = tokio::time::Instant::now() + self.probe_timeout;
        tokio::time::timeout_at(deadline, self.identify(&base))
            .await
            .unwrap_or(Err(ProbeError::Unreachable))?;
        // Magpie has answered: from here on a failure is Magpie's, and its last list stays.
        tokio::time::timeout_at(deadline, self.list(&base))
            .await
            .unwrap_or_else(|_| {
                Err(ProbeError::Failed(
                    "Magpie did not list its models in time.".into(),
                ))
            })
    }

    /// `GET /api/hello`, which Magpie answers itself (no upstream, no quota, no key needed on
    /// loopback) with its name. Any other answer, 401/403 included, is not Magpie.
    async fn identify(&self, base: &str) -> Result<(), ProbeError> {
        let response = self
            .client
            .get(format!("{base}/api/hello"))
            .bearer_auth(CALLER_KEY)
            .send()
            .await
            .map_err(|_| ProbeError::Unreachable)?;
        if !response.status().is_success() {
            return Err(ProbeError::NotMagpie);
        }
        match response.json::<Value>().await {
            Ok(hello) if hello["name"] == "magpie" => Ok(()),
            _ => Err(ProbeError::NotMagpie),
        }
    }

    /// `GET /v1/models`: the models Magpie shows this app, in Magpie's order.
    async fn list(&self, base: &str) -> Result<Vec<ModelInfo>, ProbeError> {
        let response = self
            .client
            .get(format!("{base}/v1/models"))
            .bearer_auth(CALLER_KEY)
            .send()
            .await
            .map_err(|_| {
                ProbeError::Failed("Magpie stopped answering while listing its models.".into())
            })?;
        let status = response.status();
        if !status.is_success() {
            return Err(ProbeError::Failed(match status {
                StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => {
                    format!("Magpie refused access to its model list ({status}).")
                }
                _ => format!("Magpie could not list its models ({status})."),
            }));
        }
        let body = response
            .text()
            .await
            .map_err(|_| ProbeError::Failed(UNREADABLE_LIST.into()))?;
        parse_catalog(&body).ok_or_else(|| ProbeError::Failed(UNREADABLE_LIST.into()))
    }

    /// Magpie has no default model, and a saved choice that lets Magpie pick the model per
    /// request (route group, bare id, Copilot Auto) is refused here rather than rewritten.
    pub fn resolve_model(&self, selection: &ModelSelection) -> Result<String> {
        selection.validate()?;
        match selection {
            ModelSelection::Default { .. } => {
                bail!("Magpie has no default model. Select a Magpie model in Settings.")
            }
            ModelSelection::Fixed { model, .. } if is_fixed_model(model) => Ok(model.clone()),
            ModelSelection::Fixed { model, .. } => bail!(
                "{model} lets Magpie pick a different model for each request. Select a fixed Magpie model in Settings."
            ),
        }
    }

    /// Generate an Insight. Returns (description, used_web_search): Magpie may have searched,
    /// but a Chat Completions reply cannot show it, so the flag stays false.
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
        let prompt = prompt_template
            .replace("{name}", &track.name)
            .replace("{artist}", &track.artist)
            .replace("{album}", &track.album)
            .replace("{memories}", &format_memories_block(memories));
        let base = self.connect().await?;
        let messages = [
            json!({"role": "system", "content": TRACK_SYSTEM_PROMPT}),
            json!({"role": "user", "content": prompt}),
        ];
        let request = chat_request(model, &messages, &[], web_search);
        let reply = parse_reply(&self.complete(&base, &request, model).await?)?;
        let description = reply.text.trim().to_string();
        if description.is_empty() {
            bail!("Empty response from Magpie");
        }
        self.cache.set(cache_key, description.clone()).await;
        Ok((description, false))
    }

    /// Native tool calling over Chat Completions (`stream: false`, function tools). Each round
    /// sends the whole conversation; each tool call is executed by `runner` and its real result
    /// is sent back as a `tool` message, until the model answers without calling a tool.
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
        let system = native_system_prompt(&render_chat_prompt(
            prompt_template,
            track_name,
            artist,
            album,
            volume,
            memories,
        ));
        let mut conversation = vec![json!({"role": "system", "content": system})];
        conversation.extend(history_messages(messages));
        let tool_definitions = function_tools();
        // Cancellation drops the HTTP request; a tool that is already running is never
        // interrupted, and its outcome stays in `runner` for the caller to report.
        let base = tokio::select! {
            base = self.connect() => base?,
            _ = cancellation.cancelled() => bail!(tools::CANCELLED),
        };
        let mut tool_uses = 0u32;
        let mut rounds = 0u32;
        loop {
            rounds += 1;
            let request = chat_request(model, &conversation, &tool_definitions, web_search);
            let body = tokio::select! {
                body = self.complete(&base, &request, model) => body?,
                _ = cancellation.cancelled() => bail!(tools::CANCELLED),
            };
            let reply = parse_reply(&body)?;
            if reply.calls.is_empty() {
                let text = reply.text.trim().to_string();
                if text.is_empty() && tool_uses == 0 {
                    bail!("Magpie returned an empty response");
                }
                return Ok(NativeChatOutcome {
                    text,
                    turns: Some(rounds),
                    tool_uses,
                });
            }
            if rounds >= MAX_TOOL_ROUNDS {
                bail!(
                    "Magpie kept requesting tools after {MAX_TOOL_ROUNDS} rounds; stopped without a final reply."
                );
            }
            conversation.push(reply.message);
            for call in &reply.calls {
                tool_uses += 1;
                let outcome = runner.run(ctx, call).await;
                if cancellation.is_cancelled() {
                    bail!(tools::CANCELLED);
                }
                conversation.push(json!({
                    "role": "tool",
                    "tool_call_id": call.id,
                    "content": outcome.output,
                }));
            }
        }
    }

    /// Start one request: read the port now (Magpie can move without a restart) and check that
    /// Magpie, not another service, answers there. Every round of the request uses this address.
    async fn connect(&self) -> Result<String> {
        let port = gateway_port(self.settings_path.as_deref());
        let base = base_url(port);
        match tokio::time::timeout(self.probe_timeout, self.identify(&base)).await {
            Ok(Ok(())) => Ok(base),
            Ok(Err(ProbeError::NotMagpie)) => bail!(
                "The service on 127.0.0.1:{port} is not Magpie. Check the port in Magpie's settings."
            ),
            _ => bail!(
                "Magpie could not be reached on 127.0.0.1:{port}. Start Magpie and try again."
            ),
        }
    }

    /// One Chat Completions call. An HTTP error is reported as the gateway gave it; a
    /// successful reply is used only when Magpie attributes it to the requested model.
    async fn complete(&self, base: &str, request: &Value, model: &str) -> Result<Value> {
        let response = self
            .client
            .post(format!("{base}/v1/chat/completions"))
            .bearer_auth(CALLER_KEY)
            .timeout(REQUEST_TIMEOUT)
            .json(request)
            .send()
            .await
            .map_err(request_error)?;
        let status = response.status();
        if !status.is_success() {
            let retry_after = response
                .headers()
                .get(RETRY_AFTER)
                .and_then(|value| value.to_str().ok())
                .map(str::to_owned);
            let body = response.text().await.unwrap_or_default();
            return Err(gateway_error(status, &body, retry_after.as_deref()));
        }
        check_served_model(response.headers(), model)?;
        let body = response.text().await.map_err(request_error)?;
        serde_json::from_str(&body)
            .map_err(|_| anyhow!("Magpie returned a reply Expotify could not read."))
    }
}

/// Where Magpie keeps its settings: `$XDG_CONFIG_HOME/magpie` when set, else `~/.config/magpie`
/// (on macOS too, so not `dirs::config_dir()`). An app opened from Finder does not see a
/// shell's XDG_CONFIG_HOME, and MAGPIE_ADDR or a portable install cannot be seen from outside.
fn settings_path() -> Option<PathBuf> {
    settings_file(std::env::var_os("XDG_CONFIG_HOME"), dirs::home_dir())
}

fn settings_file(xdg_config_home: Option<OsString>, home: Option<PathBuf>) -> Option<PathBuf> {
    let base = xdg_config_home
        .filter(|dir| !dir.is_empty())
        .map(PathBuf::from)
        .or_else(|| home.map(|home| home.join(".config")))?;
    Some(base.join("magpie").join("settings.json"))
}

/// The gateway port, read fresh for every discovery and request. Only `port` is read; the
/// file is never written, cached or logged. Missing or outside 1024–65535 means 3425, as in
/// Magpie itself.
fn gateway_port(path: Option<&Path>) -> u16 {
    path.and_then(|path| std::fs::read(path).ok())
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .and_then(|settings| settings.get("port")?.as_u64())
        .and_then(|port| u16::try_from(port).ok())
        .filter(|port| *port >= 1024)
        .unwrap_or(DEFAULT_PORT)
}

fn base_url(port: u16) -> String {
    format!("http://127.0.0.1:{port}")
}

/// A concrete `provider/model` that Magpie serves under that name (barring the provider's own
/// fallback). Route groups, bare ids (Magpie routes them through a group) and Copilot's Auto
/// pick a model per request, so they are neither offered nor sent as a fixed model; ids that
/// Settings could not save are not offered either.
fn is_fixed_model(id: &str) -> bool {
    id.contains('/')
        && !id.starts_with("group/")
        && id != "copilot/auto"
        && ModelSelection::Fixed {
            provider: ModelProvider::Magpie,
            model: id.to_string(),
        }
        .validate()
        .is_ok()
}

/// Fixed models in Magpie's order with their ids verbatim; the name is Magpie's display name,
/// or the id when there is none. A repeated id keeps its first entry.
fn parse_catalog(body: &str) -> Option<Vec<ModelInfo>> {
    let list: Value = serde_json::from_str(body).ok()?;
    let mut seen = HashSet::new();
    let mut models = Vec::new();
    for entry in list.get("data")?.as_array()? {
        let id = entry.get("id")?.as_str()?;
        if !is_fixed_model(id) || !seen.insert(id) {
            continue;
        }
        let name = entry
            .get("display_name")
            .and_then(Value::as_str)
            .filter(|name| !name.trim().is_empty())
            .unwrap_or(id);
        models.push(ModelInfo {
            id: id.to_string(),
            name: name.to_string(),
        });
    }
    Some(models)
}

/// The shared registry as Chat Completions function tools. `strict` is left out: Magpie
/// translates the request for many upstreams, and the executor validates arguments anyway.
fn function_tools() -> Vec<Value> {
    tools::definitions()
        .into_iter()
        .map(|def| {
            json!({
                "type": "function",
                "function": {
                    "name": def.name,
                    "description": def.description,
                    "parameters": def.parameters,
                },
            })
        })
        .collect()
}

fn chat_request(model: &str, messages: &[Value], tools: &[Value], web_search: bool) -> Value {
    let mut request = json!({"model": model, "stream": false, "messages": messages});
    if !tools.is_empty() {
        request["tools"] = json!(tools);
    }
    // Magpie decides how, and whether, to search; a Chat Completions reply cannot show it.
    if web_search {
        request["web_search_options"] = json!({});
    }
    request
}

/// Earlier turns as Chat Completions messages; an assistant message carries what its tools
/// really did (rendered as on the ChatGPT path).
fn history_messages(messages: &[ChatMessage]) -> impl Iterator<Item = Value> + '_ {
    messages.iter().map(|message| {
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
        json!({"role": message.role, "content": content})
    })
}

/// Magpie names the model that produced a non-streaming reply in `X-Magpie-Model`. Anything
/// but exactly the requested id (a fallback model, or no header) means the reply cannot be
/// attributed to the selected model, so none of it is used.
fn check_served_model(headers: &HeaderMap, requested: &str) -> Result<()> {
    let mut served = headers.get_all("x-magpie-model").iter();
    match (served.next(), served.next()) {
        (Some(model), None) if model.as_bytes() == requested.as_bytes() => Ok(()),
        (Some(model), None) => bail!(
            "Magpie answered with {} instead of the selected {requested} (for example through a fallback model), so the reply was not used.",
            String::from_utf8_lossy(model.as_bytes())
                .chars()
                .take(200)
                .collect::<String>()
        ),
        (None, _) => {
            bail!("Magpie did not report which model answered, so the reply was not used.")
        }
        (Some(_), Some(_)) => {
            bail!("Magpie reported more than one answering model, so the reply was not used.")
        }
    }
}

/// An HTTP error as the gateway described it (bounded; never the raw body), with a hint for
/// the statuses whose cause is known. Retry-After is only shown; nothing is retried.
fn gateway_error(status: StatusCode, body: &str, retry_after: Option<&str>) -> anyhow::Error {
    let json = serde_json::from_str::<Value>(body).unwrap_or(Value::Null);
    let reason = ["/error/message", "/error", "/message", "/detail"]
        .iter()
        .find_map(|field| {
            json.pointer(field)
                .and_then(Value::as_str)
                .filter(|reason| !reason.trim().is_empty())
        })
        .map(|reason| reason.chars().take(600).collect::<String>())
        .unwrap_or_else(|| {
            match status {
                StatusCode::NOT_FOUND => {
                    "Magpie does not offer this model now. Select another Magpie model in Settings."
                }
                StatusCode::FORBIDDEN => {
                    "Magpie refused the request; the model may be turned off in Magpie."
                }
                StatusCode::TOO_MANY_REQUESTS => "The model is rate limited.",
                _ => "Magpie could not complete the request.",
            }
            .to_string()
        });
    let retry = retry_after
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| match value.parse::<u64>() {
            Ok(seconds) => format!(" Try again in {seconds} s."),
            Err(_) => format!(
                " Try again after {}.",
                value.chars().take(64).collect::<String>()
            ),
        })
        .unwrap_or_default();
    anyhow!("Magpie request failed ({status}): {reason}{retry}")
}

fn request_error(error: reqwest::Error) -> anyhow::Error {
    if error.is_timeout() {
        anyhow!(
            "Magpie did not answer within {} seconds.",
            REQUEST_TIMEOUT.as_secs()
        )
    } else if error.is_connect() {
        anyhow!("Magpie could not be reached. Start Magpie and try again.")
    } else {
        anyhow!("Magpie request failed: {error}")
    }
}

/// One Chat Completions reply, checked as a whole before anything in it is used.
#[derive(Debug)]
struct Reply {
    text: String,
    calls: Vec<ToolCall>,
    /// The assistant message as received (with any reasoning fields, which Magpie reads back
    /// for the upstream), to send back with the tool results. Only `text` is shown.
    message: Value,
}

/// Only a reply that finished normally (`stop` or `tool_calls`; Magpie reports nothing else
/// for a complete reply) is used. One malformed tool call rejects the whole reply, so no tool
/// of that round runs. Arguments are not judged here; the executor validates them.
fn parse_reply(body: &Value) -> Result<Reply> {
    let choice = body
        .pointer("/choices/0")
        .filter(|choice| choice.is_object())
        .ok_or_else(|| anyhow!("Magpie returned a reply without a message."))?;
    match choice["finish_reason"].as_str() {
        Some("stop" | "tool_calls") => {}
        reason => bail!(
            "Magpie's reply did not finish normally ({}), so it was not used.",
            reason.map_or_else(
                || "no finish reason".to_string(),
                |reason| reason.chars().take(40).collect()
            )
        ),
    }
    let message = choice
        .get("message")
        .filter(|message| message["role"] == "assistant")
        .ok_or_else(|| anyhow!("Magpie returned a reply without a message."))?;
    let calls = match &message["tool_calls"] {
        Value::Null => Vec::new(),
        Value::Array(items) => items
            .iter()
            .map(tool_call)
            .collect::<Option<Vec<_>>>()
            .ok_or_else(|| anyhow!(MALFORMED_CALLS))?,
        _ => bail!(MALFORMED_CALLS),
    };
    let mut ids = HashSet::new();
    if !calls.iter().all(|call| ids.insert(call.id.as_str())) {
        bail!(MALFORMED_CALLS);
    }
    Ok(Reply {
        text: message["content"].as_str().unwrap_or_default().to_string(),
        message: message.clone(),
        calls,
    })
}

/// A call needs type `function` (Magpie always sends it), an id and a function name, both
/// non-empty and free of control characters; nothing is filled in for a missing one.
fn tool_call(item: &Value) -> Option<ToolCall> {
    let well_formed = |text: &&str| !text.trim().is_empty() && !text.chars().any(char::is_control);
    if item["type"] != "function" {
        return None;
    }
    let id = item["id"].as_str().filter(well_formed)?;
    let name = item["function"]["name"].as_str().filter(well_formed)?;
    Some(ToolCall {
        id: id.to_string(),
        name: name.to_string(),
        args: call_arguments(item["function"].get("arguments")),
    })
}

/// Chat Completions sends arguments as a JSON string; an empty one means no arguments.
/// Anything that is not a JSON object (`null`, an array or scalar, invalid JSON, a missing or
/// non-string field) reaches the executor as a string, which its validator rejects with
/// `invalid_argument`. As `Value::Null`, `null` would read as "no arguments" there.
fn call_arguments(raw: Option<&Value>) -> Value {
    match raw {
        Some(Value::String(text)) if text.trim().is_empty() => Value::Object(Default::default()),
        Some(Value::String(text)) => match serde_json::from_str(text) {
            Ok(Value::Object(object)) => Value::Object(object),
            _ => Value::String(text.clone()),
        },
        Some(other) => Value::String(other.to_string()),
        None => Value::String(String::new()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::Settings;
    use std::sync::atomic::AtomicBool;
    use std::sync::Arc;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};
    use tokio::sync::{Notify, RwLock};

    const MODEL: &str = "codex/gpt-5.5";

    /// One scripted turn of the loopback fake gateway (no upstream, no model behind it).
    struct Answer {
        status: u16,
        headers: Vec<String>,
        body: String,
        /// Never answer; wait until the client closes the connection.
        hold: bool,
        /// Close the connection without answering.
        hang_up: bool,
    }

    fn respond(status: u16, body: Value) -> Answer {
        Answer {
            status,
            headers: vec![],
            body: body.to_string(),
            hold: false,
            hang_up: false,
        }
    }

    fn hello() -> Answer {
        respond(200, json!({"name": "magpie", "version": "0.1.1267"}))
    }

    fn models(ids: &[&str]) -> Answer {
        let data: Vec<Value> = ids
            .iter()
            .map(|id| json!({"id": id, "object": "model"}))
            .collect();
        respond(200, json!({"object": "list", "data": data}))
    }

    /// A successful reply that Magpie attributes to `model`.
    fn served(model: &str, body: Value) -> Answer {
        Answer {
            headers: vec![format!("X-Magpie-Model: {model}")],
            ..respond(200, body)
        }
    }

    fn text_reply(text: &str) -> Value {
        json!({"object": "chat.completion", "model": "gpt-5.5", "choices": [{"index": 0,
            "finish_reason": "stop", "message": {"role": "assistant", "content": text}}]})
    }

    fn calls_reply(calls: Value) -> Value {
        json!({"object": "chat.completion", "model": "gpt-5.5", "choices": [{"index": 0,
            "finish_reason": "tool_calls",
            "message": {"role": "assistant", "content": null, "tool_calls": calls}}]})
    }

    fn call(id: &str, name: &str, arguments: &str) -> Value {
        json!({"id": id, "type": "function", "function": {"name": name, "arguments": arguments}})
    }

    #[derive(Debug, Clone)]
    struct Seen {
        line: String,
        head: String,
        body: String,
    }

    impl Seen {
        fn header(&self, name: &str) -> Option<String> {
            self.head.lines().skip(1).find_map(|line| {
                let (key, value) = line.split_once(':')?;
                key.trim()
                    .eq_ignore_ascii_case(name)
                    .then(|| value.trim().to_string())
            })
        }

        fn json(&self) -> Value {
            serde_json::from_str(&self.body).unwrap()
        }
    }

    struct Gateway {
        port: u16,
        seen: Arc<std::sync::Mutex<Vec<Seen>>>,
        /// Signalled when a held request arrived, and when its connection was closed.
        held: Arc<Notify>,
        closed: Arc<Notify>,
        dir: PathBuf,
        server: tokio::task::JoinHandle<()>,
    }

    impl Gateway {
        async fn start(answers: Vec<Answer>) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let port = listener.local_addr().unwrap().port();
            let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
            let (held, closed) = (Arc::new(Notify::new()), Arc::new(Notify::new()));
            let server = tokio::spawn(serve(
                listener,
                answers,
                seen.clone(),
                held.clone(),
                closed.clone(),
            ));
            let dir = std::env::temp_dir()
                .join(format!("expotify-magpie-test-{}", rand::random::<u64>()));
            std::fs::create_dir_all(dir.join("magpie")).unwrap();
            let gateway = Self {
                port,
                seen,
                held,
                closed,
                dir,
                server,
            };
            gateway.point_to(port);
            gateway
        }

        fn settings(&self) -> PathBuf {
            self.dir.join("magpie").join("settings.json")
        }

        /// Magpie's settings file, with other settings next to `port` as in the real one.
        fn point_to(&self, port: u16) {
            let settings = json!({"port": port, "proxy": "http://127.0.0.1:9", "searcher": "off"});
            std::fs::write(self.settings(), settings.to_string()).unwrap();
        }

        fn service(&self) -> MagpieService {
            MagpieService::with_settings_path(Some(self.settings()))
        }

        fn seen(&self) -> Vec<Seen> {
            self.seen.lock().unwrap().clone()
        }
    }

    impl Drop for Gateway {
        fn drop(&mut self) {
            self.server.abort();
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    async fn serve(
        listener: TcpListener,
        answers: Vec<Answer>,
        seen: Arc<std::sync::Mutex<Vec<Seen>>>,
        held: Arc<Notify>,
        closed: Arc<Notify>,
    ) {
        let mut answers = answers.into_iter();
        while let Ok((mut stream, _)) = listener.accept().await {
            let Some(request) = read_request(&mut stream).await else {
                continue;
            };
            seen.lock().unwrap().push(request);
            // A request beyond the script is recorded and refused, so a test notices it.
            let answer = answers.next().unwrap_or_else(|| {
                respond(500, json!({"error": {"message": "unexpected request"}}))
            });
            if answer.hang_up {
                continue;
            }
            if answer.hold {
                held.notify_one();
                let mut byte = [0u8; 1];
                while matches!(stream.read(&mut byte).await, Ok(read) if read > 0) {}
                closed.notify_one();
                continue;
            }
            let mut response = format!(
                "HTTP/1.1 {} Fixture\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n",
                answer.status,
                answer.body.len()
            );
            for header in &answer.headers {
                response.push_str(header);
                response.push_str("\r\n");
            }
            response.push_str("\r\n");
            response.push_str(&answer.body);
            let _ = stream.write_all(response.as_bytes()).await;
            let _ = stream.shutdown().await;
        }
    }

    async fn read_request(stream: &mut TcpStream) -> Option<Seen> {
        let mut head = Vec::new();
        let mut byte = [0u8; 1];
        while !head.ends_with(b"\r\n\r\n") {
            if stream.read(&mut byte).await.ok()? == 0 {
                return None;
            }
            head.push(byte[0]);
        }
        let head = String::from_utf8(head).ok()?;
        let length = head
            .lines()
            .find_map(|line| {
                let (key, value) = line.split_once(':')?;
                if key.trim().eq_ignore_ascii_case("content-length") {
                    value.trim().parse::<usize>().ok()
                } else {
                    None
                }
            })
            .unwrap_or(0);
        let mut body = vec![0u8; length];
        stream.read_exact(&mut body).await.ok()?;
        Some(Seen {
            line: head.lines().next()?.to_string(),
            body: String::from_utf8(body).ok()?,
            head,
        })
    }

    async fn free_port() -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        listener.local_addr().unwrap().port()
    }

    fn dry_runner() -> ToolRunner {
        ToolRunner::dry_run(Arc::new(AtomicBool::new(false)))
    }

    async fn chat(
        service: &MagpieService,
        runner: &mut ToolRunner,
        cancellation: &ChatCancellation,
        web_search: bool,
    ) -> Result<NativeChatOutcome> {
        let settings = RwLock::new(Settings::default());
        let spotify = RwLock::new(None);
        let ctx = ToolContext::new(&spotify, &settings, Some("spotify:track:current".into()));
        let messages = [ChatMessage {
            role: "user".into(),
            content: "音量调到42".into(),
            tool_results: vec![],
        }];
        service
            .agent_chat_native(
                &messages,
                MODEL,
                "You are a DJ.",
                "Song",
                "Artist",
                "Album",
                60,
                web_search,
                &[],
                &ctx,
                runner,
                cancellation,
            )
            .await
    }

    fn track() -> TrackInfo {
        TrackInfo {
            id: "spotify:track:1".into(),
            name: "Song".into(),
            artist: "Artist".into(),
            album: "Album".into(),
            album_art_url: None,
            duration_ms: 1,
            progress_ms: 0,
            is_playing: true,
            spotify_url: None,
            ai_description: None,
            ai_error: None,
            ai_used_web_search: false,
        }
    }

    #[tokio::test]
    async fn discovery_lists_fixed_models_in_magpie_order_with_two_authorised_gets() {
        let unsaveable = format!("vendor/{}", "x".repeat(200));
        let gateway = Gateway::start(vec![
            hello(),
            respond(
                200,
                json!({"object": "list", "data": [
                    {"id": "codex/gpt-5.5", "display_name": "GPT-5.5", "owned_by": "codex"},
                    {"id": "copilot/gpt-5.5", "display_name": "GPT-5.5", "owned_by": "copilot"},
                    {"id": "group/fast", "display_name": "Fast"},
                    {"id": "gpt-5.5"},
                    {"id": "copilot/auto", "display_name": "Auto"},
                    {"id": "claude/claude-sonnet-4-6"},
                    {"id": "vendor/family/model", "display_name": "   "},
                    {"id": unsaveable},
                    {"id": "vendor/bad\u{7}id"},
                    {"id": "codex/gpt-5.5", "display_name": "Repeated"},
                ]}),
            ),
        ])
        .await;
        let service = gateway.service();
        assert!(service.catalog().is_none(), "nothing before a discovery");
        assert!(!service.available());
        assert!(service.probe().await);
        assert!(service.available());
        let catalog = service.catalog().unwrap();
        assert_eq!(catalog.provider, ModelProvider::Magpie);
        assert_eq!(catalog.default_model, None);
        assert!(!catalog.stale);
        assert!(catalog.error.is_none());
        assert!(catalog.fetched_at.is_some());
        let listed: Vec<(&str, &str)> = catalog
            .models
            .iter()
            .map(|model| (model.id.as_str(), model.name.as_str()))
            .collect();
        assert_eq!(
            listed,
            vec![
                ("codex/gpt-5.5", "GPT-5.5"),
                ("copilot/gpt-5.5", "GPT-5.5"),
                ("claude/claude-sonnet-4-6", "claude/claude-sonnet-4-6"),
                ("vendor/family/model", "vendor/family/model"),
            ]
        );
        let seen = gateway.seen();
        assert_eq!(seen.len(), 2);
        assert!(seen[0].line.starts_with("GET /api/hello "));
        assert!(seen[1].line.starts_with("GET /v1/models "));
        for request in &seen {
            assert_eq!(
                request.header("authorization").as_deref(),
                Some("Bearer magpie-expotify")
            );
            assert_eq!(
                request.header("host"),
                Some(format!("127.0.0.1:{}", gateway.port))
            );
            for browser_or_proxy in ["origin", "sec-fetch-mode", "x-forwarded-for", "forwarded"] {
                assert!(
                    request.header(browser_or_proxy).is_none(),
                    "{browser_or_proxy}"
                );
            }
            assert!(!request
                .header("user-agent")
                .unwrap_or_default()
                .contains("Mozilla"));
        }
    }

    #[tokio::test]
    async fn an_empty_list_is_a_successful_discovery_without_models_or_default() {
        let gateway = Gateway::start(vec![hello(), models(&[])]).await;
        let service = gateway.service();
        assert!(!service.probe().await, "no fixed model to offer");
        let catalog = service.catalog().unwrap();
        assert!(catalog.models.is_empty());
        assert!(catalog.error.is_none());
        assert!(!catalog.stale);
        assert_eq!(catalog.default_model, None);
    }

    #[tokio::test]
    async fn absent_gateways_show_nothing_and_never_reach_the_model_list() {
        // Magpie was there, then quit: nothing listens on its port any more.
        let gateway = Gateway::start(vec![hello(), models(&[MODEL])]).await;
        let service = gateway.service();
        assert!(service.probe().await);
        gateway.point_to(free_port().await);
        let started = Instant::now();
        assert!(!service.probe().await);
        assert!(started.elapsed() < PROBE_TIMEOUT);
        assert!(service.catalog().is_none(), "no list and no error");
        assert!(!service.available());

        let other = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let elsewhere = format!(
            "Location: http://127.0.0.1:{}/api/hello",
            other.local_addr().unwrap().port()
        );
        for answer in [
            respond(200, json!({"name": "something-else"})),
            respond(200, json!({})),
            respond(404, json!({"error": "not found"})),
            respond(401, json!({})),
            respond(403, json!({"name": "magpie"})),
            Answer {
                body: "<html></html>".into(),
                ..respond(200, json!(null))
            },
            Answer {
                headers: vec![elsewhere.clone()],
                ..respond(302, json!({}))
            },
        ] {
            let gateway = Gateway::start(vec![answer]).await;
            let service = gateway.service();
            assert!(!service.probe().await);
            assert!(service.catalog().is_none());
            assert_eq!(gateway.seen().len(), 1, "the model list is never requested");
        }
        assert!(
            tokio::time::timeout(Duration::from_millis(200), other.accept())
                .await
                .is_err(),
            "the redirect was not followed"
        );
    }

    #[tokio::test]
    async fn a_failing_model_list_keeps_the_last_list_as_stale_with_a_reason() {
        let gateway = Gateway::start(vec![
            hello(),
            models(&[MODEL]),
            hello(),
            respond(401, json!({"error": {"message": "no"}})),
            hello(),
            Answer {
                body: r#"{"data": "wrong"}"#.into(),
                ..respond(200, json!(null))
            },
        ])
        .await;
        let service = gateway.service();
        assert!(service.probe().await);
        let fresh = service.catalog().unwrap();
        for expected in ["401", UNREADABLE_LIST] {
            assert!(!service.probe().await);
            assert!(!service.available());
            let stale = service.catalog().unwrap();
            assert!(stale.stale);
            assert!(
                stale.error.as_deref().unwrap().contains(expected),
                "{:?}",
                stale.error
            );
            assert_eq!(stale.models[0].id, MODEL);
            assert_eq!(stale.fetched_at, fresh.fetched_at);
        }
        assert_eq!(gateway.seen().len(), 6);

        // Without an earlier list there is nothing to keep, but the reason still shows.
        let gateway = Gateway::start(vec![hello(), respond(500, json!({}))]).await;
        let service = gateway.service();
        assert!(!service.probe().await);
        let catalog = service.catalog().unwrap();
        assert!(catalog.models.is_empty() && catalog.stale && catalog.fetched_at.is_none());
        assert!(catalog.error.unwrap().contains("500"));
    }

    #[tokio::test]
    async fn a_model_list_that_stalls_or_breaks_after_hello_keeps_the_last_list_as_stale() {
        let gateway = Gateway::start(vec![
            hello(),
            models(&[MODEL]),
            hello(),
            Answer {
                hold: true,
                ..respond(200, json!({}))
            },
            hello(),
            Answer {
                hang_up: true,
                ..respond(200, json!({}))
            },
        ])
        .await;
        let mut service = gateway.service();
        service.probe_timeout = Duration::from_millis(500);
        assert!(service.probe().await);
        for expected in ["in time", "stopped answering"] {
            assert!(!service.probe().await);
            let catalog = service
                .catalog()
                .expect("Magpie answered hello, so its last list stays");
            assert!(catalog.stale);
            assert!(
                catalog.error.as_deref().unwrap().contains(expected),
                "{:?}",
                catalog.error
            );
            assert_eq!(catalog.models[0].id, MODEL);
        }
    }

    #[tokio::test]
    async fn simultaneous_probes_share_one_discovery() {
        let gateway = Gateway::start(vec![hello(), models(&[MODEL])]).await;
        let service = gateway.service();
        let (first, second) = tokio::join!(service.probe(), service.probe());
        assert!(first && second);
        assert_eq!(gateway.seen().len(), 2);
    }

    #[test]
    fn the_port_comes_from_magpies_settings_file_which_is_only_read() {
        let dir =
            std::env::temp_dir().join(format!("expotify-magpie-port-{}", rand::random::<u64>()));
        std::fs::create_dir(&dir).unwrap();
        let path = dir.join("settings.json");
        for (content, port) in [
            (r#"{"port":4567,"proxy":"http://127.0.0.1:9"}"#, 4567),
            (r#"{"port":1024}"#, 1024),
            (r#"{"port":65535}"#, 65535),
            (r#"{"port":80}"#, 3425),
            (r#"{"port":0}"#, 3425),
            (r#"{"port":65536}"#, 3425),
            (r#"{"port":"4567"}"#, 3425),
            (r#"{"port":4567.5}"#, 3425),
            ("{}", 3425),
            ("not json", 3425),
        ] {
            std::fs::write(&path, content).unwrap();
            let modified = std::fs::metadata(&path).unwrap().modified().unwrap();
            assert_eq!(gateway_port(Some(&path)), port, "{content}");
            assert_eq!(std::fs::read_to_string(&path).unwrap(), content);
            assert_eq!(
                std::fs::metadata(&path).unwrap().modified().unwrap(),
                modified
            );
        }
        assert_eq!(gateway_port(Some(&dir.join("missing.json"))), 3425);
        assert_eq!(gateway_port(None), 3425);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn magpies_settings_live_under_xdg_config_home_or_dot_config() {
        let home = Some(PathBuf::from("/Users/someone"));
        assert_eq!(
            settings_file(Some("/custom/config".into()), home.clone()),
            Some(PathBuf::from("/custom/config/magpie/settings.json"))
        );
        for xdg in [None, Some(OsString::new())] {
            assert_eq!(
                settings_file(xdg, home.clone()),
                Some(PathBuf::from("/Users/someone/.config/magpie/settings.json"))
            );
        }
        assert_eq!(settings_file(None, None), None);
    }

    #[tokio::test]
    async fn a_tool_round_sends_the_real_result_back_with_the_whole_conversation() {
        let volume = call("call_1", "set_volume", r#"{"level":42}"#);
        let gateway = Gateway::start(vec![
            hello(),
            served(MODEL, calls_reply(json!([volume]))),
            served(MODEL, text_reply("Volume is 42.")),
        ])
        .await;
        let mut runner = dry_runner();
        let outcome = chat(
            &gateway.service(),
            &mut runner,
            &ChatCancellation::new(),
            true,
        )
        .await
        .unwrap();
        assert_eq!(outcome.text, "Volume is 42.");
        assert_eq!(outcome.tool_uses, 1);
        assert_eq!(outcome.turns, Some(2));
        assert_eq!(runner.dry_run_calls().len(), 1);
        assert_eq!(runner.dry_run_calls()[0].args, json!({"level": 42}));

        let seen = gateway.seen();
        assert_eq!(seen.len(), 3);
        assert!(seen[1].line.starts_with("POST /v1/chat/completions "));
        let first = seen[1].json();
        assert_eq!(first["model"], MODEL);
        assert_eq!(first["stream"], false);
        assert_eq!(first["web_search_options"], json!({}));
        assert!(first.get("previous_response_id").is_none());
        assert_eq!(first["messages"][0]["role"], "system");
        assert!(first["messages"][0]["content"]
            .as_str()
            .unwrap()
            .starts_with("You are a DJ."));
        assert_eq!(
            first["messages"][1],
            json!({"role": "user", "content": "音量调到42"})
        );
        let tools = first["tools"].as_array().unwrap();
        assert_eq!(tools.len(), tools::definitions().len());
        assert!(tools.iter().all(|tool| tool["type"] == "function"
            && tool.get("strict").is_none()
            && tool["function"].get("strict").is_none()));

        let second = seen[2].json();
        let messages = second["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 4);
        assert_eq!(
            &messages[..2],
            first["messages"].as_array().unwrap().as_slice()
        );
        assert_eq!(
            messages[2],
            json!({"role": "assistant", "content": null, "tool_calls": [volume]})
        );
        assert_eq!(
            messages[3],
            json!({"role": "tool", "tool_call_id": "call_1", "content": runner.outcomes()[0].output})
        );
        for request in &seen[1..] {
            assert_eq!(
                request.header("authorization").as_deref(),
                Some("Bearer magpie-expotify")
            );
            assert!(request.header("chatgpt-account-id").is_none());
        }
    }

    #[tokio::test]
    async fn calls_answer_in_order_and_a_repeated_call_id_does_not_run_again() {
        let volume = call("call_1", "set_volume", r#"{"level":30}"#);
        let like = call("call_2", "like_current", "");
        let gateway = Gateway::start(vec![
            hello(),
            served(MODEL, calls_reply(json!([volume, like]))),
            served(MODEL, calls_reply(json!([volume]))),
            served(MODEL, text_reply("Done.")),
        ])
        .await;
        let mut runner = dry_runner();
        let outcome = chat(
            &gateway.service(),
            &mut runner,
            &ChatCancellation::new(),
            false,
        )
        .await
        .unwrap();
        assert_eq!(outcome.text, "Done.");
        let ran: Vec<&str> = runner
            .dry_run_calls()
            .iter()
            .map(|call| call.call_id.as_str())
            .collect();
        assert_eq!(ran, vec!["call_1", "call_2"], "call_1 ran once");
        let results = runner.outcomes();
        let messages = gateway.seen()[3].json()["messages"].clone();
        let replies: Vec<(&str, &str)> = messages
            .as_array()
            .unwrap()
            .iter()
            .filter(|message| message["role"] == "tool")
            .map(|message| {
                (
                    message["tool_call_id"].as_str().unwrap(),
                    message["content"].as_str().unwrap(),
                )
            })
            .collect();
        assert_eq!(
            replies,
            vec![
                ("call_1", results[0].output.as_str()),
                ("call_2", results[1].output.as_str()),
                ("call_1", results[0].output.as_str()),
            ]
        );
    }

    #[tokio::test]
    async fn cancelling_after_a_tool_ran_keeps_its_result_and_runs_nothing_more() {
        let gateway = Gateway::start(vec![
            hello(),
            served(
                MODEL,
                calls_reply(json!([call("call_1", "set_volume", r#"{"level":42}"#)])),
            ),
            Answer {
                hold: true,
                ..respond(200, json!({}))
            },
        ])
        .await;
        let service = gateway.service();
        let mut runner = dry_runner();
        let cancellation = ChatCancellation::new();
        let canceller = cancellation.clone();
        let held = gateway.held.clone();
        let (result, ()) = tokio::join!(
            chat(&service, &mut runner, &cancellation, false),
            async move {
                held.notified().await;
                canceller.cancel();
            }
        );
        assert_eq!(result.unwrap_err().to_string(), tools::CANCELLED);
        let results = runner.outcomes();
        assert_eq!(results.len(), 1);
        assert!(results[0].ok);
        assert_eq!(runner.dry_run_calls().len(), 1);
        assert_eq!(gateway.seen().len(), 3, "nothing is sent after the cancel");
    }

    #[tokio::test]
    async fn the_round_limit_stops_tools_and_text_never_becomes_an_action() {
        let gateway = Gateway::start(vec![
            hello(),
            served(
                MODEL,
                calls_reply(json!([call("call_1", "like_current", "")])),
            ),
            served(
                MODEL,
                calls_reply(json!([call("call_2", "unlike_current", "")])),
            ),
            served(
                MODEL,
                calls_reply(json!([call("call_3", "shuffle_liked", "")])),
            ),
        ])
        .await;
        let mut runner = dry_runner();
        let error = chat(
            &gateway.service(),
            &mut runner,
            &ChatCancellation::new(),
            false,
        )
        .await
        .unwrap_err()
        .to_string();
        assert!(error.contains("after 3 rounds"), "{error}");
        assert_eq!(
            runner.dry_run_calls().len(),
            2,
            "the third round's call never ran"
        );
        assert_eq!(gateway.seen().len(), 4);

        let action = r#"{"action":"set_volume","args":{"level":5},"message":"Done"}"#;
        let gateway = Gateway::start(vec![hello(), served(MODEL, text_reply(action))]).await;
        let mut runner = dry_runner();
        let outcome = chat(
            &gateway.service(),
            &mut runner,
            &ChatCancellation::new(),
            false,
        )
        .await
        .unwrap();
        assert_eq!(outcome.text, action);
        assert!(runner.outcomes().is_empty(), "plain text is never executed");
    }

    #[tokio::test]
    async fn the_assistant_message_goes_back_whole_with_its_reasoning() {
        let message = json!({"role": "assistant", "content": "Setting it now.",
            "reasoning_content": "The user wants 42.",
            "reasoning_details": [{"type": "reasoning.text", "text": "42"}],
            "tool_calls": [call("call_1", "set_volume", r#"{"level":42}"#)]});
        let gateway = Gateway::start(vec![
            hello(),
            served(
                MODEL,
                json!({"choices": [{"index": 0, "finish_reason": "tool_calls", "message": message}]}),
            ),
            served(MODEL, text_reply("Done.")),
        ])
        .await;
        let mut runner = dry_runner();
        let outcome = chat(
            &gateway.service(),
            &mut runner,
            &ChatCancellation::new(),
            false,
        )
        .await
        .unwrap();
        assert_eq!(outcome.text, "Done.", "reasoning is never shown");
        assert_eq!(gateway.seen()[2].json()["messages"][2], message);
    }

    #[tokio::test]
    async fn replies_not_attributed_to_the_requested_model_run_nothing() {
        let header = |model: &str| format!("X-Magpie-Model: {model}");
        for headers in [
            vec![],
            vec![header("copilot/gpt-5.5")],
            vec![header("Codex/gpt-5.5")],
            vec![header("codex/gpt-5.5-mini")],
            vec![header(MODEL), header(MODEL)],
        ] {
            let gateway = Gateway::start(vec![
                hello(),
                Answer {
                    headers: headers.clone(),
                    ..respond(
                        200,
                        calls_reply(json!([call("call_1", "like_current", "{}")])),
                    )
                },
            ])
            .await;
            let mut runner = dry_runner();
            let error = chat(
                &gateway.service(),
                &mut runner,
                &ChatCancellation::new(),
                false,
            )
            .await
            .unwrap_err()
            .to_string();
            assert!(error.contains("was not used"), "{error}");
            assert!(runner.outcomes().is_empty(), "{headers:?}");
            assert_eq!(gateway.seen().len(), 2, "nothing is retried");
        }
    }

    #[tokio::test]
    async fn a_substituted_later_round_keeps_earlier_results_and_runs_nothing_new() {
        let gateway = Gateway::start(vec![
            hello(),
            served(
                MODEL,
                calls_reply(json!([call("call_1", "set_volume", r#"{"level":42}"#)])),
            ),
            served(
                "copilot/gpt-5.5",
                calls_reply(json!([call("call_2", "like_current", "{}")])),
            ),
        ])
        .await;
        let mut runner = dry_runner();
        let error = chat(
            &gateway.service(),
            &mut runner,
            &ChatCancellation::new(),
            false,
        )
        .await
        .unwrap_err()
        .to_string();
        assert!(error.contains("copilot/gpt-5.5"), "{error}");
        let results = runner.outcomes();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "set_volume");
        assert_eq!(runner.dry_run_calls().len(), 1);
        assert_eq!(
            gateway.seen().len(),
            3,
            "the refused round is not sent again"
        );
    }

    #[tokio::test]
    async fn the_header_decides_whatever_model_the_reply_body_names() {
        // Magpie's MemberModel setting may put either name into the body.
        for body_model in ["gpt-5.5", MODEL] {
            let mut body = text_reply("Hi.");
            body["model"] = json!(body_model);
            let gateway = Gateway::start(vec![hello(), served(MODEL, body)]).await;
            let outcome = chat(
                &gateway.service(),
                &mut dry_runner(),
                &ChatCancellation::new(),
                false,
            )
            .await
            .unwrap();
            assert_eq!(outcome.text, "Hi.");
        }
        let mut body = text_reply("Hi.");
        body["model"] = json!(MODEL);
        let gateway = Gateway::start(vec![hello(), respond(200, body)]).await;
        assert!(
            chat(
                &gateway.service(),
                &mut dry_runner(),
                &ChatCancellation::new(),
                false
            )
            .await
            .is_err(),
            "a matching body never stands in for the header"
        );
    }

    #[tokio::test]
    async fn http_errors_are_reported_as_given_and_never_retried() {
        let cases = [
            (
                Answer {
                    headers: vec!["Retry-After: 7".into()],
                    ..respond(429, json!({"error": {"message": "upstream rate limit"}}))
                },
                vec!["429", "upstream rate limit", "Try again in 7 s"],
            ),
            (
                respond(
                    403,
                    json!({"error": {"message": "codex/gpt-5.5 is turned off"}}),
                ),
                vec!["403", "codex/gpt-5.5 is turned off"],
            ),
            (
                respond(404, json!({})),
                vec!["404", "does not offer this model"],
            ),
            (
                respond(
                    400,
                    json!({"error": {"code": "too_long"}, "message": "context too long"}),
                ),
                vec!["400", "context too long"],
            ),
            (
                Answer {
                    body: "<html>proxy page</html>".into(),
                    ..respond(502, json!(null))
                },
                vec!["502", "could not complete"],
            ),
        ];
        for (answer, expected) in cases {
            let gateway = Gateway::start(vec![hello(), answer]).await;
            let mut runner = dry_runner();
            let error = chat(
                &gateway.service(),
                &mut runner,
                &ChatCancellation::new(),
                false,
            )
            .await
            .unwrap_err()
            .to_string();
            for part in expected {
                assert!(error.contains(part), "{error}");
            }
            assert!(!error.contains("<html>"), "{error}");
            assert!(
                !error.contains("which model"),
                "the HTTP error comes first: {error}"
            );
            assert!(runner.outcomes().is_empty());
            assert_eq!(gateway.seen().len(), 2, "nothing is retried");
        }
    }

    #[tokio::test]
    async fn unfinished_or_malformed_rounds_run_no_tool() {
        let valid = call("call_1", "set_volume", r#"{"level":42}"#);
        let finishing = |reason: Value| {
            json!({"choices": [{"index": 0, "finish_reason": reason,
                "message": {"role": "assistant", "content": null, "tool_calls": [valid]}}]})
        };
        let mut bodies = vec![
            finishing(json!("length")),
            finishing(json!("content_filter")),
            finishing(json!("end_turn")),
            finishing(json!(null)),
            json!({"choices": [{"index": 0,
                "message": {"role": "assistant", "content": null, "tool_calls": [valid]}}]}),
            json!({"choices": [{"index": 0, "finish_reason": "tool_calls",
                "message": {"role": "tool", "content": null, "tool_calls": [valid]}}]}),
            json!({"choices": []}),
            calls_reply(json!({"id": "call_1"})),
        ];
        for broken in [
            json!({"type": "function", "function": {"name": "like_current", "arguments": "{}"}}),
            call(" ", "like_current", "{}"),
            call("call_2", "", "{}"),
            call("call\n2", "like_current", "{}"),
            call("call_2", "like_current\u{0}", "{}"),
            json!({"id": "call_2", "type": "function"}),
            json!({"id": "call_2", "function": {"name": "like_current", "arguments": "{}"}}),
            json!({"id": "call_2", "type": "custom",
                "function": {"name": "like_current", "arguments": "{}"}}),
            call("call_1", "like_current", "{}"),
        ] {
            bodies.push(calls_reply(json!([valid, broken])));
        }
        for body in bodies {
            let gateway = Gateway::start(vec![hello(), served(MODEL, body.clone())]).await;
            let mut runner = dry_runner();
            let error = chat(
                &gateway.service(),
                &mut runner,
                &ChatCancellation::new(),
                false,
            )
            .await
            .unwrap_err();
            assert!(runner.outcomes().is_empty(), "{body}: {error}");
        }
        // A text reply that did not finish normally is not shown either.
        let gateway = Gateway::start(vec![
            hello(),
            served(
                MODEL,
                json!({"choices": [{"index": 0, "finish_reason": "length",
                    "message": {"role": "assistant", "content": "Half a sen"}}]}),
            ),
        ])
        .await;
        let error = chat(
            &gateway.service(),
            &mut dry_runner(),
            &ChatCancellation::new(),
            false,
        )
        .await
        .unwrap_err()
        .to_string();
        assert!(
            error.contains("did not finish normally (length)"),
            "{error}"
        );
    }

    #[test]
    fn arguments_that_are_not_an_object_never_reach_a_tool_as_no_arguments() {
        let validate = |raw: Option<Value>| {
            tools::validate(&ToolCall {
                id: "c".into(),
                name: tools::LIKE_CURRENT.into(),
                args: call_arguments(raw.as_ref()),
            })
        };
        for raw in [
            json!("null"),
            json!("[]"),
            json!("[1]"),
            json!("5"),
            json!("true"),
            json!("\"x\""),
            json!("{"),
            json!(null),
            json!({}),
            json!([]),
            json!(5),
        ] {
            assert_eq!(
                validate(Some(raw.clone())).unwrap_err().code,
                "invalid_argument",
                "{raw}"
            );
        }
        assert_eq!(validate(None).unwrap_err().code, "invalid_argument");
        for empty in ["", "  ", "{}"] {
            assert!(validate(Some(json!(empty))).is_ok(), "{empty:?}");
        }
        assert_eq!(
            call_arguments(Some(&json!(r#"{"level":42}"#))),
            json!({"level": 42})
        );
    }

    #[tokio::test]
    async fn a_null_argument_is_fed_back_as_a_failure_and_nothing_runs() {
        let gateway = Gateway::start(vec![
            hello(),
            served(
                MODEL,
                calls_reply(json!([call("call_1", "like_current", "null")])),
            ),
            served(MODEL, text_reply("I could not like it.")),
        ])
        .await;
        let mut runner = dry_runner();
        let outcome = chat(
            &gateway.service(),
            &mut runner,
            &ChatCancellation::new(),
            false,
        )
        .await
        .unwrap();
        assert_eq!(outcome.text, "I could not like it.");
        let results = runner.outcomes();
        assert_eq!(results.len(), 1);
        assert!(!results[0].ok);
        assert_eq!(results[0].error_code.as_deref(), Some("invalid_argument"));
        assert!(
            runner.dry_run_calls().is_empty(),
            "nothing was accepted to run"
        );
        assert_eq!(
            gateway.seen()[2].json()["messages"][3]["content"],
            results[0].output
        );
    }

    #[tokio::test]
    async fn cancelling_closes_the_request_and_nothing_runs_afterwards() {
        let gateway = Gateway::start(vec![
            hello(),
            Answer {
                hold: true,
                ..respond(200, json!({}))
            },
        ])
        .await;
        let service = gateway.service();
        let mut runner = dry_runner();
        let cancellation = ChatCancellation::new();
        let canceller = cancellation.clone();
        let held = gateway.held.clone();
        let started = Instant::now();
        let (result, ()) = tokio::join!(
            chat(&service, &mut runner, &cancellation, false),
            async move {
                held.notified().await;
                canceller.cancel();
            }
        );
        assert_eq!(result.unwrap_err().to_string(), tools::CANCELLED);
        tokio::time::timeout(Duration::from_secs(5), gateway.closed.notified())
            .await
            .expect("the gateway saw the request's connection close");
        assert!(started.elapsed() < Duration::from_secs(10));
        assert!(runner.outcomes().is_empty());
        assert_eq!(gateway.seen().len(), 2, "nothing is sent again");
    }

    #[tokio::test]
    async fn requests_fail_before_generating_when_magpie_is_not_on_the_port() {
        let gateway = Gateway::start(vec![respond(200, json!({"name": "something-else"}))]).await;
        let mut runner = dry_runner();
        let error = chat(
            &gateway.service(),
            &mut runner,
            &ChatCancellation::new(),
            false,
        )
        .await
        .unwrap_err()
        .to_string();
        assert!(error.contains("is not Magpie"), "{error}");
        assert_eq!(gateway.seen().len(), 1, "nothing was generated");
        let port = free_port().await;
        gateway.point_to(port);
        let error = chat(
            &gateway.service(),
            &mut runner,
            &ChatCancellation::new(),
            false,
        )
        .await
        .unwrap_err()
        .to_string();
        assert!(
            error.contains(&format!("could not be reached on 127.0.0.1:{port}")),
            "{error}"
        );
        assert!(runner.outcomes().is_empty());
    }

    #[test]
    fn only_fixed_models_are_sent_and_there_is_no_default() {
        let service = MagpieService::with_settings_path(None);
        let fixed = |model: &str| ModelSelection::Fixed {
            provider: ModelProvider::Magpie,
            model: model.into(),
        };
        for model in [MODEL, "vendor/family/model"] {
            assert_eq!(service.resolve_model(&fixed(model)).unwrap(), model);
        }
        for model in ["group/fast", "gpt-5.5", "copilot/auto"] {
            let error = service
                .resolve_model(&fixed(model))
                .unwrap_err()
                .to_string();
            assert!(
                error.contains("Select a fixed Magpie model"),
                "{model}: {error}"
            );
        }
        let default = ModelSelection::Default {
            provider: ModelProvider::Magpie,
        };
        assert!(service
            .resolve_model(&default)
            .unwrap_err()
            .to_string()
            .contains("no default model"));
    }

    #[tokio::test]
    async fn insight_uses_one_attributed_reply_and_never_claims_a_search() {
        let gateway = Gateway::start(vec![
            hello(),
            served(MODEL, text_reply("A song about rain.")),
            hello(),
            served("copilot/gpt-5.5", text_reply("Another model's answer.")),
        ])
        .await;
        let service = gateway.service();
        let track = track();
        let template = "About {name} by {artist} ({album}). {memories}";
        assert_eq!(
            service
                .get_track_description(
                    &track,
                    MODEL,
                    template,
                    false,
                    false,
                    &["Likes jazz".into()]
                )
                .await
                .unwrap(),
            ("A song about rain.".to_string(), false)
        );
        let request = gateway.seen()[1].json();
        assert_eq!(
            request["messages"][0],
            json!({"role": "system", "content": TRACK_SYSTEM_PROMPT})
        );
        assert_eq!(
            request["messages"][1]["content"],
            "About Song by Artist (Album). User memories:\n1. Likes jazz"
        );
        assert!(request.get("tools").is_none());
        assert!(request.get("web_search_options").is_none());

        let cached = service
            .get_track_description(&track, MODEL, template, false, false, &[])
            .await
            .unwrap();
        assert_eq!(cached.0, "A song about rain.");
        assert_eq!(gateway.seen().len(), 2, "served from the cache");

        let error = service
            .get_track_description(&track, MODEL, template, true, true, &[])
            .await
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("instead of the selected codex/gpt-5.5"),
            "{error}"
        );
        assert_eq!(gateway.seen()[3].json()["web_search_options"], json!({}));
    }

    #[tokio::test]
    async fn every_request_reads_the_port_again() {
        let first = Gateway::start(vec![hello(), served(MODEL, text_reply("First port."))]).await;
        let second = Gateway::start(vec![hello(), served(MODEL, text_reply("Second port."))]).await;
        let service = first.service();
        let track = track();
        let insight = || service.get_track_description(&track, MODEL, "x", false, true, &[]);
        assert_eq!(insight().await.unwrap().0, "First port.");
        // Magpie moved to another port without a restart.
        first.point_to(second.port);
        assert_eq!(insight().await.unwrap().0, "Second port.");
        assert_eq!(first.seen().len(), 2);
        assert_eq!(second.seen().len(), 2);
    }
}
