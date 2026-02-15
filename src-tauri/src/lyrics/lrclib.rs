use anyhow::{Context, Result};
use reqwest::Client;
use serde::Deserialize;
use std::time::Duration;

use super::lrc_parser::parse_lrc;
use super::types::{LyricsInfo, LyricsSource};

const API_URL: &str = "https://lrclib.net/api/get";
const USER_AGENT: &str = "Expotify/0.1.0 (https://github.com/stabruriss/Expotify)";
const TIMEOUT: Duration = Duration::from_secs(5);

pub struct LrclibClient {
    client: Client,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LrclibResponse {
    instrumental: Option<bool>,
    plain_lyrics: Option<String>,
    synced_lyrics: Option<String>,
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
        let duration_secs = (duration_ms / 1000).to_string();

        let resp = self
            .client
            .get(API_URL)
            .header("User-Agent", USER_AGENT)
            .query(&[
                ("track_name", track_name),
                ("artist_name", artist),
                ("album_name", album),
                ("duration", &duration_secs),
            ])
            .timeout(TIMEOUT)
            .send()
            .await
            .context("LRCLIB request failed")?;

        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }

        let data: LrclibResponse = resp
            .error_for_status()
            .context("LRCLIB returned an error")?
            .json()
            .await
            .context("Failed to parse LRCLIB response")?;

        // Check instrumental flag
        if data.instrumental == Some(true) {
            return Ok(Some(LyricsInfo {
                track_id: String::new(),
                is_instrumental: true,
                synced_lines: Vec::new(),
                plain_lyrics: None,
                translation_lines: Vec::new(),
                source: LyricsSource::Lrclib,
            }));
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

        // No lyrics at all
        if synced_lines.is_empty() && plain_lyrics.is_none() {
            return Ok(None);
        }

        Ok(Some(LyricsInfo {
            track_id: String::new(),
            is_instrumental: false,
            synced_lines,
            plain_lyrics,
            translation_lines: Vec::new(),
            source: LyricsSource::Lrclib,
        }))
    }
}
