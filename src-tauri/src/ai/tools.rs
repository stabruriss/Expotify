//! Shared chat tool layer: the single registry of tools the assistant may call,
//! argument validation, and the executor that performs them.
//!
//! Both providers feed this layer. In the legacy protocol the call comes from
//! `parse_agent_response` (a JSON object the model wrote as text); in the native
//! protocols it comes from the provider's structured tool-call item. Either way:
//! every branch yields an explicit, model-readable success or failure, a call id is
//! executed at most once, and nothing is reported as done unless the executor did it.

use super::events::{self, EventContext};
use crate::spotify::{self, SearchResult, SpotifyWebApi};
use crate::storage::Settings;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::RwLock;

pub const SEARCH_AND_PLAY: &str = "search_and_play";
pub const LIKE_CURRENT: &str = "like_current";
pub const UNLIKE_CURRENT: &str = "unlike_current";
pub const SHUFFLE_LIKED: &str = "shuffle_liked";
pub const SET_VOLUME: &str = "set_volume";
pub const SAVE_MEMORY: &str = "save_memory";
pub const UPDATE_PROMPT: &str = "update_prompt";

#[cfg(test)]
pub const TOOL_NAMES: [&str; 7] = [
    SEARCH_AND_PLAY,
    LIKE_CURRENT,
    UNLIKE_CURRENT,
    SHUFFLE_LIKED,
    SET_VOLUME,
    SAVE_MEMORY,
    UPDATE_PROMPT,
];

/// Non-tool actions the legacy text protocol may produce.
const CONVERSATIONAL_ACTIONS: [&str; 3] = ["reply", "ask", "refuse"];

/// How tool calls are obtained from a provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ToolProtocol {
    /// Prompt convention: the model writes a JSON object as text; we parse it. Kept for
    /// diagnosis only (no UI entry; set `tool_protocol` in settings.json by hand).
    Legacy,
    /// Provider-native tool calling (function_call / tool_use). The default.
    #[default]
    Native,
}

impl ToolProtocol {
    pub fn label(self) -> &'static str {
        match self {
            Self::Legacy => "legacy",
            Self::Native => "native",
        }
    }
}

/// Per-provider protocol choice, decided before a request starts and never switched
/// mid-request. Both default to native tool calling.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ToolProtocolSettings {
    #[serde(default)]
    pub anthropic: ToolProtocol,
    #[serde(default)]
    pub openai: ToolProtocol,
}

/// Cancellation handle for one chat request: the flag stops further tool execution,
/// the notifier aborts the in-flight provider call.
#[derive(Clone)]
pub struct ChatCancellation {
    pub flag: Arc<AtomicBool>,
    pub notify: Arc<tokio::sync::Notify>,
}

impl ChatCancellation {
    pub fn new() -> Self {
        Self {
            flag: Arc::new(AtomicBool::new(false)),
            notify: Arc::new(tokio::sync::Notify::new()),
        }
    }

    pub fn cancel(&self) {
        self.flag.store(true, Ordering::SeqCst);
        self.notify.notify_waiters();
        self.notify.notify_one();
    }

    pub fn is_cancelled(&self) -> bool {
        self.flag.load(Ordering::SeqCst)
    }

    /// Resolves once the request is cancelled, including when `cancel` ran before the wait.
    pub async fn cancelled(&self) {
        loop {
            if self.is_cancelled() {
                return;
            }
            self.notify.notified().await;
        }
    }
}

/// Error message of a request that ended because the user cancelled it.
pub const CANCELLED: &str = "Cancelled";

impl Default for ChatCancellation {
    fn default() -> Self {
        Self::new()
    }
}

/// Provider-neutral tool definition. `parameters` is a strict-compatible JSON Schema
/// (every property required, `additionalProperties: false`).
#[derive(Debug, Clone, Serialize)]
pub struct ToolDefinition {
    pub name: &'static str,
    pub description: &'static str,
    pub parameters: Value,
}

fn object_schema(properties: Value, required: &[&str]) -> Value {
    json!({
        "type": "object",
        "properties": properties,
        "required": required,
        "additionalProperties": false,
    })
}

