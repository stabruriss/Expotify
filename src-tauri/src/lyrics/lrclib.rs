use anyhow::{Context, Result};
use reqwest::Client;
use serde::Deserialize;
use std::time::Duration;

use super::lrc_parser::parse_lrc;
use super::types::{LyricsInfo, LyricsSource};

const API_GET_URL: &str = "https://lrclib.net/api/get";
const API_SEARCH_URL: &str = "https://lrclib.net/api/search";
const USER_AGENT: &str = "Expotify/0.1.0 (https://github.com/stabruriss/Expotify)";

/// Timeouts per attempt: 5s → 8s → 12s (progressively longer for a flaky server)
const TIMEOUTS: [Duration; 3] = [
    Duration::from_secs(5),
    Duration::from_secs(8),
    Duration::from_secs(12),
];
/// Backoff delays between retries
const BACKOFF: [Duration; 2] = [Duration::from_secs(1), Duration::from_secs(3)];

pub struct LrclibClient {
    client: Client,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LrclibResponse {
    instrumental: Option<bool>,
    plain_lyrics: Option<String>,
    synced_lyrics: Option<String>,
    #[serde(default)]
    duration: Option<f64>,
}

impl LrclibClient {
    pub fn new(client: Client) -> Self {
        Self { client }
    }

    pub async fn fetch_lyrics(
        &self,
        track_name: &str,
        artist: &str,
        album: &str,
        duration_ms: u64,
    ) -> Result<Option<LyricsInfo>> {
        // Try exact-match /api/get with retries
        match self
            .fetch_get_with_retry(track_name, artist, album, duration_ms)
            .await
        {
            Ok(Some(info)) => return Ok(Some(info)),
            Ok(None) => {
                // 404 from /api/get — fall through to search
                log::debug!("LRCLIB /api/get returned 404, trying /api/search");
            }
            Err(e) => {
                // All retries exhausted on transient errors — fall through to search
                log::warn!("LRCLIB /api/get failed after retries: {}", e);
            }
        }

        // Fallback: /api/search is more lenient (no exact duration/album match needed)
        self.fetch_search_with_retry(track_name, artist, duration_ms)
            .await
    }

    /// /api/get with retry + exponential backoff for transient errors
    async fn fetch_get_with_retry(
        &self,
        track_name: &str,
        artist: &str,
        album: &str,
        duration_ms: u64,
    ) -> Result<Option<LyricsInfo>> {
        let duration_secs = (duration_ms / 1000).to_string();
        let mut last_err = None;

        for attempt in 0..TIMEOUTS.len() {
            if attempt > 0 {
                tokio::time::sleep(BACKOFF[attempt - 1]).await;
                log::info!(
                    "LRCLIB /api/get retry {} for '{}' - '{}'",
                    attempt,
                    track_name,
                    artist
                );
            }

            match self
                .client
                .get(API_GET_URL)
                .header("User-Agent", USER_AGENT)
                .query(&[
                    ("track_name", track_name),
                    ("artist_name", artist),
                    ("album_name", album),
                    ("duration", duration_secs.as_str()),
                ])
                .timeout(TIMEOUTS[attempt])
                .send()
                .await
            {
                Ok(resp) => {
                    if resp.status() == reqwest::StatusCode::NOT_FOUND {
                        return Ok(None);
                    }
                    if resp.status().is_server_error() {
                        let status = resp.status();
                        last_err = Some(anyhow::anyhow!("LRCLIB server error: {}", status));
                        log::warn!("LRCLIB /api/get attempt {} got {}", attempt + 1, status);
                        continue; // retry on 5xx
                    }
                    let data: LrclibResponse = resp
                        .error_for_status()
                        .context("LRCLIB returned an error")?
                        .json()
                        .await
                        .context("Failed to parse LRCLIB response")?;
                    return Ok(Self::parse_response(data));
                }
                Err(e) => {
                    log::warn!("LRCLIB /api/get attempt {} failed: {}", attempt + 1, e);
                    last_err = Some(e.into());
                    continue; // retry on timeout/connection error
                }
            }
        }

        Err(last_err.unwrap_or_else(|| anyhow::anyhow!("LRCLIB /api/get: all retries exhausted")))
    }

    /// /api/search fallback with retry (more lenient matching)
    async fn fetch_search_with_retry(
        &self,
        track_name: &str,
        artist: &str,
        duration_ms: u64,
    ) -> Result<Option<LyricsInfo>> {
        let query = format!("{} {}", track_name, artist);
        let duration_secs = duration_ms as f64 / 1000.0;
        let mut last_err = None;

        for attempt in 0..TIMEOUTS.len() {
            if attempt > 0 {
                tokio::time::sleep(BACKOFF[attempt - 1]).await;
                log::info!("LRCLIB /api/search retry {} for '{}'", attempt, query);
            }

            match self
                .client
                .get(API_SEARCH_URL)
                .header("User-Agent", USER_AGENT)
                .query(&[("q", query.as_str())])
                .timeout(TIMEOUTS[attempt])
                .send()
                .await
            {
                Ok(resp) => {
                    if resp.status().is_server_error() {
                        let status = resp.status();
                        last_err = Some(anyhow::anyhow!("LRCLIB search server error: {}", status));
                        log::warn!("LRCLIB /api/search attempt {} got {}", attempt + 1, status);
                        continue;
                    }
                    let results: Vec<LrclibResponse> = resp
                        .error_for_status()
                        .context("LRCLIB search returned an error")?
                        .json()
                        .await
                        .context("Failed to parse LRCLIB search response")?;

                    // Pick best match: prefer synced lyrics, duration within ±5s
                    let best = results
                        .into_iter()
                        .filter(|r| {
                            r.duration
                                .map(|d| (d - duration_secs).abs() < 5.0)
                                .unwrap_or(true)
                        })
                        .filter(|r| {
                            r.synced_lyrics.is_some()
                                || r.plain_lyrics.is_some()
                                || r.instrumental == Some(true)
                        })
                        .min_by_key(|r| {
                            // Prefer synced, then instrumental, then plain
                            if r.synced_lyrics.is_some() {
                                0
                            } else if r.instrumental == Some(true) {
                                1
                            } else {
                                2
                            }
                        });

                    return Ok(best.and_then(Self::parse_response));
                }
                Err(e) => {
                    log::warn!("LRCLIB /api/search attempt {} failed: {}", attempt + 1, e);
                    last_err = Some(e.into());
                    continue;
                }
            }
        }

        Err(last_err
            .unwrap_or_else(|| anyhow::anyhow!("LRCLIB /api/search: all retries exhausted")))
    }

    fn parse_response(data: LrclibResponse) -> Option<LyricsInfo> {
        if data.instrumental == Some(true) {
            return Some(LyricsInfo {
                track_id: String::new(),
                is_instrumental: true,
                synced_lines: Vec::new(),
                plain_lyrics: None,
                translation_lines: Vec::new(),
                source: LyricsSource::Lrclib,
                fetch_log: Vec::new(),
            });
        }

        let synced_lines = data
            .synced_lyrics
            .as_deref()
            .map(parse_lrc)
            .unwrap_or_default();

        let plain_lyrics = if synced_lines.is_empty() {
            data.plain_lyrics.clone()
        } else {
            None
        };

        if synced_lines.is_empty() && plain_lyrics.is_none() {
            return None;
        }

        Some(LyricsInfo {
            track_id: String::new(),
            is_instrumental: false,
            synced_lines,
            plain_lyrics,
            translation_lines: Vec::new(),
            source: LyricsSource::Lrclib,
            fetch_log: Vec::new(),
        })
    }
}
