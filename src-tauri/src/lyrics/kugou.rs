use anyhow::{Context, Result};
use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use reqwest::Client;
use serde::Deserialize;
use std::time::Duration;

use super::lrc_parser::parse_lrc;
use super::types::{LyricsInfo, LyricsSource};

const SEARCH_URL: &str = "http://mobilecdn.kugou.com/api/v3/search/song";
const LYRICS_SEARCH_URL: &str = "http://krcs.kugou.com/search";
const LYRICS_DOWNLOAD_URL: &str = "http://lyrics.kugou.com/download";
const USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36";
const TIMEOUT: Duration = Duration::from_secs(5);

pub struct KugouClient {
    client: Client,
}

// --- Song search response types ---

#[derive(Debug, Deserialize)]
struct KugouSearchResponse {
    status: Option<i32>,
    data: Option<KugouSearchData>,
}

#[derive(Debug, Deserialize)]
struct KugouSearchData {
    info: Option<Vec<KugouSongInfo>>,
}

#[derive(Debug, Deserialize)]
struct KugouSongInfo {
    hash: String,
    duration: u64, // seconds
    songname: String,
    singername: String,
}

// --- Lyrics search response types ---

#[derive(Debug, Deserialize)]
struct KugouLyricsSearchResponse {
    status: Option<i32>,
    candidates: Option<Vec<KugouLyricsCandidate>>,
}

#[derive(Debug, Deserialize)]
struct KugouLyricsCandidate {
    id: String,
    accesskey: String,
}

// --- Lyrics download response types ---

#[derive(Debug, Deserialize)]
struct KugouLyricsDownloadResponse {
    status: Option<i32>,
    content: Option<String>, // Base64-encoded LRC
}

impl KugouClient {
    pub fn new(client: Client) -> Self {
        Self { client }
    }

    pub async fn fetch_lyrics(
        &self,
        track_name: &str,
        artist: &str,
        duration_ms: u64,
    ) -> Result<Option<LyricsInfo>> {
        // Step A: Search for the song to get hash
        let song = self.search_song(track_name, artist).await?;
        let Some(song) = song else {
            return Ok(None);
        };

        // Step B: Search for lyrics using hash + duration from Spotify
        let candidate = self.search_lyrics(&song.hash, duration_ms).await?;
        let Some(candidate) = candidate else {
            return Ok(None);
        };

        // Step C: Download lyrics
        self.download_lyrics(&candidate.id, &candidate.accesskey)
            .await
    }

    async fn search_song(&self, track_name: &str, artist: &str) -> Result<Option<KugouSongInfo>> {
        let keyword = format!("{} {}", track_name, artist);

        let resp = self
            .client
            .get(SEARCH_URL)
            .header("User-Agent", USER_AGENT)
            .query(&[
                ("keyword", keyword.as_str()),
                ("page", "1"),
                ("pagesize", "5"),
            ])
            .timeout(TIMEOUT)
            .send()
            .await
            .context("Kugou search request failed")?;

        let data: KugouSearchResponse = resp
            .json()
            .await
            .context("Failed to parse Kugou search response")?;

        if data.status != Some(1) {
            return Ok(None);
        }

        let songs = match data.data.and_then(|d| d.info) {
            Some(s) if !s.is_empty() => s,
            _ => return Ok(None),
        };

        let track_lower = normalize(track_name);
        let artist_lower = normalize(artist);

        // Prefer exact name match
        for song in &songs {
            let song_name = normalize(&song.songname);
            let song_artist = normalize(&song.singername);

            if song_name == track_lower
                || (song_name.contains(&track_lower) && song_artist.contains(&artist_lower))
            {
                return Ok(Some(KugouSongInfo {
                    hash: song.hash.clone(),
                    duration: song.duration,
                    songname: song.songname.clone(),
                    singername: song.singername.clone(),
                }));
            }
        }

        // Fall back to first result if the name roughly matches
        let first = &songs[0];
        let first_name = normalize(&first.songname);
        if first_name.contains(&track_lower) || track_lower.contains(&first_name) {
            return Ok(Some(KugouSongInfo {
                hash: first.hash.clone(),
                duration: first.duration,
                songname: first.songname.clone(),
                singername: first.singername.clone(),
            }));
        }

        Ok(None)
    }

    async fn search_lyrics(
        &self,
        hash: &str,
        duration_ms: u64,
    ) -> Result<Option<KugouLyricsCandidate>> {
        let duration_str = duration_ms.to_string();

        let resp = self
            .client
            .get(LYRICS_SEARCH_URL)
            .header("User-Agent", USER_AGENT)
            .query(&[
                ("keyword", ""),
                ("hash", hash),
                ("timelength", &duration_str),
                ("ver", "1"),
                ("man", "yes"),
                ("client", "mobi"),
            ])
            .timeout(TIMEOUT)
            .send()
            .await
            .context("Kugou lyrics search request failed")?;

        let data: KugouLyricsSearchResponse = resp
            .json()
            .await
            .context("Failed to parse Kugou lyrics search response")?;

        if data.status != Some(200) {
            return Ok(None);
        }

        let candidates = match data.candidates {
            Some(c) if !c.is_empty() => c,
            _ => return Ok(None),
        };

        Ok(Some(KugouLyricsCandidate {
            id: candidates[0].id.clone(),
            accesskey: candidates[0].accesskey.clone(),
        }))
    }

    async fn download_lyrics(&self, id: &str, accesskey: &str) -> Result<Option<LyricsInfo>> {
        let resp = self
            .client
            .get(LYRICS_DOWNLOAD_URL)
            .header("User-Agent", USER_AGENT)
            .query(&[
                ("id", id),
                ("accesskey", accesskey),
                ("fmt", "lrc"),
                ("charset", "utf8"),
                ("ver", "1"),
                ("client", "mobi"),
            ])
            .timeout(TIMEOUT)
            .send()
            .await
            .context("Kugou lyrics download request failed")?;

        let data: KugouLyricsDownloadResponse = resp
            .json()
            .await
            .context("Failed to parse Kugou lyrics download response")?;

        if data.status != Some(200) {
            return Ok(None);
        }

        let lrc_text = match &data.content {
            Some(b64) if !b64.is_empty() => {
                let bytes = STANDARD
                    .decode(b64)
                    .context("Kugou: base64 decode failed for lyrics")?;
                String::from_utf8_lossy(&bytes).to_string()
            }
            _ => return Ok(None),
        };

        if lrc_text.is_empty() {
            return Ok(None);
        }

        let synced_lines = parse_lrc(&lrc_text);

        let has_content = synced_lines.iter().any(|l| !l.text.is_empty());
        if !has_content && synced_lines.is_empty() {
            return Ok(None);
        }

        let plain_lyrics = if synced_lines.is_empty() {
            Some(lrc_text)
        } else {
            None
        };

        Ok(Some(LyricsInfo {
            track_id: String::new(),
            is_instrumental: false,
            synced_lines,
            plain_lyrics,
            translation_lines: Vec::new(),
            source: LyricsSource::Kugou,
            fetch_log: Vec::new(),
        }))
    }
}

/// Normalize a string for fuzzy comparison: lowercase, trim, strip parenthetical suffixes.
fn normalize(s: &str) -> String {
    let s = s.to_lowercase().trim().to_string();
    if let Some(idx) = s.find('(') {
        s[..idx].trim().to_string()
    } else if let Some(idx) = s.find('（') {
        s[..idx].trim().to_string()
    } else {
        s
    }
}
