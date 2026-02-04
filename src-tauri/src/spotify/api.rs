use anyhow::Result;
use reqwest::Client;

use super::types::CurrentlyPlaying;
use crate::auth::SpotifyAuth;

const SPOTIFY_API_BASE: &str = "https://api.spotify.com/v1";

pub struct SpotifyApi {
    client: Client,
}

impl SpotifyApi {
    pub fn new() -> Self {
        Self {
            client: Client::new(),
        }
    }

    /// Get the currently playing track
    pub async fn get_currently_playing(&self, auth: &SpotifyAuth) -> Result<Option<CurrentlyPlaying>> {
        let token = auth.get_access_token().await?;

        let response = self
            .client
            .get(format!("{}/me/player/currently-playing", SPOTIFY_API_BASE))
            .bearer_auth(&token)
            .send()
            .await?;

        // 204 No Content means nothing is playing
        if response.status() == reqwest::StatusCode::NO_CONTENT {
            return Ok(None);
        }

        // Check for other error statuses
        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            anyhow::bail!("Spotify API error: {} - {}", status, text);
        }

        let currently_playing = response.json::<CurrentlyPlaying>().await?;

        // Only return if it's a track (not episode/ad)
        if currently_playing.currently_playing_type != "track" {
            return Ok(None);
        }

        Ok(Some(currently_playing))
    }

    /// Get playback state (includes device info)
    pub async fn get_playback_state(&self, auth: &SpotifyAuth) -> Result<Option<serde_json::Value>> {
        let token = auth.get_access_token().await?;

        let response = self
            .client
            .get(format!("{}/me/player", SPOTIFY_API_BASE))
            .bearer_auth(&token)
            .send()
            .await?;

        if response.status() == reqwest::StatusCode::NO_CONTENT {
            return Ok(None);
        }

        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            anyhow::bail!("Spotify API error: {} - {}", status, text);
        }

        let state = response.json().await?;
        Ok(Some(state))
    }
}

impl Default for SpotifyApi {
    fn default() -> Self {
        Self::new()
    }
}
