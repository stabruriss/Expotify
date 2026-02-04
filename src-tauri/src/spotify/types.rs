use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Track {
    pub id: String,
    pub name: String,
    pub artists: Vec<Artist>,
    pub album: Album,
    pub duration_ms: u64,
    pub external_urls: ExternalUrls,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Artist {
    pub id: String,
    pub name: String,
    pub external_urls: ExternalUrls,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Album {
    pub id: String,
    pub name: String,
    pub images: Vec<Image>,
    pub release_date: Option<String>,
    pub external_urls: ExternalUrls,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Image {
    pub url: String,
    pub height: Option<u32>,
    pub width: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExternalUrls {
    pub spotify: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CurrentlyPlaying {
    pub is_playing: bool,
    pub progress_ms: Option<u64>,
    pub item: Option<Track>,
    pub currently_playing_type: String,
}

/// Simplified track info for frontend display
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrackInfo {
    pub id: String,
    pub name: String,
    pub artist: String,
    pub album: String,
    pub album_art_url: Option<String>,
    pub duration_ms: u64,
    pub progress_ms: u64,
    pub is_playing: bool,
    pub spotify_url: Option<String>,
    /// AI-generated description about the track
    pub ai_description: Option<String>,
}

impl TrackInfo {
    pub fn from_currently_playing(cp: &CurrentlyPlaying) -> Option<Self> {
        let track = cp.item.as_ref()?;

        Some(Self {
            id: track.id.clone(),
            name: track.name.clone(),
            artist: track
                .artists
                .iter()
                .map(|a| a.name.clone())
                .collect::<Vec<_>>()
                .join(", "),
            album: track.album.name.clone(),
            album_art_url: track
                .album
                .images
                .first()
                .map(|img| img.url.clone()),
            duration_ms: track.duration_ms,
            progress_ms: cp.progress_ms.unwrap_or(0),
            is_playing: cp.is_playing,
            spotify_url: track.external_urls.spotify.clone(),
            ai_description: None,
        })
    }
}