pub fn definitions() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition {
            name: SEARCH_AND_PLAY,
            description: "Search Spotify for a song, artist, album, genre or mood and immediately play the best match. Use this whenever the user wants music played. When the user names an artist, pass it in `artist` as well: only a result credited to exactly that artist is played; otherwise nothing plays and the closest matches come back for the user to confirm.",
            parameters: object_schema(
                json!({
                    "query": { "type": "string", "description": "Search text built from the user's request, including the artist when one was named, e.g. 'Fly Me to the Moon Frank Sinatra', '晴天 周杰伦' or 'rainy day jazz'" },
                    "artist": { "type": "string", "description": "The artist's full name as Spotify credits it (e.g. 'Frank Sinatra', not 'Sinatra'); an empty string when no artist was named" }
                }),
                &["query", "artist"],
            ),
        },
        ToolDefinition {
            name: LIKE_CURRENT,
            description: "Add the currently playing song to the user's Liked Songs.",
            parameters: object_schema(json!({}), &[]),
        },
        ToolDefinition {
            name: UNLIKE_CURRENT,
            description: "Remove the currently playing song from the user's Liked Songs.",
            parameters: object_schema(json!({}), &[]),
        },
        ToolDefinition {
            name: SHUFFLE_LIKED,
            description: "Play a random song from the user's Liked Songs.",
            parameters: object_schema(json!({}), &[]),
        },
        ToolDefinition {
            name: SET_VOLUME,
            description: "Set the Spotify playback volume to an absolute level between 0 and 100.",
            parameters: object_schema(
                json!({ "level": { "type": "integer", "minimum": 0, "maximum": 100, "description": "Target volume, 0-100" } }),
                &["level"],
            ),
        },
        ToolDefinition {
            name: SAVE_MEMORY,
            description: "Save a durable note about the user's music preferences or interests for future conversations.",
            parameters: object_schema(
                json!({ "content": { "type": "string", "description": "One concise sentence describing the preference" } }),
                &["content"],
            ),
        },
        ToolDefinition {
            name: UPDATE_PROMPT,
            description: "Update the AI Insight prompt ('insight') or the Chat prompt ('chat'): replace it entirely, or append to it when the user wants to keep the existing prompt (you cannot read the current prompt, so use 'append' for additions).",
            parameters: object_schema(
                json!({
                    "type": { "type": "string", "enum": ["insight", "chat"], "description": "Which prompt to update" },
                    "content": { "type": "string", "description": "With mode 'replace': the full new prompt text. With mode 'append': only the text to add at the end." },
                    "mode": { "type": "string", "enum": ["replace", "append"], "description": "'replace' overwrites the whole prompt; 'append' keeps the current prompt and adds the content after it" }
                }),
                &["type", "content", "mode"],
            ),
        },
    ]
}

/// A tool call requested by the model, in either protocol.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub args: Value,
}

