use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command as StdCommand, Stdio};
use std::sync::Arc;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

use super::cache::TrackInfoCache;
use super::AgentResponse;
use crate::auth::AnthropicAuth;
use crate::spotify::TrackInfo;

const TRACK_SYSTEM_PROMPT: &str = "You are a music expert with deep knowledge of musical styles, genres, creators, music theory, music and art history, as well as fascinating stories and trivia. You excel at making music accessible and engaging, effectively conveying knowledge while sparking the listener's curiosity.";

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ClaudeHelperRequest {
    oauth_token: String,
    model: String,
    system_prompt: String,
    prompt: String,
}

#[derive(Debug, Deserialize)]
struct ClaudeHelperResponse {
    text: String,
    #[allow(dead_code)]
    structured: Option<serde_json::Value>,
}

pub struct AnthropicService {
    auth: Arc<AnthropicAuth>,
    helper_script_path: PathBuf,
    cache: TrackInfoCache,
}

impl AnthropicService {
    pub fn new(auth: Arc<AnthropicAuth>, helper_script_path: PathBuf) -> Self {
        Self {
            auth,
            helper_script_path,
            cache: TrackInfoCache::default(),
        }
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
        if force {
            self.cache.remove(&track.id).await;
        } else if let Some(cached) = self.cache.get(&track.id).await {
            return Ok((cached, false));
        }

        let prompt = prompt_template
            .replace("{name}", &track.name)
            .replace("{artist}", &track.artist)
            .replace("{album}", &track.album)
            .replace("{memories}", &format_memories(memories));

        let description = self.run_prompt(model, TRACK_SYSTEM_PROMPT, &prompt).await?;
        if description.is_empty() {
            anyhow::bail!("Empty response from Claude");
        }

        self.cache.set(track.id.clone(), description.clone()).await;
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

        let prompt = format_chat_history(messages);
        let text = self.run_prompt(model, &system_prompt, &prompt).await?;
        Ok(super::parse_agent_response(&text))
    }

    async fn run_prompt(&self, model: &str, system_prompt: &str, prompt: &str) -> Result<String> {
        let oauth_token = self.auth.get_access_token().await?;
        let request = ClaudeHelperRequest {
            oauth_token,
            model: model.to_string(),
            system_prompt: system_prompt.to_string(),
            prompt: prompt.to_string(),
        };
        let payload = serde_json::to_vec(&request)?;
        let node_path = resolve_node_binary()?;

        log::info!(
            "Starting Claude helper with Node at {}",
            node_path.display()
        );
        let mut child = Command::new(&node_path)
            .arg(&self.helper_script_path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| {
                format!(
                    "Failed to start Claude helper at {} with Node {}. Ensure Node.js 18+ is installed or set EXPOTIFY_NODE_PATH.",
                    self.helper_script_path.display(),
                    node_path.display()
                )
            })?;

        let mut stdin = child
            .stdin
            .take()
            .context("Claude helper stdin was not available")?;
        stdin.write_all(&payload).await?;
        drop(stdin);

        let output = child.wait_with_output().await?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
            let detail = if !stderr.is_empty() { stderr } else { stdout };
            anyhow::bail!(
                "Claude helper failed{}",
                if detail.is_empty() {
                    String::new()
                } else {
                    format!(": {}", detail)
                }
            );
        }

        let response: ClaudeHelperResponse = serde_json::from_slice(&output.stdout)
            .context("Failed to parse Claude helper output")?;
        Ok(response.text)
    }
}

fn resolve_node_binary() -> Result<PathBuf> {
    if let Ok(explicit) = env::var("EXPOTIFY_NODE_PATH") {
        let path = PathBuf::from(explicit);
        match node_major_version(&path) {
            Some(major) if major >= 18 => return Ok(path),
            Some(major) => {
                log::warn!(
                    "Ignoring EXPOTIFY_NODE_PATH={} because it points to Node.js {}",
                    path.display(),
                    major
                );
            }
            None => {
                log::warn!(
                    "Ignoring EXPOTIFY_NODE_PATH={} because it is not a usable Node.js binary",
                    path.display()
                );
            }
        }
    }

    if let Some(path) = find_in_path("node") {
        return Ok(path);
    }

    for candidate in common_node_candidates() {
        if is_usable_binary(&candidate) {
            return Ok(candidate);
        }
    }

    anyhow::bail!(
        "Node.js 18+ executable not found. Install Node.js or set EXPOTIFY_NODE_PATH to the full node binary path."
    );
}

fn find_in_path(binary_name: &str) -> Option<PathBuf> {
    let path_var = env::var_os("PATH")?;
    env::split_paths(&path_var)
        .map(|dir| dir.join(binary_name))
        .find(|candidate| is_usable_binary(candidate))
}

fn common_node_candidates() -> Vec<PathBuf> {
    let mut candidates = vec![
        PathBuf::from("/opt/homebrew/bin/node"),
        PathBuf::from("/usr/local/bin/node"),
        PathBuf::from("/usr/bin/node"),
        PathBuf::from("/opt/local/bin/node"),
    ];

    if let Some(home) = dirs::home_dir() {
        candidates.push(home.join(".volta/bin/node"));
        candidates.push(home.join(".asdf/shims/node"));
        candidates.push(home.join(".fnm/current/bin/node"));
        candidates.push(home.join(".local/share/mise/shims/node"));
        candidates.push(home.join(".mise/shims/node"));
        candidates.extend(find_nvm_nodes(&home));
    }

    candidates
}

fn find_nvm_nodes(home: &Path) -> Vec<PathBuf> {
    let base = home.join(".nvm/versions/node");
    let entries = match fs::read_dir(base) {
        Ok(entries) => entries,
        Err(_) => return Vec::new(),
    };

    let mut versions: Vec<PathBuf> = entries
        .filter_map(|entry| entry.ok().map(|item| item.path().join("bin/node")))
        .filter(|candidate| is_usable_binary(candidate))
        .collect();

    versions.sort();
    versions.reverse();
    versions
}

fn is_usable_binary(path: &Path) -> bool {
    matches!(node_major_version(path), Some(major) if major >= 18)
}

fn node_major_version(path: &Path) -> Option<u32> {
    if !path.is_file() {
        return None;
    }

    let output = StdCommand::new(path).arg("--version").output().ok()?;
    if !output.status.success() {
        return None;
    }

    let version = String::from_utf8_lossy(&output.stdout);
    parse_node_major_version(&version)
}

fn parse_node_major_version(version: &str) -> Option<u32> {
    let trimmed = version.trim();
    let raw = trimmed.strip_prefix('v').unwrap_or(trimmed);
    raw.split('.').next()?.parse().ok()
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

    let mut transcript = String::from(
        "Conversation so far:\n\nReply to the latest user message. If you decide to call a tool, return only the JSON object requested in the system prompt.\n\n",
    );

    for message in messages {
        let speaker = if message.role == "assistant" {
            "Assistant"
        } else {
            "User"
        };
        transcript.push_str(speaker);
        transcript.push_str(": ");
        transcript.push_str(&message.content);
        transcript.push_str("\n\n");
    }

    transcript
}
