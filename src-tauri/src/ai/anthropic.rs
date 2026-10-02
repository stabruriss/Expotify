use anyhow::{Context, Result};
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;

use super::cache::TrackInfoCache;
use super::models::{CatalogCache, ModelInfo, ModelProvider, ModelSelection, ProviderCatalog};
use super::tools::{self, ChatCancellation, ToolContext, ToolRunner};
use super::{
    legacy_system_prompt, native_system_prompt, render_chat_prompt, AgentResponse, ChatMessage,
    NativeChatOutcome,
};
use crate::auth::AnthropicAuth;
use crate::claude_runtime::{ClaudeAuthenticationError, ClaudeSession, HelperEvent};
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
        if let Err(error) = &result {
            self.note_auth_error(error).await;
        }
        result
    }

    /// An authentication failure from the runtime invalidates the cached connection state
    /// so the next status check re-reads the native credential.
    async fn note_auth_error(&self, error: &anyhow::Error) {
        if error.is::<ClaudeAuthenticationError>() {
            self.auth.invalidate().await;
        }
    }

    /// Native tool calling: the SDK registers the Rust tool registry as in-process MCP tools;
    /// every call the model makes is relayed here, executed by `runner`, and its outcome
    /// written back before the model continues. Plain text is never turned into an action.
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
        memories: &[String],
        ctx: &ToolContext<'_>,
        runner: &mut ToolRunner,
        cancellation: &ChatCancellation,
    ) -> Result<NativeChatOutcome> {
        let rendered =
            render_chat_prompt(prompt_template, track_name, artist, album, volume, memories);
        let request = json!({
            "action": "prompt",
            "protocol": "native",
            "model": model,
            "systemPrompt": native_system_prompt(&rendered),
            "prompt": format_native_history(messages),
            "tools": tools::definitions(),
            "maxTurns": 4,
        });
        let session = match self
            .auth
            .runtime
            .start(request, Duration::from_secs(240))
            .await
        {
            Ok(session) => session,
            Err(error) => {
                self.note_auth_error(&error).await;
                return Err(error);
            }
        };
        let result = drive_native_session(session, ctx, runner, cancellation).await;
        if let Err(error) = &result {
            self.note_auth_error(error).await;
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
        let system_prompt = legacy_system_prompt(&render_chat_prompt(
            prompt_template,
            track_name,
            artist,
            album,
            volume,
            memories,
        ));
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

/// Relay tool calls between the helper and the executor until the helper's final message.
/// Cancellation only interrupts the wait for the helper (dropping the session kills it):
/// a tool that is already running finishes and stays recorded in `runner`, and no further
/// tool runs. The caller reports `runner.outcomes()` whether or not this returns `Ok`.
async fn drive_native_session(
    mut session: ClaudeSession<'_>,
    ctx: &ToolContext<'_>,
    runner: &mut ToolRunner,
    cancellation: &ChatCancellation,
) -> Result<NativeChatOutcome> {
    loop {
        let event = tokio::select! {
            event = session.next() => event?,
            _ = cancellation.cancelled() => anyhow::bail!(tools::CANCELLED),
        };
        match event {
            HelperEvent::ToolCall(call) => {
                let outcome = runner.run(ctx, &call).await;
                if cancellation.is_cancelled() {
                    anyhow::bail!(tools::CANCELLED);
                }
                session.reply(&outcome).await?;
            }
            HelperEvent::Final(data) => {
                return Ok(NativeChatOutcome {
                    text: data["text"].as_str().unwrap_or_default().trim().to_string(),
                    turns: data["turns"].as_u64().map(|turns| turns as u32),
                    tool_uses: data["tool_uses"].as_u64().unwrap_or(0) as u32,
                });
            }
        }
    }
}

/// Native prompt: a single user message is sent as-is; a longer history is rendered as a
/// transcript that includes what each earlier tool call actually did.
fn format_native_history(messages: &[ChatMessage]) -> String {
    match messages {
        [] => "User:".to_string(),
        [only] if only.role != "assistant" => only.content.clone(),
        _ => {
            let mut transcript = String::from(
                "Conversation so far (earlier turns, for context only). Reply to the latest user message; use the tools for any action.\n\n",
            );
            for message in messages {
                let speaker = if message.role == "assistant" {
                    "Assistant"
                } else {
                    "User"
                };
                transcript.push_str(&format!("{speaker}: {}\n", message.content));
                for result in &message.tool_results {
                    let status = if result.ok { "ok" } else { "failed" };
                    transcript.push_str(&format!(
                        "  [tool {} → {status}: {}]\n",
                        result.name, result.output
                    ));
                }
                transcript.push('\n');
            }
            transcript
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::tools::ToolOutcome;
    use crate::ai::{LEGACY_TOOL_SECTION_START, NATIVE_TOOL_GUIDANCE};
    #[cfg(unix)]
    use crate::claude_runtime::test_support::FakeHelper;
    use crate::storage::settings::{DEFAULT_CHAT_PROMPT, LEGACY_DEFAULT_CHAT_PROMPTS};
    use crate::storage::Settings;
    use std::sync::atomic::AtomicBool;
    use tokio::sync::RwLock;

    #[cfg(unix)]
    const ONE_TOOL_CALL: &str = "IFS= read -r request\nprintf '%s\\n' '{\"type\":\"tool_call\",\"id\":\"call-1\",\"name\":\"set_volume\",\"args\":{\"level\":30}}'\nIFS= read -r result\n";

    /// A helper that asks for one tool call and then fails: the executed call must survive.
    #[cfg(unix)]
    #[tokio::test]
    async fn outcomes_survive_a_helper_failure_after_the_tool_ran() {
        let helper = FakeHelper::new(&format!(
            "{ONE_TOOL_CALL}printf '{{\"ok\":false,\"error\":\"helper crashed after the tool ran\"}}'"
        ));
        let settings = RwLock::new(Settings::default());
        let spotify = RwLock::new(None);
        let ctx = ToolContext::new(&spotify, &settings, None);
        let mut runner = ToolRunner::dry_run(Arc::new(AtomicBool::new(false)));
        let cancellation = ChatCancellation::new();
        let session = helper
            .runtime
            .start(
                json!({"action":"prompt","protocol":"native"}),
                Duration::from_secs(60),
            )
            .await
            .unwrap();
        let error = drive_native_session(session, &ctx, &mut runner, &cancellation)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("helper crashed"), "{error}");
        let outcomes = runner.outcomes();
        assert_eq!(outcomes.len(), 1);
        assert_eq!(outcomes[0].name, "set_volume");
        assert!(outcomes[0].ok);
    }

    /// Cancelling while the helper is "thinking" returns promptly, kills the helper, and keeps
    /// the outcome of the call that already ran. The helper marks the moment it consumed the
    /// tool result, so the cancel is issued after the tool ran, whatever the process timing.
    #[cfg(unix)]
    #[tokio::test]
    async fn cancellation_interrupts_the_helper_wait_and_keeps_outcomes() {
        let helper = FakeHelper::new(&format!("{ONE_TOOL_CALL}: > replied\nsleep 60"));
        let settings = RwLock::new(Settings::default());
        let spotify = RwLock::new(None);
        let ctx = ToolContext::new(&spotify, &settings, None);
        let mut runner = ToolRunner::dry_run(Arc::new(AtomicBool::new(false)));
        let cancellation = ChatCancellation::new();
        let session = helper
            .runtime
            .start(
                json!({"action":"prompt","protocol":"native"}),
                Duration::from_secs(120),
            )
            .await
            .unwrap();
        let canceller = cancellation.clone();
        let replied = helper.dir.join("config/replied");
        tokio::spawn(async move {
            for _ in 0..1000 {
                if replied.exists() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            canceller.cancel();
        });
        let started = std::time::Instant::now();
        let error = drive_native_session(session, &ctx, &mut runner, &cancellation)
            .await
            .unwrap_err();
        assert_eq!(error.to_string(), tools::CANCELLED);
        assert!(
            started.elapsed() < Duration::from_secs(15),
            "took {:?}",
            started.elapsed()
        );
        assert_eq!(runner.outcomes().len(), 1);
        assert!(runner.outcomes()[0].ok);
    }

    #[test]
    fn native_prompt_replaces_the_legacy_json_section_of_old_default_prompts() {
        for legacy in LEGACY_DEFAULT_CHAT_PROMPTS {
            let rendered = render_chat_prompt(legacy, "Song", "Artist", "Album", 60, &[]);
            let native = native_system_prompt(&rendered);
            assert!(!native.contains("reply with a single JSON object"));
            assert!(!native.contains("Tool response format"));
            assert_eq!(native.matches(NATIVE_TOOL_GUIDANCE).count(), 1);
            assert!(
                native.contains("Music playback intent"),
                "intent rules must survive"
            );
            assert!(native.contains("Current playback: Song - Artist (Album)"));
        }
    }

    #[test]
    fn the_shipped_default_is_already_native_and_legacy_can_be_derived_from_it() {
        let rendered = render_chat_prompt(DEFAULT_CHAT_PROMPT, "Song", "Artist", "Album", 60, &[]);
        assert_eq!(
            native_system_prompt(&rendered),
            rendered,
            "no duplicate guidance"
        );
        let legacy = legacy_system_prompt(&rendered);
        assert!(legacy.contains(LEGACY_TOOL_SECTION_START));
        assert!(!legacy.contains(NATIVE_TOOL_GUIDANCE));
        assert!(legacy.contains("Music playback intent"));
        assert_eq!(
            legacy_system_prompt(&legacy),
            legacy,
            "legacy conversion is idempotent"
        );
        assert!(legacy_system_prompt("You are a DJ.").ends_with("no JSON needed."));
    }

    #[test]
    fn native_prompt_appends_guidance_when_the_user_customised_the_prompt() {
        let native = native_system_prompt("You are a DJ.");
        assert!(native.starts_with("You are a DJ."));
        assert!(native.ends_with(NATIVE_TOOL_GUIDANCE));
    }

    #[test]
    fn native_history_sends_a_single_message_verbatim_and_renders_tool_results_otherwise() {
        let single = vec![ChatMessage {
            role: "user".into(),
            content: "播放晴天".into(),
            tool_results: vec![],
        }];
        assert_eq!(format_native_history(&single), "播放晴天");
        let history = vec![
            ChatMessage {
                role: "user".into(),
                content: "音量30".into(),
                tool_results: vec![],
            },
            ChatMessage {
                role: "assistant".into(),
                content: "已调到30。".into(),
                tool_results: vec![ToolOutcome {
                    call_id: "c".into(),
                    name: "set_volume".into(),
                    ok: true,
                    output: "Volume set to 30.".into(),
                    error_code: None,
                    track_name: None,
                }],
            },
            ChatMessage {
                role: "user".into(),
                content: "再大一点".into(),
                tool_results: vec![],
            },
        ];
        let transcript = format_native_history(&history);
        assert!(transcript.contains("User: 音量30"));
        assert!(transcript.contains("[tool set_volume → ok: Volume set to 30.]"));
        assert!(transcript.ends_with("User: 再大一点\n\n"));
        assert!(!transcript.contains("return only the JSON"));
    }
}