impl ToolCall {
    /// Convert a parsed legacy response into a call. Conversational actions yield `None`.
    /// Any other action string (including unknown ones) becomes a call so the executor
    /// can report it instead of silently ignoring it.
    pub fn from_legacy(response: &super::AgentResponse, sequence: usize) -> Option<ToolCall> {
        let action = response.action.trim();
        if action.is_empty() || CONVERSATIONAL_ACTIONS.contains(&action) {
            return None;
        }
        Some(ToolCall {
            id: format!("legacy-{sequence}"),
            name: action.to_string(),
            args: response.args.clone(),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PromptKind {
    Insight,
    Chat,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PromptMode {
    Replace,
    Append,
}

/// A validated, typed invocation.
#[derive(Debug, Clone, PartialEq)]
pub enum Invocation {
    SearchAndPlay {
        query: String,
        artist: String,
    },
    LikeCurrent,
    UnlikeCurrent,
    ShuffleLiked,
    SetVolume {
        level: u32,
    },
    SaveMemory {
        content: String,
    },
    UpdatePrompt {
        kind: PromptKind,
        content: String,
        mode: PromptMode,
    },
}

impl Invocation {
    fn uses_spotify_web_api(&self) -> bool {
        matches!(
            self,
            Invocation::SearchAndPlay { .. }
                | Invocation::LikeCurrent
                | Invocation::UnlikeCurrent
                | Invocation::ShuffleLiked
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ToolError {
    pub code: &'static str,
    pub message: String,
}

impl ToolError {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

fn required_string(args: &Value, key: &str) -> Result<String, ToolError> {
    match args.get(key) {
        Some(Value::String(s)) if !s.trim().is_empty() => Ok(s.trim().to_string()),
        Some(Value::String(_)) => Err(ToolError::new(
            "invalid_argument",
            format!("`{key}` must not be empty"),
        )),
        Some(_) => Err(ToolError::new(
            "invalid_argument",
            format!("`{key}` must be a string"),
        )),
        None => Err(ToolError::new(
            "missing_argument",
            format!("`{key}` is required"),
        )),
    }
}

/// A string argument that may be absent (legacy callers): absent or null reads as empty.
fn optional_string(args: &Value, key: &str) -> Result<String, ToolError> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(String::new()),
        Some(Value::String(s)) => Ok(s.trim().to_string()),
        Some(_) => Err(ToolError::new(
            "invalid_argument",
            format!("`{key}` must be a string"),
        )),
    }
}

/// Validate a call against the registry. Numeric arguments must be JSON numbers
/// (an integral float such as 50.0 is accepted; a numeric string is not).
pub fn validate(call: &ToolCall) -> Result<Invocation, ToolError> {
    let args = if call.args.is_null() {
        Value::Object(Default::default())
    } else {
        call.args.clone()
    };
    if !args.is_object() {
        return Err(ToolError::new(
            "invalid_argument",
            "arguments must be a JSON object",
        ));
    }
    match call.name.as_str() {
        SEARCH_AND_PLAY => Ok(Invocation::SearchAndPlay {
            query: required_string(&args, "query")?,
            artist: optional_string(&args, "artist")?,
        }),
        LIKE_CURRENT => Ok(Invocation::LikeCurrent),
        UNLIKE_CURRENT => Ok(Invocation::UnlikeCurrent),
        SHUFFLE_LIKED => Ok(Invocation::ShuffleLiked),
        SET_VOLUME => {
            let level = match args.get("level") {
                Some(Value::Number(n)) => n.as_u64().or_else(|| {
                    n.as_f64()
                        .filter(|f| f.fract() == 0.0 && *f >= 0.0)
                        .map(|f| f as u64)
                }),
                Some(_) => None,
                None => {
                    return Err(ToolError::new("missing_argument", "`level` is required"));
                }
            };
            match level {
                Some(l) if l <= 100 => Ok(Invocation::SetVolume { level: l as u32 }),
                _ => Err(ToolError::new(
                    "invalid_argument",
                    "`level` must be an integer between 0 and 100",
                )),
            }
        }
        SAVE_MEMORY => Ok(Invocation::SaveMemory {
            content: required_string(&args, "content")?,
        }),
        UPDATE_PROMPT => {
            let kind = match required_string(&args, "type")?.as_str() {
                "insight" => PromptKind::Insight,
                "chat" => PromptKind::Chat,
                other => {
                    return Err(ToolError::new(
                        "invalid_argument",
                        format!("`type` must be 'insight' or 'chat', got '{other}'"),
                    ))
                }
            };
            let mode = match optional_string(&args, "mode")?.as_str() {
                "" | "replace" => PromptMode::Replace,
                "append" => PromptMode::Append,
                other => {
                    return Err(ToolError::new(
                        "invalid_argument",
                        format!("`mode` must be 'replace' or 'append', got '{other}'"),
                    ))
                }
            };
            Ok(Invocation::UpdatePrompt {
                kind,
                content: required_string(&args, "content")?,
                mode,
            })
        }
        other => Err(ToolError::new(
            "unknown_tool",
            format!("'{other}' is not an available tool"),
        )),
    }
}

/// The result of one tool call. `output` is the text the model (and the UI) reads.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolOutcome {
    pub call_id: String,
    pub name: String,
    pub ok: bool,
    pub output: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub track_name: Option<String>,
}

impl ToolOutcome {
    /// A failure outcome for a call that was not (or could not be) executed.
    pub fn failed(call: &ToolCall, code: &str, message: impl Into<String>) -> Self {
        Self::failure(call, code, message)
    }

    fn success(call: &ToolCall, output: impl Into<String>) -> Self {
        Self {
            call_id: call.id.clone(),
            name: call.name.clone(),
            ok: true,
            output: output.into(),
            error_code: None,
            track_name: None,
        }
    }

    fn failure(call: &ToolCall, code: &str, message: impl Into<String>) -> Self {
        Self {
            call_id: call.id.clone(),
            name: call.name.clone(),
            ok: false,
            output: message.into(),
            error_code: Some(code.to_string()),
            track_name: None,
        }
    }
}

/// Which track "the current song" means while a request runs. It starts as the track that
/// was playing when the request began and follows the executor's own confirmed playback
/// changes, so "play X, then like it" likes X, and a failed playback change never lets a
/// later like/unlike hit the wrong song.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrackFocus {
    NothingPlaying,
    Playing(String),
    /// A playback change in this request failed; which song is meant is unclear now.
    Unconfirmed,
}

/// Everything the executor needs; borrowed from `AppState` so this module does not
/// depend on the Tauri command layer.
pub struct ToolContext<'a> {
    pub spotify_webapi: &'a RwLock<Option<SpotifyWebApi>>,
    pub settings: &'a RwLock<Settings>,
    focus: std::sync::Mutex<TrackFocus>,
}

impl<'a> ToolContext<'a> {
    pub fn new(
        spotify_webapi: &'a RwLock<Option<SpotifyWebApi>>,
        settings: &'a RwLock<Settings>,
        current_track_id: Option<String>,
    ) -> Self {
        let focus = current_track_id
            .map(TrackFocus::Playing)
            .unwrap_or(TrackFocus::NothingPlaying);
        Self {
            spotify_webapi,
            settings,
            focus: std::sync::Mutex::new(focus),
        }
    }

    pub fn track_focus(&self) -> TrackFocus {
        self.focus
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    /// Record the result of a playback change made by this request.
    fn note_playback(&self, played_track_id: Option<&str>) {
        let focus = played_track_id
            .map(|id| TrackFocus::Playing(id.to_string()))
            .unwrap_or(TrackFocus::Unconfirmed);
        *self
            .focus
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = focus;
    }
}

/// A validated call seen by a dry-run executor (probe reports only; never logged).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DryRunCall {
    pub call_id: String,
    pub name: String,
    pub args: Value,
}

/// Executes calls for one chat request: a call id runs at most once (repeat ids return
/// the recorded outcome), nothing runs after cancellation, and every outcome is recorded
/// (and logged, when an event context is attached) the moment the call finishes, so the
/// record survives a provider failure or cancellation later in the request.
pub struct ToolRunner {
    outcomes: HashMap<String, ToolOutcome>,
    order: Vec<String>,
    cancelled: Arc<AtomicBool>,
    dry_run: bool,
    dry_run_calls: Vec<DryRunCall>,
    events: Option<EventContext>,
}

impl ToolRunner {
    pub fn new(cancelled: Arc<AtomicBool>) -> Self {
        Self {
            outcomes: HashMap::new(),
            order: Vec::new(),
            cancelled,
            dry_run: false,
            dry_run_calls: Vec::new(),
            events: None,
        }
    }

    /// Validate calls but never execute them (endpoint probes).
    pub fn dry_run(cancelled: Arc<AtomicBool>) -> Self {
        Self {
            dry_run: true,
            ..Self::new(cancelled)
        }
    }

    /// Record an `exec` event for every call as soon as it finishes.
    pub fn with_events(mut self, events: EventContext) -> Self {
        self.events = Some(events);
        self
    }

    /// Outcomes in execution order.
    pub fn outcomes(&self) -> Vec<ToolOutcome> {
        self.order
            .iter()
            .filter_map(|id| self.outcomes.get(id).cloned())
            .collect()
    }

    /// Calls a dry-run executor accepted, with their validated arguments.
    pub fn dry_run_calls(&self) -> &[DryRunCall] {
        &self.dry_run_calls
    }

    pub async fn run(&mut self, ctx: &ToolContext<'_>, call: &ToolCall) -> ToolOutcome {
        if let Some(previous) = self.outcomes.get(&call.id) {
            log::warn!("[tools] duplicate call id {} for {}", call.id, call.name);
            return previous.clone();
        }
        let outcome = if self.cancelled.load(Ordering::SeqCst) {
            ToolOutcome::failure(
                call,
                "cancelled",
                "The request was cancelled before this action ran.",
            )
        } else {
            match validate(call) {
                Ok(_) if self.dry_run => {
                    self.dry_run_calls.push(DryRunCall {
                        call_id: call.id.clone(),
                        name: call.name.clone(),
                        args: call.args.clone(),
                    });
                    ToolOutcome::success(
                        call,
                        format!(
                            "dry-run: {} accepted with {} (not executed)",
                            call.name, call.args
                        ),
                    )
                }
                Ok(_) if crate::faults::active("tool_exec_fail") => ToolOutcome::failure(
                    call,
                    "injected_failure",
                    "Injected failure (EXPOTIFY_FAULTS=tool_exec_fail); nothing was executed.",
                ),
                Ok(invocation)
                    if invocation.uses_spotify_web_api()
                        && crate::faults::active("spotify_not_connected") =>
                {
                    ToolOutcome::failure(
                        call,
                        "spotify_not_connected",
                        "Spotify is not connected. Connect it in Settings.",
                    )
                }
                Ok(invocation) => {
                    let outcome = execute(ctx, call, invocation).await;
                    // Test hook: hold the request after each executed action so a tester can
                    // cancel or send a new request between the actions of one request
                    // (`tool_delay` = 5 s, `tool_delay=15` = 15 s).
                    if crate::faults::active("tool_delay") {
                        let seconds = crate::faults::value("tool_delay")
                            .and_then(|value| value.parse::<u64>().ok())
                            .unwrap_or(5);
                        tokio::time::sleep(std::time::Duration::from_secs(seconds)).await;
                    }
                    outcome
                }
                Err(error) => ToolOutcome::failure(call, error.code, error.message),
            }
        };
        self.order.push(call.id.clone());
        self.outcomes.insert(call.id.clone(), outcome.clone());
        if let Some(events) = &self.events {
            let mut event = events.event("exec");
            event.tool = Some(outcome.name.clone());
            event.call_id = Some(outcome.call_id.clone());
            event.ok = Some(outcome.ok);
            event.error_code = outcome.error_code.clone();
            events::record(event);
        }
        outcome
    }
}

/// Case- and whitespace-insensitive artist name, for exact comparison.
fn normalized_artist(name: &str) -> String {
    name.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// Index of the first result credited to exactly `artist` (one of the result's credited
/// artists, compared after normalisation), with a flag saying whether the artist matched.
/// Substring matches are deliberately not accepted ("Queen" is not Queens of the Stone Age,
/// a tribute act is not the artist), and short forms or translations are not guessed: those
/// come back as `artist_not_found` with candidates for the user to confirm. The display
/// string is never split on commas (a band name may contain one); when a source only gave
/// the display string it counts as a single name. Without a named artist the top hit is taken.
fn pick_track(results: &[SearchResult], artist: &str) -> (usize, bool) {
    let wanted = normalized_artist(artist);
    if wanted.is_empty() {
        return (0, true);
    }
    let found = results.iter().position(|result| {
        if result.artist_names.is_empty() {
            normalized_artist(&result.artist) == wanted
        } else {
            result
                .artist_names
                .iter()
                .any(|name| normalized_artist(name) == wanted)
        }
    });
    match found {
        Some(index) => (index, true),
        None => (0, false),
    }
}

/// The prompt text after an update: `append` keeps the existing prompt and adds the new
/// text as a final paragraph.
fn merged_prompt(previous: &str, content: &str, mode: &PromptMode) -> String {
    match mode {
        PromptMode::Replace => content.to_string(),
        PromptMode::Append => {
            let base = previous.trim_end();
            if base.is_empty() {
                content.to_string()
            } else {
                format!("{base}\n\n{content}")
            }
        }
    }
}

/// Start playback of `track` and report what actually happened. The play command can succeed
/// while the window handling after it fails or times out, so an error is checked against the
/// player's current track before it is reported; when that check cannot confirm either way,
/// the outcome says the current song is unknown instead of implying the previous song is
/// still playing.
async fn start_playback(
    ctx: &ToolContext<'_>,
    call: &ToolCall,
    track: &SearchResult,
    prefix: &str,
) -> ToolOutcome {
    let label = format!("{} - {}", track.name, track.artist);
    let uri = track.uri.clone();
    let error =
        match tokio::task::spawn_blocking(move || spotify::applescript::spotify_play_track(&uri))
            .await
        {
            Ok(Ok(())) => None,
            Ok(Err(e)) => Some(e.to_string()),
            Err(e) => Some(e.to_string()),
        };
    let Some(error) = error else {
        ctx.note_playback(Some(&track.id));
        let mut outcome = ToolOutcome::success(call, format!("{prefix}: {label}"));
        outcome.track_name = Some(label);
        return outcome;
    };
    log::warn!("[tools] play reported an error, checking the player: {error}");
    let observed = tokio::task::spawn_blocking(spotify::applescript::get_current_track)
        .await
        .ok()
        .and_then(Result::ok)
        .flatten();
    match observed {
        Some(current) if same_track(&current.id, &track.id) => {
            ctx.note_playback(Some(&track.id));
            let mut outcome = ToolOutcome::success(
                call,
                format!("{prefix}: {label} (playback confirmed; the window handling after it failed: {error})"),
            );
            outcome.track_name = Some(label);
            outcome
        }
        _ => {
            ctx.note_playback(None);
            ToolOutcome::failure(
                call,
                "play_failed",
                format!(
                    "Could not confirm playback of \"{label}\": {error}. Playback may still have started, so the current song is unknown now; ask the user what is playing before liking or unliking."
                ),
            )
        }
    }
}

/// Spotify track ids compare equal with or without the `spotify:track:` prefix.
fn same_track(a: &str, b: &str) -> bool {
    let bare = |id: &str| id.strip_prefix("spotify:track:").unwrap_or(id).to_string();
    !a.trim().is_empty() && bare(a.trim()) == bare(b.trim())
}

async fn execute(ctx: &ToolContext<'_>, call: &ToolCall, invocation: Invocation) -> ToolOutcome {
    match invocation {
        Invocation::SearchAndPlay { query, artist } => {
            // With a named artist, look past Spotify's personalised first hit and take the
            // first result by that artist; a substitute by someone else is never played on
            // the user's behalf. Without a named artist the top hit is the best match.
            let limit = if artist.is_empty() { 1 } else { 10 };
            let results = {
                let webapi = ctx.spotify_webapi.read().await;
                let Some(webapi) = webapi.as_ref() else {
                    return ToolOutcome::failure(
                        call,
                        "spotify_not_connected",
                        "Spotify is not connected. Connect it in Settings.",
                    );
                };
                webapi.search_tracks(&query, limit).await
            };
            let track = match results {
                Ok(mut results) if !results.is_empty() => {
                    let (index, matched) = pick_track(&results, &artist);
                    if !matched {
                        ctx.note_playback(None);
                        let candidates: Vec<String> = results
                            .iter()
                            .take(3)
                            .map(|result| format!("{} - {}", result.name, result.artist))
                            .collect();
                        return ToolOutcome::failure(
                            call,
                            "artist_not_found",
                            format!(
                                "No result by \"{artist}\" among the top {limit} matches for \"{query}\"; nothing was played. Closest matches: {}. Ask the user whether one of these will do.",
                                candidates.join("; ")
                            ),
                        );
                    }
                    results.remove(index)
                }
                Ok(_) => {
                    ctx.note_playback(None);
                    return ToolOutcome::failure(
                        call,
                        "no_results",
                        format!("No Spotify results for \"{query}\"."),
                    );
                }
                Err(e) => {
                    log::warn!("[tools] search failed: {e}");
                    ctx.note_playback(None);
                    return ToolOutcome::failure(
                        call,
                        "search_failed",
                        format!("Spotify search failed: {e}"),
                    );
                }
            };
            start_playback(ctx, call, &track, "Now playing").await
        }
        Invocation::LikeCurrent | Invocation::UnlikeCurrent => {
            let like = matches!(invocation, Invocation::LikeCurrent);
            let track_id = match ctx.track_focus() {
                TrackFocus::Playing(id) => id,
                TrackFocus::NothingPlaying => {
                    return ToolOutcome::failure(
                        call,
                        "nothing_playing",
                        "Nothing is playing right now.",
                    );
                }
                TrackFocus::Unconfirmed => {
                    return ToolOutcome::failure(
                        call,
                        "track_unconfirmed",
                        "The last playback change in this request failed, so it is unclear which song is meant. Ask the user which song to like.",
                    );
                }
            };
            let webapi = ctx.spotify_webapi.read().await;
            let Some(webapi) = webapi.as_ref() else {
                return ToolOutcome::failure(
                    call,
                    "spotify_not_connected",
                    "Spotify is not connected. Connect it in Settings.",
                );
            };
            let result = if like {
                webapi.like_track(&track_id).await
            } else {
                webapi.unlike_track(&track_id).await
            };
            match result {
                Ok(()) if like => {
                    ToolOutcome::success(call, "Added the current song to Liked Songs.")
                }
                Ok(()) => ToolOutcome::success(call, "Removed the current song from Liked Songs."),
                Err(e) => {
                    log::warn!("[tools] like/unlike failed: {e}");
                    ToolOutcome::failure(
                        call,
                        "spotify_request_failed",
                        format!("Spotify rejected the request: {e}"),
                    )
                }
            }
        }
        Invocation::ShuffleLiked => {
            let picked = {
                let webapi = ctx.spotify_webapi.read().await;
                let Some(webapi) = webapi.as_ref() else {
                    return ToolOutcome::failure(
                        call,
                        "spotify_not_connected",
                        "Spotify is not connected. Connect it in Settings.",
                    );
                };
                webapi.get_random_liked_track().await
            };
            let track = match picked {
                Ok(track) => track,
                Err(e) => {
                    log::warn!("[tools] random liked track failed: {e}");
                    ctx.note_playback(None);
                    return ToolOutcome::failure(
                        call,
                        "spotify_request_failed",
                        format!("Could not pick a liked song: {e}"),
                    );
                }
            };
            start_playback(ctx, call, &track, "Now playing a random liked song").await
        }
        Invocation::SetVolume { level } => {
            match tokio::task::spawn_blocking(move || {
                spotify::applescript::set_spotify_volume(level)
            })
            .await
            {
                Ok(Ok(())) => ToolOutcome::success(call, format!("Volume set to {level}.")),
                Ok(Err(e)) => {
                    log::warn!("[tools] set volume failed: {e}");
                    ToolOutcome::failure(
                        call,
                        "volume_failed",
                        format!("Could not change the volume: {e}"),
                    )
                }
                Err(e) => ToolOutcome::failure(
                    call,
                    "volume_failed",
                    format!("Could not change the volume: {e}"),
                ),
            }
        }
        Invocation::SaveMemory { content } => {
            let mut settings = ctx.settings.write().await;
            settings.memories.push(content);
            if settings.memories.len() > 50 {
                settings.memories.remove(0);
            }
            match settings.save() {
                Ok(()) => ToolOutcome::success(call, "Memory saved."),
                Err(e) => {
                    settings.memories.pop();
                    log::warn!("[tools] save memory failed: {e}");
                    ToolOutcome::failure(
                        call,
                        "settings_write_failed",
                        format!("Could not save the memory: {e}"),
                    )
                }
            }
        }
        Invocation::UpdatePrompt {
            kind,
            content,
            mode,
        } => {
            let mut settings = ctx.settings.write().await;
            let (slot, label) = match kind {
                PromptKind::Insight => (&mut settings.ai_prompt, "insight"),
                PromptKind::Chat => (&mut settings.chat_prompt, "chat"),
            };
            let merged = merged_prompt(slot, &content, &mode);
            let previous = std::mem::replace(slot, merged);
            match settings.save() {
                Ok(()) => ToolOutcome::success(
                    call,
                    match mode {
                        PromptMode::Replace => format!("Replaced the {label} prompt."),
                        PromptMode::Append => {
                            format!("Appended to the {label} prompt; the earlier text is kept.")
                        }
                    },
                ),
                Err(e) => {
                    match kind {
                        PromptKind::Insight => settings.ai_prompt = previous,
                        PromptKind::Chat => settings.chat_prompt = previous,
                    }
                    log::warn!("[tools] update prompt failed: {e}");
                    ToolOutcome::failure(
                        call,
                        "settings_write_failed",
                        format!("Could not save the {label} prompt: {e}"),
                    )
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::AgentResponse;

    fn call(name: &str, args: Value) -> ToolCall {
        ToolCall {
            id: "c1".into(),
            name: name.into(),
            args,
        }
    }

    #[test]
    fn registry_schemas_are_strict_compatible() {
        assert_eq!(definitions().len(), TOOL_NAMES.len());
        for def in definitions() {
            assert!(TOOL_NAMES.contains(&def.name));
            assert_eq!(def.parameters["type"], "object");
            assert_eq!(def.parameters["additionalProperties"], false);
            let properties = def.parameters["properties"].as_object().unwrap();
            let required: Vec<&str> = def.parameters["required"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_str().unwrap())
                .collect();
            for key in properties.keys() {
                assert!(
                    required.contains(&key.as_str()),
                    "{} must require {key}",
                    def.name
                );
            }
        }
    }

    #[test]
    fn volume_accepts_integers_and_integral_floats_but_not_strings() {
        assert_eq!(
            validate(&call(SET_VOLUME, json!({"level": 30}))).unwrap(),
            Invocation::SetVolume { level: 30 }
        );
        assert_eq!(
            validate(&call(SET_VOLUME, json!({"level": 50.0}))).unwrap(),
            Invocation::SetVolume { level: 50 }
        );
        assert_eq!(
            validate(&call(SET_VOLUME, json!({"level": "30"})))
                .unwrap_err()
                .code,
            "invalid_argument"
        );
        assert_eq!(
            validate(&call(SET_VOLUME, json!({"level": 120})))
                .unwrap_err()
                .code,
            "invalid_argument"
        );
        assert_eq!(
            validate(&call(SET_VOLUME, json!({"level": 30.5})))
                .unwrap_err()
                .code,
            "invalid_argument"
        );
        assert_eq!(
            validate(&call(SET_VOLUME, json!({}))).unwrap_err().code,
            "missing_argument"
        );
    }

    #[test]
    fn string_arguments_and_prompt_kind_are_checked() {
        assert_eq!(
            validate(&call(SEARCH_AND_PLAY, json!({"query": "  "})))
                .unwrap_err()
                .code,
            "invalid_argument"
        );
        assert_eq!(
            validate(&call(SEARCH_AND_PLAY, Value::Null))
                .unwrap_err()
                .code,
            "missing_argument"
        );
        assert_eq!(
            validate(&call(
                UPDATE_PROMPT,
                json!({"type": "lyrics", "content": "x"})
            ))
            .unwrap_err()
            .code,
            "invalid_argument"
        );
        assert_eq!(
            validate(&call(
                UPDATE_PROMPT,
                json!({"type": "chat", "content": "Be brief."})
            ))
            .unwrap(),
            Invocation::UpdatePrompt {
                kind: PromptKind::Chat,
                content: "Be brief.".into(),
                mode: PromptMode::Replace
            }
        );
        assert_eq!(
            validate(&call("play", json!({}))).unwrap_err().code,
            "unknown_tool"
        );
        assert_eq!(
            validate(&call(LIKE_CURRENT, json!([]))).unwrap_err().code,
            "invalid_argument"
        );
    }

    #[test]
    fn legacy_conversational_actions_do_not_become_calls_but_unknown_actions_do() {
        let reply = AgentResponse {
            action: "reply".into(),
            message: "hi".into(),
            args: Value::Null,
            parse_via: None,
        };
        assert!(ToolCall::from_legacy(&reply, 1).is_none());
        let unknown = AgentResponse {
            action: "play".into(),
            message: "..".into(),
            args: json!({"q": 1}),
            parse_via: None,
        };
        let call = ToolCall::from_legacy(&unknown, 2).unwrap();
        assert_eq!(call.name, "play");
        assert_eq!(call.id, "legacy-2");
        assert_eq!(validate(&call).unwrap_err().code, "unknown_tool");
    }

    #[tokio::test]
    async fn runner_reports_failures_explicitly_and_runs_each_call_id_once() {
        let webapi = RwLock::new(None);
        let settings = RwLock::new(Settings::default());
        let ctx = ToolContext::new(&webapi, &settings, None);
        let mut runner = ToolRunner::new(Arc::new(AtomicBool::new(false)));

        let like = runner.run(&ctx, &call(LIKE_CURRENT, json!({}))).await;
        assert!(!like.ok);
        assert_eq!(like.error_code.as_deref(), Some("nothing_playing"));

        let again = runner.run(&ctx, &call(LIKE_CURRENT, json!({}))).await;
        assert_eq!(again, like);
        assert_eq!(runner.outcomes().len(), 1);

        let unknown = runner
            .run(
                &ctx,
                &ToolCall {
                    id: "c2".into(),
                    name: "dance".into(),
                    args: Value::Null,
                },
            )
            .await;
        assert_eq!(unknown.error_code.as_deref(), Some("unknown_tool"));
        assert_eq!(runner.outcomes().len(), 2);
    }

    #[tokio::test]
    async fn cancelled_runner_does_not_execute() {
        let webapi = RwLock::new(None);
        let settings = RwLock::new(Settings::default());
        let ctx = ToolContext::new(&webapi, &settings, Some("t".into()));
        let cancelled = Arc::new(AtomicBool::new(true));
        let mut runner = ToolRunner::new(cancelled);
        let outcome = runner
            .run(&ctx, &call(SEARCH_AND_PLAY, json!({"query": "x"})))
            .await;
        assert_eq!(outcome.error_code.as_deref(), Some("cancelled"));
    }

    #[tokio::test]
    async fn cancellation_wait_resolves_even_when_cancel_came_first() {
        let cancellation = ChatCancellation::new();
        cancellation.cancel();
        tokio::time::timeout(
            std::time::Duration::from_millis(100),
            cancellation.cancelled(),
        )
        .await
        .expect("an already-cancelled request must not block");

        let pending = ChatCancellation::new();
        let canceller = pending.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            canceller.cancel();
        });
        tokio::time::timeout(std::time::Duration::from_secs(2), pending.cancelled())
            .await
            .expect("cancel must wake the waiter");
        assert!(pending.is_cancelled());
    }

    #[tokio::test]
    async fn dry_run_keeps_validated_calls_with_their_arguments() {
        let settings = RwLock::new(Settings::default());
        let spotify = RwLock::new(None);
        let ctx = ToolContext::new(&spotify, &settings, None);
        let mut runner = ToolRunner::dry_run(Arc::new(AtomicBool::new(false)));
        let accepted = runner
            .run(&ctx, &call(SET_VOLUME, json!({"level": 42})))
            .await;
        assert!(accepted.ok);
        let rejected = runner
            .run(
                &ctx,
                &ToolCall {
                    id: "c2".into(),
                    name: "unknown_tool".into(),
                    args: json!({}),
                },
            )
            .await;
        assert_eq!(rejected.error_code.as_deref(), Some("unknown_tool"));
        assert_eq!(
            runner.dry_run_calls(),
            &[DryRunCall {
                call_id: "c1".into(),
                name: SET_VOLUME.into(),
                args: json!({"level": 42})
            }]
        );
        assert_eq!(runner.outcomes().len(), 2);
    }

    #[tokio::test]
    async fn like_follows_confirmed_playback_and_refuses_after_a_failed_change() {
        let settings = RwLock::new(Settings::default());
        let spotify = RwLock::new(None);
        let ctx = ToolContext::new(&spotify, &settings, Some("old".into()));
        assert_eq!(ctx.track_focus(), TrackFocus::Playing("old".into()));
        ctx.note_playback(Some("new"));
        assert_eq!(
            ctx.track_focus(),
            TrackFocus::Playing("new".into()),
            "a confirmed play moves the focus"
        );
        ctx.note_playback(None);
        assert_eq!(
            ctx.track_focus(),
            TrackFocus::Unconfirmed,
            "a failed play leaves the focus unclear"
        );
        let mut runner = ToolRunner::new(Arc::new(AtomicBool::new(false)));
        let outcome = runner.run(&ctx, &call(LIKE_CURRENT, json!({}))).await;
        assert_eq!(outcome.error_code.as_deref(), Some("track_unconfirmed"));
        let fresh = ToolContext::new(&spotify, &settings, None);
        let outcome = runner
            .run(
                &fresh,
                &ToolCall {
                    id: "c2".into(),
                    name: UNLIKE_CURRENT.into(),
                    args: json!({}),
                },
            )
            .await;
        assert_eq!(outcome.error_code.as_deref(), Some("nothing_playing"));
    }

    fn search_result(name: &str, artists: &[&str]) -> SearchResult {
        SearchResult {
            id: name.to_lowercase().replace(' ', "-"),
            name: name.into(),
            artist: artists.join(", "),
            artist_names: artists.iter().map(|artist| artist.to_string()).collect(),
            album: String::new(),
            album_art_url: None,
            duration_ms: 0,
            uri: format!("spotify:track:{name}"),
        }
    }

    /// A result from a source that only provides the joined display string.
    fn display_only_result(name: &str, artist: &str) -> SearchResult {
        SearchResult {
            artist_names: Vec::new(),
            ..search_result(name, &[artist])
        }
    }

    #[test]
    fn a_named_artist_wins_over_spotifys_first_hit_only_on_an_exact_name() {
        let results = vec![
            search_result("FLY ME TO THE MOON - 2020 Version", &["Yoko Takahashi"]),
            search_result("Fly Me to the Moon", &["Frank Sinatra", "Count Basie"]),
        ];
        assert_eq!(pick_track(&results, "Frank Sinatra"), (1, true));
        assert_eq!(
            pick_track(&results, "  frank   SINATRA "),
            (1, true),
            "case and spacing do not matter"
        );
        assert_eq!(
            pick_track(&results, "Count Basie"),
            (1, true),
            "any credited artist counts"
        );
        assert_eq!(pick_track(&results, "Yoko Takahashi"), (0, true));
        assert_eq!(
            pick_track(&results, "Sinatra"),
            (0, false),
            "short forms are not guessed; the user confirms"
        );
        assert_eq!(
            pick_track(&results, "Diana Krall"),
            (0, false),
            "no match is reported, never played as a substitute"
        );
        assert_eq!(pick_track(&results, ""), (0, true));

        let lookalikes = vec![
            search_result("No One Knows", &["Queens of the Stone Age"]),
            search_result("红豆", &["王菲儿"]),
            search_result("The Look of Love", &["Diana Krall Tribute"]),
        ];
        assert_eq!(pick_track(&lookalikes, "Queen"), (0, false));
        assert_eq!(pick_track(&lookalikes, "王菲"), (0, false));
        assert_eq!(pick_track(&lookalikes, "Diana Krall"), (0, false));

        // A band name containing a comma is one credited artist, never split; the same holds
        // when a source only provided the joined display string.
        let band = vec![search_result("September", &["Earth, Wind & Fire"])];
        assert_eq!(pick_track(&band, "Earth, Wind & Fire"), (0, true));
        assert_eq!(pick_track(&band, "Earth"), (0, false));
        let display_only = vec![display_only_result("September", "Earth, Wind & Fire")];
        assert_eq!(pick_track(&display_only, "Earth, Wind & Fire"), (0, true));
        assert_eq!(pick_track(&display_only, "Earth"), (0, false));
        assert_eq!(pick_track(&display_only, "Fire"), (0, false));
    }

    #[test]
    fn track_ids_match_with_or_without_the_uri_prefix() {
        assert!(same_track("spotify:track:abc", "abc"));
        assert!(same_track("abc", "spotify:track:abc"));
        assert!(!same_track("abc", "abd"));
        assert!(!same_track("", ""));
    }

    #[test]
    fn optional_artist_and_prompt_modes_validate() {
        assert_eq!(
            validate(&call(SEARCH_AND_PLAY, json!({"query": "x"}))).unwrap(),
            Invocation::SearchAndPlay {
                query: "x".into(),
                artist: String::new()
            },
            "legacy callers without `artist` still work"
        );
        assert_eq!(
            validate(&call(
                SEARCH_AND_PLAY,
                json!({"query": "x", "artist": " Frank Sinatra "})
            ))
            .unwrap(),
            Invocation::SearchAndPlay {
                query: "x".into(),
                artist: "Frank Sinatra".into()
            }
        );
        assert_eq!(
            validate(&call(SEARCH_AND_PLAY, json!({"query": "x", "artist": 5})))
                .unwrap_err()
                .code,
            "invalid_argument"
        );
        assert_eq!(
            validate(&call(
                UPDATE_PROMPT,
                json!({"type": "chat", "content": "Be brief.", "mode": "append"})
            ))
            .unwrap(),
            Invocation::UpdatePrompt {
                kind: PromptKind::Chat,
                content: "Be brief.".into(),
                mode: PromptMode::Append
            }
        );
        assert_eq!(
            validate(&call(
                UPDATE_PROMPT,
                json!({"type": "chat", "content": "x", "mode": "merge"})
            ))
            .unwrap_err()
            .code,
            "invalid_argument"
        );
        assert_eq!(
            merged_prompt("Old prompt.\n", "Be brief.", &PromptMode::Append),
            "Old prompt.\n\nBe brief."
        );
        assert_eq!(
            merged_prompt("", "Be brief.", &PromptMode::Append),
            "Be brief."
        );
        assert_eq!(merged_prompt("Old", "New", &PromptMode::Replace), "New");
    }
}
