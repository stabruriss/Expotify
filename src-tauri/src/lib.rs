mod auth;
mod ai;
mod commands;
mod lyrics;
mod spotify;
mod storage;

use auth::{OpenAIAuth, SpotifyAuth};
use commands::AppState;
use spotify::SpotifyApi;
use storage::Settings;
use std::sync::Arc;
use tauri::Manager;
use tokio::sync::RwLock;

// Spotify Client ID - you'll need to create an app at https://developer.spotify.com/dashboard
const SPOTIFY_CLIENT_ID: &str = "YOUR_SPOTIFY_CLIENT_ID";

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    env_logger::init();

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let spotify_auth = Arc::new(SpotifyAuth::new(SPOTIFY_CLIENT_ID.to_string()));
            let openai_auth = Arc::new(OpenAIAuth::new());

            // Load stored tokens
            let spotify_auth_clone = Arc::clone(&spotify_auth);
            let openai_auth_clone = Arc::clone(&openai_auth);

            tauri::async_runtime::spawn(async move {
                if let Err(e) = spotify_auth_clone.load_stored_token().await {
                    log::warn!("Failed to load Spotify token: {}", e);
                }
                if let Err(e) = openai_auth_clone.load_stored_token().await {
                    log::warn!("Failed to load OpenAI token: {}", e);
                }
            });

            // Load settings
            let settings = Settings::load().unwrap_or_default();

            // Initialize OpenAI service if authenticated
            let openai_service = Arc::new(RwLock::new(None));

            let state = AppState {
                spotify_auth,
                openai_auth,
                spotify_api: SpotifyApi::new(),
                openai_service,
                settings: Arc::new(RwLock::new(settings)),
                current_track: Arc::new(RwLock::new(None)),
                lyrics_fetcher: lyrics::LyricsFetcher::new(),
            };

            app.manage(state);

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::spotify_is_authenticated,
            commands::spotify_get_auth_url,
            commands::spotify_exchange_code,
            commands::spotify_logout,
            commands::openai_is_authenticated,
            commands::openai_get_auth_url,
            commands::openai_exchange_code,
            commands::openai_logout,
            commands::get_current_track,
            commands::get_current_track_with_ai,
            commands::get_settings,
            commands::update_settings,
            commands::get_auth_status,
            commands::get_lyrics,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
