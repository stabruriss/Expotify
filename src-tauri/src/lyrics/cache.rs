use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

use super::types::LyricsInfo;

pub struct LyricsCache {
    cache: Arc<RwLock<HashMap<String, LyricsInfo>>>,
    max_size: usize,
}

impl LyricsCache {
    pub fn new(max_size: usize) -> Self {
        Self {
            cache: Arc::new(RwLock::new(HashMap::new())),
            max_size,
        }
    }

    pub async fn get(&self, track_id: &str) -> Option<LyricsInfo> {
        self.cache.read().await.get(track_id).cloned()
    }

    pub async fn set(&self, track_id: String, lyrics: LyricsInfo) {
        let mut cache = self.cache.write().await;
        if cache.len() >= self.max_size {
            if let Some(key) = cache.keys().next().cloned() {
                cache.remove(&key);
            }
        }
        cache.insert(track_id, lyrics);
    }

    pub async fn clear(&self) {
        self.cache.write().await.clear();
    }
}

impl Default for LyricsCache {
    fn default() -> Self {
        Self::new(50)
    }
}
