use anyhow::Result;
use reqwest::Client;

use super::types::LyricsInfo;

/// PetitLyrics client for Japanese music lyrics.
/// Currently a stub — full implementation can be added later.
pub struct PetitLyricsClient {
    _client: Client,
}

impl PetitLyricsClient {
    pub fn new(client: Client) -> Self {
        Self { _client: client }
    }

    pub async fn fetch_lyrics(
        &self,
        _track_name: &str,
        _artist: &str,
    ) -> Result<Option<LyricsInfo>> {
        // TODO: Implement PetitLyrics API integration
        // Endpoint: https://pl.t.petitlyrics.com/mh/1/lyrics/list.xml
        // Requires auth_key parameter
        Ok(None)
    }
}
