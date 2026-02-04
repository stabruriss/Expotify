use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, RwLock};
use tokio::time::interval;

use super::api::SpotifyApi;
use super::types::TrackInfo;
use crate::auth::SpotifyAuth;

pub enum PollingEvent {
    TrackChanged(TrackInfo),
    PlaybackStateChanged { is_playing: bool },
    Error(String),
}

pub struct SpotifyPoller {
    api: SpotifyApi,
    auth: Arc<SpotifyAuth>,
    poll_interval: Arc<RwLock<Duration>>,
    current_track_id: Arc<RwLock<Option<String>>>,
    is_running: Arc<RwLock<bool>>,
}

impl SpotifyPoller {
    pub fn new(auth: Arc<SpotifyAuth>) -> Self {
        Self {
            api: SpotifyApi::new(),
            auth,
            poll_interval: Arc::new(RwLock::new(Duration::from_secs(3))),
            current_track_id: Arc::new(RwLock::new(None)),
            is_running: Arc::new(RwLock::new(false)),
        }
    }

    /// Set the polling interval
    pub async fn set_poll_interval(&self, seconds: u64) {
        *self.poll_interval.write().await = Duration::from_secs(seconds);
    }

    /// Start polling and return a channel for events
    pub async fn start(&self) -> mpsc::Receiver<PollingEvent> {
        let (tx, rx) = mpsc::channel(32);

        *self.is_running.write().await = true;

        let api = SpotifyApi::new();
        let auth = Arc::clone(&self.auth);
        let poll_interval = Arc::clone(&self.poll_interval);
        let current_track_id = Arc::clone(&self.current_track_id);
        let is_running = Arc::clone(&self.is_running);

        tokio::spawn(async move {
            let mut was_playing = false;

            loop {
                if !*is_running.read().await {
                    break;
                }

                let interval_duration = *poll_interval.read().await;
                let mut ticker = interval(interval_duration);
                ticker.tick().await; // First tick is immediate

                // Check authentication
                if !auth.is_authenticated().await {
                    tokio::time::sleep(Duration::from_secs(1)).await;
                    continue;
                }

                // Poll currently playing
                match api.get_currently_playing(&auth).await {
                    Ok(Some(currently_playing)) => {
                        if let Some(track_info) = TrackInfo::from_currently_playing(&currently_playing) {
                            let mut current_id = current_track_id.write().await;

                            // Check if track changed
                            let track_changed = current_id
                                .as_ref()
                                .map_or(true, |id| id != &track_info.id);

                            if track_changed {
                                *current_id = Some(track_info.id.clone());
                                if tx.send(PollingEvent::TrackChanged(track_info.clone())).await.is_err() {
                                    break;
                                }
                            }

                            // Check if playback state changed
                            if was_playing != track_info.is_playing {
                                was_playing = track_info.is_playing;
                                if tx.send(PollingEvent::PlaybackStateChanged {
                                    is_playing: track_info.is_playing
                                }).await.is_err() {
                                    break;
                                }
                            }
                        }
                    }
                    Ok(None) => {
                        // Nothing playing, clear current track
                        let mut current_id = current_track_id.write().await;
                        if current_id.is_some() {
                            *current_id = None;
                            if was_playing {
                                was_playing = false;
                                let _ = tx.send(PollingEvent::PlaybackStateChanged {
                                    is_playing: false
                                }).await;
                            }
                        }
                    }
                    Err(e) => {
                        let _ = tx.send(PollingEvent::Error(e.to_string())).await;
                    }
                }

                ticker.tick().await;
            }
        });

        rx
    }

    /// Stop polling
    pub async fn stop(&self) {
        *self.is_running.write().await = false;
    }

    /// Check if polling is running
    pub async fn is_running(&self) -> bool {
        *self.is_running.read().await
    }
}
