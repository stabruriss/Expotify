use anyhow::{Context, Result};
use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use reqwest::Client;
use serde::Deserialize;
use serde_json::json;
use std::time::Duration;

use super::lrc_parser::parse_lrc;
use super::types::{LyricsInfo, LyricsLine, LyricsSource};

const SEARCH_URL: &str = "https://u.y.qq.com/cgi-bin/musicu.fcg";
const LYRIC_URL: &str = "https://c.y.qq.com/lyric/fcgi-bin/fcg_query_lyric_new.fcg";
const USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36";
const TIMEOUT: Duration = Duration::from_secs(5);

pub struct QQMusicClient {
    client: Client,
}

// --- Search response types ---

#[derive(Debug, Deserialize)]
struct QQSearchWrapper {
    #[serde(rename = "music.search.SearchCgiService")]
    search_service: Option<QQSearchService>,
}

#[derive(Debug, Deserialize)]
struct QQSearchService {
    data: Option<QQSearchData>,
}

#[derive(Debug, Deserialize)]
struct QQSearchData {
    body: Option<QQSearchBody>,
}

#[derive(Debug, Deserialize)]
struct QQSearchBody {
    song: Option<QQSongList>,
}

#[derive(Debug, Deserialize)]
struct QQSongList {
    list: Option<Vec<QQSong>>,
}

#[derive(Debug, Deserialize)]
struct QQSong {
    mid: String,
    name: String,
    singer: Vec<QQSinger>,
}

#[derive(Debug, Deserialize)]
struct QQSinger {
    name: String,
}

// --- Lyrics response types ---

#[derive(Debug, Deserialize)]
struct QQLyricResponse {
    lyric: Option<String>,
    trans: Option<String>,
    retcode: Option<i32>,
}

impl QQMusicClient {
    pub fn new(client: Client) -> Self {
        Self { client }
    }

    pub async fn fetch_lyrics(
        &self,
        track_name: &str,
        artist: &str,
    ) -> Result<Option<LyricsInfo>> {
        let songmid = self.search_song(track_name, artist).await?;
        let Some(songmid) = songmid else {
            return Ok(None);
        };

        self.get_lyrics(&songmid).await
    }

    async fn search_song(&self, track_name: &str, artist: &str) -> Result<Option<String>> {
        let query = format!("{} {}", track_name, artist);

        let body = json!({
            "music.search.SearchCgiService": {
                "method": "DoSearchForQQMusicDesktop",
                "module": "music.search.SearchCgiService",
                "param": {
                    "search_type": 0,
                    "query": query,
                    "page_num": 1,
                    "num_per_page": 5
                }
            }
        });

        let resp = self
            .client
            .post(SEARCH_URL)
            .header("User-Agent", USER_AGENT)
            .header("Referer", "https://y.qq.com")
            .json(&body)
            .timeout(TIMEOUT)
            .send()
            .await
            .context("QQ Music search request failed")?;

        let data: QQSearchWrapper = resp
            .json()
            .await
            .context("Failed to parse QQ Music search response")?;

        let songs = match data
            .search_service
            .and_then(|s| s.data)
            .and_then(|d| d.body)
            .and_then(|b| b.song)
            .and_then(|s| s.list)
        {
            Some(s) if !s.is_empty() => s,
            _ => return Ok(None),
        };

        let track_lower = normalize(track_name);
        let artist_lower = normalize(artist);

        // Prefer exact name match
        for song in &songs {
            let song_name = normalize(&song.name);
            let song_artist = song
                .singer
                .iter()
                .map(|s| normalize(&s.name))
                .collect::<Vec<_>>()
                .join(" ");

            if song_name == track_lower
                || (song_name.contains(&track_lower) && song_artist.contains(&artist_lower))
            {
                return Ok(Some(song.mid.clone()));
            }
        }

        // Fall back to first result if the name roughly matches
        let first = &songs[0];
        let first_name = normalize(&first.name);
        if first_name.contains(&track_lower) || track_lower.contains(&first_name) {
            return Ok(Some(first.mid.clone()));
        }

        Ok(None)
    }

    async fn get_lyrics(&self, songmid: &str) -> Result<Option<LyricsInfo>> {
        let resp = self
            .client
            .get(LYRIC_URL)
            .header("User-Agent", USER_AGENT)
            .header("Referer", "https://y.qq.com")
            .query(&[
                ("songmid", songmid),
                ("format", "json"),
                ("nobase64", "0"),
                ("g_tk", "5381"),
            ])
            .timeout(TIMEOUT)
            .send()
            .await
            .context("QQ Music lyrics request failed")?;

        let body = resp
            .text()
            .await
            .context("QQ Music lyrics: failed to read response body")?;

        // Defensively strip JSONP wrapper if present
        let json_str = strip_jsonp(&body);

        let data: QQLyricResponse = serde_json::from_str(json_str)
            .context("Failed to parse QQ Music lyrics response")?;

        if data.retcode.unwrap_or(-1) != 0 {
            return Ok(None);
        }

        // Decode base64 lyrics
        let lrc_text = match &data.lyric {
            Some(b64) if !b64.is_empty() => {
                let bytes = STANDARD
                    .decode(b64)
                    .context("QQ Music: base64 decode failed for lyrics")?;
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

        // Decode translation if present
        let translation_lines: Vec<LyricsLine> = match &data.trans {
            Some(b64) if !b64.is_empty() => {
                let bytes = STANDARD.decode(b64).unwrap_or_default();
                let text = String::from_utf8_lossy(&bytes);
                parse_lrc(&text)
            }
            _ => Vec::new(),
        };

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
            translation_lines,
            source: LyricsSource::QQMusic,
            fetch_log: Vec::new(),
        }))
    }
}

/// Strip JSONP callback wrapper if present, e.g. `callback({...})` → `{...}`
fn strip_jsonp(body: &str) -> &str {
    let trimmed = body.trim();
    if let Some(start) = trimmed.find('(') {
        if let Some(end) = trimmed.rfind(')') {
            if start < end {
                return &trimmed[start + 1..end];
            }
        }
    }
    trimmed
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
