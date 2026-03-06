use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    /// Polling interval in seconds
    pub poll_interval_secs: u64,
    /// Whether to show AI descriptions
    pub show_ai_description: bool,
    /// AI model to use
    pub ai_model: String,
    /// Custom AI prompt template
    pub ai_prompt: String,
    /// Enable web search for AI
    #[serde(default)]
    pub ai_web_search: bool,
    /// Auto-generate AI insight on track change
    #[serde(default)]
    pub ai_auto: bool,
    /// Read AI insight aloud before playing new track (requires ai_auto=true)
    #[serde(default)]
    pub ai_read_aloud: bool,
    /// Window position (x, y) - None means default position
    pub window_position: Option<(f64, f64)>,
    /// Window opacity (0.0 - 1.0)
    pub window_opacity: f64,
    /// TTS voice volume (0.0 - 1.0)
    #[serde(default = "default_tts_volume")]
    pub tts_volume: f64,
    /// AI model for chat
    #[serde(default = "default_chat_model")]
    pub chat_model: String,
    /// Custom chat prompt template
    #[serde(default = "default_chat_prompt")]
    pub chat_prompt: String,
    /// Whether Anthropic (Claude) is activated by the user
    #[serde(default)]
    pub anthropic_enabled: bool,
    /// User memories (preferences, notes saved by AI)
    #[serde(default)]
    pub memories: Vec<String>,
}

fn default_tts_volume() -> f64 {
    0.8
}

fn default_chat_model() -> String {
    "gpt-5.2".to_string()
}

fn default_chat_prompt() -> String {
    DEFAULT_CHAT_PROMPT.to_string()
}

pub const DEFAULT_AI_PROMPT: &str = "Briefly introduce this song (under 500 words):\n\nSong: {name}\nArtist: {artist}\nAlbum: {album}\n\nInclude the song's style/genre and creative background. Do not repeat the song title or artist name. Give the introduction directly without preamble. No citation links in the output.\n\nSearch online for interesting stories about the track, the creator, and details about this specific version and performer, and weave them into the introduction.\n\n{memories}\nConsult the user's memories above (if any) for personalized insights. Always reply in the user's language.";

pub const DEFAULT_CHAT_PROMPT: &str = r#"You are the Expotify music assistant and the user's chat companion.

Current playback: {name} - {artist} ({album})
Current volume: {volume}%

{memories}

Available tools (reply with a single JSON object when using a tool):
- search_and_play(query): Search for a song and play the best match.
- like_current: Add current song to Liked Songs.
- unlike_current: Remove current song from Liked Songs.
- shuffle_liked: Randomly play a song from Liked Songs.
- set_volume(level): Set volume (0-100).
- save_memory(content): Save something about the user's preferences or interests.
- update_prompt(type, content): Update the AI Insight ("insight") or Chat ("chat") prompt.

Tool response format (JSON only, no markdown):
{"action": "<tool>", "args": {"<param>": <value>}, "message": "brief explanation"}

For normal conversation, just reply with plain text — no JSON needed.

IMPORTANT — Music playback intent:
When the user's intent is clearly to play music (they mention a song, artist, album, genre, mood, era, or any music-related request), DO NOT ask follow-up questions. Immediately use search_and_play with the best query you can construct from the information given. Only ask for clarification if the request is genuinely too ambiguous to form any search query (e.g. "play something" with zero context).

You can chat about any topic. Use web search when helpful for factual questions.
Use save_memory when you learn something about the user's preferences.
Consult the memories above for user preferences when relevant.
Always reply in the user's language."#;

impl Default for Settings {
    fn default() -> Self {
        Self {
            poll_interval_secs: 3,
            show_ai_description: true,
            ai_model: "gpt-5.2".to_string(),
            ai_prompt: DEFAULT_AI_PROMPT.to_string(),
            ai_web_search: true,
            ai_auto: false,
            ai_read_aloud: false,
            window_position: None,
            window_opacity: 0.95,
            tts_volume: 0.8,
            chat_model: "gpt-5.2".to_string(),
            chat_prompt: DEFAULT_CHAT_PROMPT.to_string(),
            anthropic_enabled: false,
            memories: Vec::new(),
        }
    }
}

impl Settings {
    /// Get the settings file path
    fn get_settings_path() -> Result<PathBuf> {
        let config_dir =
            dirs::config_dir().ok_or_else(|| anyhow::anyhow!("Could not find config directory"))?;
        let app_dir = config_dir.join("expotify");
        std::fs::create_dir_all(&app_dir)?;
        Ok(app_dir.join("settings.json"))
    }

    /// Load settings from disk
    pub fn load() -> Result<Self> {
        let path = Self::get_settings_path()?;
        if path.exists() {
            let content = std::fs::read_to_string(&path)?;
            let settings: Self = serde_json::from_str(&content)?;
            log::info!(
                "[settings] Loaded from {:?} (memories: {}, chat_prompt len: {})",
                path,
                settings.memories.len(),
                settings.chat_prompt.len()
            );
            Ok(settings)
        } else {
            log::info!("[settings] No settings file found, using defaults");
            Ok(Self::default())
        }
    }

    /// Save settings to disk
    pub fn save(&self) -> Result<()> {
        let path = Self::get_settings_path()?;
        log::info!(
            "[settings] Saving to {:?} (memories: {:?})",
            path,
            self.memories
        );
        let content = serde_json::to_string_pretty(self)?;
        std::fs::write(&path, content)?;
        Ok(())
    }
}
