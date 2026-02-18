use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;

use super::types::LyricsInfo;

/// Lyrics cache with in-memory layer backed by file-based persistence.
/// Files are stored at ~/.config/expotify/lyrics_cache/{track_id}.json
pub struct LyricsCache {
    mem: Arc<RwLock<HashMap<String, LyricsInfo>>>,
    dir: PathBuf,
}

impl LyricsCache {
    pub fn new() -> Self {
        let dir = dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("expotify")
            .join("lyrics_cache");
        let _ = std::fs::create_dir_all(&dir);
        Self {
            mem: Arc::new(RwLock::new(HashMap::new())),
            dir,
        }
    }

    fn file_path(&self, track_id: &str) -> PathBuf {
        // Sanitize track_id for use as filename
        let safe_id: String = track_id
            .chars()
            .map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
            .collect();
        self.dir.join(format!("{}.json", safe_id))
    }

    pub async fn get(&self, track_id: &str) -> Option<LyricsInfo> {
        // Check in-memory first
        if let Some(cached) = self.mem.read().await.get(track_id).cloned() {
            return Some(cached);
        }

        // Try disk
        let path = self.file_path(track_id);
        let data = tokio::fs::read_to_string(&path).await.ok()?;
        let lyrics: LyricsInfo = serde_json::from_str(&data).ok()?;

        // Promote to memory
        self.mem.write().await.insert(track_id.to_string(), lyrics.clone());
        Some(lyrics)
    }

    pub async fn set(&self, track_id: String, lyrics: LyricsInfo) {
        // Write to disk (best-effort)
        let path = self.file_path(&track_id);
        if let Ok(json) = serde_json::to_string(&lyrics) {
            let _ = tokio::fs::write(&path, json).await;
        }

        // Update memory
        self.mem.write().await.insert(track_id, lyrics);
    }

    pub async fn remove(&self, track_id: &str) {
        self.mem.write().await.remove(track_id);
        let path = self.file_path(track_id);
        let _ = tokio::fs::remove_file(&path).await;
    }
}

impl Default for LyricsCache {
    fn default() -> Self {
        Self::new()
    }
}
