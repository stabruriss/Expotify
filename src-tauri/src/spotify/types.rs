use serde::{Deserialize, Serialize};

/// Simplified track info for frontend display
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrackInfo {
    pub id: String,
    pub name: String,
    pub artist: String,
    pub album: String,
    pub album_art_url: Option<String>,
    pub duration_ms: u64,
    pub progress_ms: u64,
    pub is_playing: bool,
    pub spotify_url: Option<String>,
    /// AI-generated description about the track
    pub ai_description: Option<String>,
    /// Whether the AI description used web search
    #[serde(default)]
    pub ai_used_web_search: bool,
}
