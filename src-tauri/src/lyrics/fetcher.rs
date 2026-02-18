use anyhow::Result;
use reqwest::Client;

use super::cache::LyricsCache;
use super::instrumental::is_likely_instrumental;
use super::kugou::KugouClient;
use super::lrclib::LrclibClient;
use super::netease::NetEaseClient;
use super::petitlyrics::PetitLyricsClient;
use super::qqmusic::QQMusicClient;
use super::types::{LyricsInfo, LyricsSource};

pub struct LyricsFetcher {
    netease: NetEaseClient,
    qqmusic: QQMusicClient,
    kugou: KugouClient,
    lrclib: LrclibClient,
    petitlyrics: PetitLyricsClient,
    cache: LyricsCache,
}

impl LyricsFetcher {
    pub fn new() -> Self {
        let client = Client::new();
        Self {
            netease: NetEaseClient::new(client.clone()),
            qqmusic: QQMusicClient::new(client.clone()),
            kugou: KugouClient::new(client.clone()),
            lrclib: LrclibClient::new(client.clone()),
            petitlyrics: PetitLyricsClient::new(client),
            cache: LyricsCache::default(),
        }
    }

    /// Fetch lyrics with waterfall: NetEase → QQ Music → Kugou → PetitLyrics → LRCLIB
    /// If `force` is true, bypass the cache and re-fetch.
    pub async fn get_lyrics(
        &self,
        track_id: &str,
        track_name: &str,
        artist: &str,
        album: &str,
        duration_ms: u64,
        force: bool,
    ) -> Result<LyricsInfo> {
        // 1. Check cache (skip if forcing)
        if force {
            self.cache.remove(track_id).await;
        } else if let Some(cached) = self.cache.get(track_id).await {
            return Ok(cached);
        }

        let mut fetch_log: Vec<String> = Vec::new();

        // 2. Quick keyword-based instrumental check
        if is_likely_instrumental(track_name) {
            fetch_log.push(format!("Detected instrumental track: \"{}\"", track_name));
            let mut result = LyricsInfo::instrumental(track_id.to_string());
            result.fetch_log = fetch_log;
            self.cache.set(track_id.to_string(), result.clone()).await;
            return Ok(result);
        }

        // 3. Try NetEase (primary source)
        match self.netease.fetch_lyrics(track_name, artist).await {
            Ok(Some(mut lyrics)) => {
                let synced_count = lyrics.synced_lines.len();
                let trans_count = lyrics.translation_lines.len();
                let plain = lyrics.plain_lyrics.is_some();
                fetch_log.push(format!(
                    "NetEase: found (synced: {} lines, translations: {}, plain: {})",
                    synced_count, trans_count, plain
                ));
                lyrics.track_id = track_id.to_string();
                lyrics.source = LyricsSource::NetEase;
                lyrics.fetch_log = fetch_log;
                self.cache.set(track_id.to_string(), lyrics.clone()).await;
                return Ok(lyrics);
            }
            Ok(None) => {
                fetch_log.push("NetEase: no match".to_string());
                log::debug!("NetEase: no lyrics for '{}' - '{}'", track_name, artist);
            }
            Err(e) => {
                fetch_log.push(format!("NetEase: error — {}", e));
                log::warn!("NetEase error: {}", e);
            }
        }

        // 4. Try QQ Music
        match self.qqmusic.fetch_lyrics(track_name, artist).await {
            Ok(Some(mut lyrics)) => {
                let synced_count = lyrics.synced_lines.len();
                let trans_count = lyrics.translation_lines.len();
                let plain = lyrics.plain_lyrics.is_some();
                fetch_log.push(format!(
                    "QQ Music: found (synced: {} lines, translations: {}, plain: {})",
                    synced_count, trans_count, plain
                ));
                lyrics.track_id = track_id.to_string();
                lyrics.source = LyricsSource::QQMusic;
                lyrics.fetch_log = fetch_log;
                self.cache.set(track_id.to_string(), lyrics.clone()).await;
                return Ok(lyrics);
            }
            Ok(None) => {
                fetch_log.push("QQ Music: no match".to_string());
                log::debug!("QQ Music: no lyrics for '{}' - '{}'", track_name, artist);
            }
            Err(e) => {
                fetch_log.push(format!("QQ Music: error — {}", e));
                log::warn!("QQ Music error: {}", e);
            }
        }

        // 5. Try Kugou
        match self
            .kugou
            .fetch_lyrics(track_name, artist, duration_ms)
            .await
        {
            Ok(Some(mut lyrics)) => {
                let synced_count = lyrics.synced_lines.len();
                let plain = lyrics.plain_lyrics.is_some();
                fetch_log.push(format!(
                    "Kugou: found (synced: {} lines, plain: {})",
                    synced_count, plain
                ));
                lyrics.track_id = track_id.to_string();
                lyrics.source = LyricsSource::Kugou;
                lyrics.fetch_log = fetch_log;
                self.cache.set(track_id.to_string(), lyrics.clone()).await;
                return Ok(lyrics);
            }
            Ok(None) => {
                fetch_log.push("Kugou: no match".to_string());
                log::debug!("Kugou: no lyrics for '{}' - '{}'", track_name, artist);
            }
            Err(e) => {
                fetch_log.push(format!("Kugou: error — {}", e));
                log::warn!("Kugou error: {}", e);
            }
        }

        // 6. Try PetitLyrics (Japanese music fallback)
        match self.petitlyrics.fetch_lyrics(track_name, artist).await {
            Ok(Some(mut lyrics)) => {
                let synced_count = lyrics.synced_lines.len();
                let plain = lyrics.plain_lyrics.is_some();
                fetch_log.push(format!(
                    "PetitLyrics: found (synced: {} lines, plain: {})",
                    synced_count, plain
                ));
                lyrics.track_id = track_id.to_string();
                lyrics.source = LyricsSource::PetitLyrics;
                lyrics.fetch_log = fetch_log;
                self.cache.set(track_id.to_string(), lyrics.clone()).await;
                return Ok(lyrics);
            }
            Ok(None) => {
                fetch_log.push("PetitLyrics: no match".to_string());
                log::debug!("PetitLyrics: no lyrics for '{}' - '{}'", track_name, artist);
            }
            Err(e) => {
                fetch_log.push(format!("PetitLyrics: error — {}", e));
                log::warn!("PetitLyrics error: {}", e);
            }
        }

        // 7. Try LRCLIB (last resort, with retry + search fallback)
        match self
            .lrclib
            .fetch_lyrics(track_name, artist, album, duration_ms)
            .await
        {
            Ok(Some(mut lyrics)) => {
                if lyrics.is_instrumental {
                    fetch_log.push("LRCLIB: instrumental".to_string());
                    let mut result = LyricsInfo::instrumental(track_id.to_string());
                    result.fetch_log = fetch_log;
                    self.cache.set(track_id.to_string(), result.clone()).await;
                    return Ok(result);
                }
                let synced_count = lyrics.synced_lines.len();
                let plain = lyrics.plain_lyrics.is_some();
                fetch_log.push(format!(
                    "LRCLIB: found (synced: {} lines, plain: {})",
                    synced_count, plain
                ));
                lyrics.track_id = track_id.to_string();
                lyrics.source = LyricsSource::Lrclib;
                lyrics.fetch_log = fetch_log;
                self.cache.set(track_id.to_string(), lyrics.clone()).await;
                return Ok(lyrics);
            }
            Ok(None) => {
                fetch_log.push("LRCLIB: no match (tried /api/get + /api/search)".to_string());
                log::debug!("LRCLIB: no lyrics for '{}' - '{}'", track_name, artist);
            }
            Err(e) => {
                fetch_log.push(format!("LRCLIB: error after retries — {}", e));
                log::warn!("LRCLIB error after retries: {}", e);
            }
        }

        // 8. All sources failed
        fetch_log.push("All sources exhausted".to_string());
        let mut result = LyricsInfo::not_found(track_id.to_string());
        result.fetch_log = fetch_log;
        self.cache.set(track_id.to_string(), result.clone()).await;
        Ok(result)
    }
}
