mod auth;
mod ai;
mod commands;
mod lyrics;
mod spotify;
mod storage;

use ai::OpenAIService;
use auth::OpenAIAuth;
use commands::AppState;
use storage::Settings;
use std::sync::Arc;
use tauri::Manager;
use tokio::sync::RwLock;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    env_logger::init();

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            // Token is loaded synchronously from keychain inside new()
            let openai_auth = Arc::new(OpenAIAuth::new());
            let openai_service = if openai_auth.has_stored_token() {
                Arc::new(RwLock::new(Some(OpenAIService::new(Arc::clone(&openai_auth)))))
            } else {
                Arc::new(RwLock::new(None))
            };

            // Load settings
            let settings = Settings::load().unwrap_or_default();

            let state = AppState {
                openai_auth,
                openai_service,
                settings: Arc::new(RwLock::new(settings)),
                current_track: Arc::new(RwLock::new(None)),
                lyrics_fetcher: lyrics::LyricsFetcher::new(),
            };

            app.manage(state);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::is_spotify_running,
            commands::openai_is_authenticated,
            commands::openai_login,
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
