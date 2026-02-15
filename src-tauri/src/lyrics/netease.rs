use anyhow::{Context, Result};
use reqwest::Client;
use serde::Deserialize;
use std::time::Duration;

use super::lrc_parser::parse_lrc;
use super::types::{LyricsInfo, LyricsLine, LyricsSource};

const SEARCH_URL: &str = "http://music.163.com/api/search/get";
const LYRIC_URL: &str = "http://music.163.com/api/song/lyric";
const USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36";
const TIMEOUT: Duration = Duration::from_secs(5);

pub struct NetEaseClient {
    client: Client,
}

#[derive(Debug, Deserialize)]
struct SearchResponse {
    result: Option<SearchResult>,
    code: i32,
}

#[derive(Debug, Deserialize)]
struct SearchResult {
    songs: Option<Vec<Song>>,
}

#[derive(Debug, Deserialize)]
struct Song {
    id: u64,
    name: String,
    artists: Vec<Artist>,
}

#[derive(Debug, Deserialize)]
struct Artist {
    name: String,
}

#[derive(Debug, Deserialize)]
struct LyricResponse {
    lrc: Option<LyricContent>,
    tlyric: Option<LyricContent>,
    code: i32,
}

#[derive(Debug, Deserialize)]
struct LyricContent {
    lyric: Option<String>,
}

impl NetEaseClient {
    pub fn new(client: Client) -> Self {
        Self { client }
    }

    pub async fn fetch_lyrics(
        &self,
        track_name: &str,
        artist: &str,
    ) -> Result<Option<LyricsInfo>> {
        // Search for the song
        let song_id = self.search_song(track_name, artist).await?;
        let Some(song_id) = song_id else {
            return Ok(None);
        };

        // Fetch lyrics by song ID
        self.get_lyrics(song_id).await
    }

    async fn search_song(&self, track_name: &str, artist: &str) -> Result<Option<u64>> {
        let query = format!("{} {}", track_name, artist);

        let resp = self
            .client
            .post(SEARCH_URL)
            .header("User-Agent", USER_AGENT)
            .header("Referer", "https://music.163.com")
            .form(&[("s", query.as_str()), ("type", "1"), ("limit", "5")])
            .timeout(TIMEOUT)
            .send()
            .await
            .context("NetEase search request failed")?;

        let data: SearchResponse = resp
            .json()
            .await
            .context("Failed to parse NetEase search response")?;

        if data.code != 200 {
            return Ok(None);
        }

        let songs = match data.result.and_then(|r| r.songs) {
            Some(s) if !s.is_empty() => s,
            _ => return Ok(None),
        };

        // Try to find a good match
        let track_lower = normalize(track_name);
        let artist_lower = normalize(artist);

        // Prefer exact name match
        for song in &songs {
            let song_name = normalize(&song.name);
            let song_artist = song
                .artists
                .iter()
                .map(|a| normalize(&a.name))
                .collect::<Vec<_>>()
                .join(" ");

            if song_name == track_lower
                || (song_name.contains(&track_lower) && song_artist.contains(&artist_lower))
            {
                return Ok(Some(song.id));
            }
        }

        // Fall back to first result if the name roughly matches
        let first = &songs[0];
        let first_name = normalize(&first.name);
        if first_name.contains(&track_lower) || track_lower.contains(&first_name) {
            return Ok(Some(first.id));
        }

        Ok(None)
    }

    async fn get_lyrics(&self, song_id: u64) -> Result<Option<LyricsInfo>> {
        let resp = self
            .client
            .get(LYRIC_URL)
            .header("User-Agent", USER_AGENT)
            .header("Referer", "https://music.163.com")
            .query(&[
                ("id", song_id.to_string()),
                ("lv", "1".to_string()),
                ("kv", "1".to_string()),
                ("tv", "-1".to_string()),
            ])
            .timeout(TIMEOUT)
            .send()
            .await
            .context("NetEase lyrics request failed")?;

        let data: LyricResponse = resp
            .json()
            .await
            .context("Failed to parse NetEase lyrics response")?;

        if data.code != 200 {
            return Ok(None);
        }

        let lrc_text = data
            .lrc
            .and_then(|l| l.lyric)
            .unwrap_or_default();

        if lrc_text.is_empty() {
            return Ok(None);
        }

        let synced_lines = parse_lrc(&lrc_text);

        // Check if the parsed lyrics actually have content (not just metadata)
        let has_content = synced_lines.iter().any(|l| !l.text.is_empty());
        if !has_content && synced_lines.is_empty() {
            return Ok(None);
        }

        // Parse translation lyrics
        let translation_lines: Vec<LyricsLine> = data
            .tlyric
            .and_then(|t| t.lyric)
            .map(|text| parse_lrc(&text))
            .unwrap_or_default();

        // If no synced lines but we have raw text, provide as plain
        let plain_lyrics = if synced_lines.is_empty() {
            Some(lrc_text)
        } else {
            None
        };

        Ok(Some(LyricsInfo {
            track_id: String::new(), // Will be set by fetcher
            is_instrumental: false,
            synced_lines,
            plain_lyrics,
            translation_lines,
            source: LyricsSource::NetEase,
        }))
    }
}

/// Normalize a string for fuzzy comparison: lowercase, trim, strip parenthetical suffixes.
fn normalize(s: &str) -> String {
    let s = s.to_lowercase().trim().to_string();
    // Strip trailing parenthetical like " (Deluxe Edition)" or " （Live）"
    if let Some(idx) = s.find('(') {
        s[..idx].trim().to_string()
    } else if let Some(idx) = s.find('（') {
        s[..idx].trim().to_string()
    } else {
        s
    }
}
