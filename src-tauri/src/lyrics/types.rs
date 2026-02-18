use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum LyricsSource {
    NetEase,
    QQMusic,
    Kugou,
    Lrclib,
    PetitLyrics,
    None,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LyricsLine {
    pub time_ms: u64,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LyricsInfo {
    pub track_id: String,
    pub is_instrumental: bool,
    pub synced_lines: Vec<LyricsLine>,
    pub plain_lyrics: Option<String>,
    pub translation_lines: Vec<LyricsLine>,
    pub source: LyricsSource,
    #[serde(default)]
    pub fetch_log: Vec<String>,
}

impl LyricsInfo {
    pub fn instrumental(track_id: String) -> Self {
        Self {
            track_id,
            is_instrumental: true,
            synced_lines: Vec::new(),
            plain_lyrics: None,
            translation_lines: Vec::new(),
            source: LyricsSource::None,
            fetch_log: Vec::new(),
        }
    }

    pub fn not_found(track_id: String) -> Self {
        Self {
            track_id,
            is_instrumental: false,
            synced_lines: Vec::new(),
            plain_lyrics: None,
            translation_lines: Vec::new(),
            source: LyricsSource::None,
            fetch_log: Vec::new(),
        }
    }

    pub fn has_synced(&self) -> bool {
        !self.synced_lines.is_empty()
    }
}
