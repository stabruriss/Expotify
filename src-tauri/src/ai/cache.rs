use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Simple in-memory cache for track AI descriptions
pub struct TrackInfoCache {
    cache: Arc<RwLock<HashMap<String, String>>>,
    max_size: usize,
}

impl TrackInfoCache {
    pub fn new(max_size: usize) -> Self {
        Self {
            cache: Arc::new(RwLock::new(HashMap::new())),
            max_size,
        }
    }

    /// Get cached description for a track
    pub async fn get(&self, track_id: &str) -> Option<String> {
        self.cache.read().await.get(track_id).cloned()
    }

    /// Store description for a track
    pub async fn set(&self, track_id: String, description: String) {
        let mut cache = self.cache.write().await;

        // Simple eviction: if at capacity, remove oldest entry
        if cache.len() >= self.max_size {
            if let Some(key) = cache.keys().next().cloned() {
                cache.remove(&key);
            }
        }

        cache.insert(track_id, description);
    }

    /// Check if track is cached
    pub async fn contains(&self, track_id: &str) -> bool {
        self.cache.read().await.contains_key(track_id)
    }

    /// Clear the cache
    pub async fn clear(&self) {
        self.cache.write().await.clear();
    }

    /// Get cache size
    pub async fn len(&self) -> usize {
        self.cache.read().await.len()
    }

    /// Check if cache is empty
    pub async fn is_empty(&self) -> bool {
        self.cache.read().await.is_empty()
    }
}

impl Default for TrackInfoCache {
    fn default() -> Self {
        Self::new(100) // Default cache size of 100 tracks
    }
}
