use crate::ai::models::{deserialize_selection, ModelProvider, ModelSelection};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    /// Polling interval in seconds
    pub poll_interval_secs: u64,
    /// Whether to show AI descriptions
    pub show_ai_description: bool,
    /// AI model to use
    #[serde(default, deserialize_with = "deserialize_selection")]
    pub ai_model: ModelSelection,
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
    #[serde(default, deserialize_with = "deserialize_selection")]
    pub chat_model: ModelSelection,
    /// Custom chat prompt template
    #[serde(default = "default_chat_prompt")]
    pub chat_prompt: String,
    /// Whether Claude OAuth is connected and should auto-restore on launch
    #[serde(default)]
    pub anthropic_enabled: bool,
    /// Existing installs keep their selection; a fresh install follows its first connected account.
    #[serde(default = "existing_model_defaults")]
    pub model_defaults_initialized: bool,
    /// User memories (preferences, notes saved by AI)
    #[serde(default)]
    pub memories: Vec<String>,
}

fn default_tts_volume() -> f64 {
    0.8
}

fn existing_model_defaults() -> bool {
    true
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
            ai_model: ModelSelection::default(),
            ai_prompt: DEFAULT_AI_PROMPT.to_string(),
            ai_web_search: true,
            ai_auto: false,
            ai_read_aloud: false,
            window_position: None,
            window_opacity: 0.95,
            tts_volume: 0.8,
            chat_model: ModelSelection::default(),
            chat_prompt: DEFAULT_CHAT_PROMPT.to_string(),
            anthropic_enabled: false,
            model_defaults_initialized: false,
            memories: Vec::new(),
        }
    }
}

impl Settings {
    pub fn initialize_model_defaults(&mut self, provider: ModelProvider) {
        if !self.model_defaults_initialized {
            self.ai_model = ModelSelection::Default { provider };
            self.chat_model = ModelSelection::Default { provider };
            self.model_defaults_initialized = true;
        }
    }

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
        Self::load_from_path(&path)
    }

    fn load_from_path(path: &Path) -> Result<Self> {
        if path.exists() {
            let content = std::fs::read_to_string(path)?;
            let raw: serde_json::Value = serde_json::from_str(&content)?;
            let legacy = raw["ai_model"].is_string() || raw["chat_model"].is_string();
            let settings: Self = serde_json::from_value(raw)?;
            if legacy {
                backup_legacy_settings(path, &content)?;
            }
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

fn backup_legacy_settings(path: &Path, content: &str) -> Result<()> {
    let backup = path.with_file_name("settings.json.pre-model-selection.bak");
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    match options.open(&backup) {
        Ok(mut file) => {
            if let Err(error) = file
                .write_all(content.as_bytes())
                .and_then(|_| file.sync_all())
            {
                let _ = std::fs::remove_file(&backup);
                return Err(error.into());
            }
            log::info!("Preserved pre-migration settings at {}", backup.display());
            Ok(())
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
        Err(error) => Err(error.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_settings_migrate_without_changing_the_model() {
        let mut legacy = serde_json::to_value(Settings::default()).unwrap();
        legacy
            .as_object_mut()
            .unwrap()
            .remove("model_defaults_initialized");
        legacy["ai_model"] = serde_json::json!("gpt-5.4");
        legacy["chat_model"] = serde_json::json!("claude-opus-4-6");
        let mut loaded: Settings = serde_json::from_value(legacy).unwrap();
        loaded.initialize_model_defaults(ModelProvider::Anthropic);
        assert!(matches!(loaded.ai_model, ModelSelection::Fixed { .. }));
        assert!(matches!(loaded.chat_model, ModelSelection::Fixed { .. }));
        let roundtrip: Settings =
            serde_json::from_str(&serde_json::to_string(&loaded).unwrap()).unwrap();
        assert_eq!(loaded.chat_model, roundtrip.chat_model);
    }

    #[test]
    fn fresh_install_uses_first_account_default_only_once() {
        let mut settings = Settings::default();
        settings.initialize_model_defaults(ModelProvider::Anthropic);
        assert_eq!(
            settings.chat_model,
            ModelSelection::Default {
                provider: ModelProvider::Anthropic
            }
        );
        settings.initialize_model_defaults(ModelProvider::Openai);
        assert_eq!(settings.chat_model.provider(), ModelProvider::Anthropic);
    }

    #[test]
    fn migration_creates_one_private_backup_and_leaves_original_settings_untouched() {
        let dir =
            std::env::temp_dir().join(format!("expotify-settings-test-{}", rand::random::<u64>()));
        std::fs::create_dir(&dir).unwrap();
        let path = dir.join("settings.json");
        let mut legacy = serde_json::to_value(Settings::default()).unwrap();
        legacy["ai_model"] = serde_json::json!("gpt-old");
        legacy["memories"] = serde_json::json!(["Keep this memory"]);
        let original = serde_json::to_string(&legacy).unwrap();
        std::fs::write(&path, &original).unwrap();
        let loaded = Settings::load_from_path(&path).unwrap();
        assert_eq!(loaded.memories, vec!["Keep this memory"]);
        let backup = dir.join("settings.json.pre-model-selection.bak");
        assert_eq!(std::fs::read_to_string(&backup).unwrap(), original);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
        legacy["ai_prompt"] = serde_json::json!("Changed later");
        std::fs::write(&path, serde_json::to_string(&legacy).unwrap()).unwrap();
        Settings::load_from_path(&path).unwrap();
        assert_eq!(std::fs::read_to_string(&backup).unwrap(), original);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&backup).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        std::fs::remove_dir_all(dir).unwrap();
    }
}
