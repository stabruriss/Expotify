use anyhow::Result;
use reqwest::Client;

use super::cache::LyricsCache;
use super::instrumental::is_likely_instrumental;
use super::lrclib::LrclibClient;
use super::netease::NetEaseClient;
use super::petitlyrics::PetitLyricsClient;
use super::types::{LyricsInfo, LyricsSource};

pub struct LyricsFetcher {
    netease: NetEaseClient,
    lrclib: LrclibClient,
    petitlyrics: PetitLyricsClient,
    cache: LyricsCache,
}

impl LyricsFetcher {
    pub fn new() -> Self {
        let client = Client::new();
        Self {
            netease: NetEaseClient::new(client.clone()),
            lrclib: LrclibClient::new(client.clone()),
            petitlyrics: PetitLyricsClient::new(client),
            cache: LyricsCache::default(),
        }
    }

    /// Fetch lyrics with waterfall: NetEase → LRCLIB → PetitLyrics
    pub async fn get_lyrics(
        &self,
        track_id: &str,
        track_name: &str,
        artist: &str,
        album: &str,
        duration_ms: u64,
    ) -> Result<LyricsInfo> {
        // 1. Check cache
        if let Some(cached) = self.cache.get(track_id).await {
            return Ok(cached);
        }

        // 2. Quick keyword-based instrumental check
        if is_likely_instrumental(track_name) {
            let result = LyricsInfo::instrumental(track_id.to_string());
            self.cache.set(track_id.to_string(), result.clone()).await;
            return Ok(result);
        }

        // 3. Try NetEase (primary source)
        match self.netease.fetch_lyrics(track_name, artist).await {
            Ok(Some(mut lyrics)) => {
                lyrics.track_id = track_id.to_string();
                lyrics.source = LyricsSource::NetEase;
                self.cache.set(track_id.to_string(), lyrics.clone()).await;
                return Ok(lyrics);
            }
            Ok(None) => {
                log::debug!("NetEase: no lyrics for '{}' - '{}'", track_name, artist);
            }
            Err(e) => {
                log::warn!("NetEase error: {}", e);
            }
        }

        // 4. Try LRCLIB (fallback)
        match self
            .lrclib
            .fetch_lyrics(track_name, artist, album, duration_ms)
            .await
        {
            Ok(Some(mut lyrics)) => {
                if lyrics.is_instrumental {
                    let result = LyricsInfo::instrumental(track_id.to_string());
                    self.cache.set(track_id.to_string(), result.clone()).await;
                    return Ok(result);
                }
                lyrics.track_id = track_id.to_string();
                lyrics.source = LyricsSource::Lrclib;
                self.cache.set(track_id.to_string(), lyrics.clone()).await;
                return Ok(lyrics);
            }
            Ok(None) => {
                log::debug!("LRCLIB: no lyrics for '{}' - '{}'", track_name, artist);
            }
            Err(e) => {
                log::warn!("LRCLIB error: {}", e);
            }
        }

        // 5. Try PetitLyrics (Japanese music fallback)
        match self.petitlyrics.fetch_lyrics(track_name, artist).await {
            Ok(Some(mut lyrics)) => {
                lyrics.track_id = track_id.to_string();
                lyrics.source = LyricsSource::PetitLyrics;
                self.cache.set(track_id.to_string(), lyrics.clone()).await;
                return Ok(lyrics);
            }
            Ok(None) => {
                log::debug!("PetitLyrics: no lyrics for '{}' - '{}'", track_name, artist);
            }
            Err(e) => {
                log::warn!("PetitLyrics error: {}", e);
            }
        }

        // 6. All sources failed
        let result = LyricsInfo::not_found(track_id.to_string());
        self.cache.set(track_id.to_string(), result.clone()).await;
        Ok(result)
    }
}
