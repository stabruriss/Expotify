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
    /// Window position (x, y) - None means default position
    pub window_position: Option<(f64, f64)>,
    /// Window opacity (0.0 - 1.0)
    pub window_opacity: f64,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            poll_interval_secs: 3,
            show_ai_description: true,
            ai_model: "gpt-4o-mini".to_string(),
            window_position: None,
            window_opacity: 0.95,
        }
    }
}

impl Settings {
    /// Get the settings file path
    fn get_settings_path() -> Result<PathBuf> {
        let config_dir = dirs::config_dir()
            .ok_or_else(|| anyhow::anyhow!("Could not find config directory"))?;
        let app_dir = config_dir.join("expotify");
        std::fs::create_dir_all(&app_dir)?;
        Ok(app_dir.join("settings.json"))
    }

    /// Load settings from disk
    pub fn load() -> Result<Self> {
        let path = Self::get_settings_path()?;
        if path.exists() {
            let content = std::fs::read_to_string(&path)?;
            let settings = serde_json::from_str(&content)?;
            Ok(settings)
        } else {
            Ok(Self::default())
        }
    }

    /// Save settings to disk
    pub fn save(&self) -> Result<()> {
        let path = Self::get_settings_path()?;
        let content = serde_json::to_string_pretty(self)?;
        std::fs::write(&path, content)?;
        Ok(())
    }
}
