use crate::ai::OpenAIService;
use crate::auth::OpenAIAuth;
use crate::lyrics::{LyricsFetcher, LyricsInfo};
use crate::spotify::{self, TrackInfo};
use crate::storage::Settings;
use std::sync::Arc;
use tauri::{AppHandle, Manager, State};
use tokio::sync::RwLock;

// ============ Overlay Geometry ============

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
pub struct OverlayGeometry {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

fn geometry_path() -> Result<std::path::PathBuf, String> {
    let config_dir = dirs::config_dir()
        .ok_or_else(|| "Could not find config directory".to_string())?;
    let app_dir = config_dir.join("expotify");
    std::fs::create_dir_all(&app_dir).map_err(|e| e.to_string())?;
    Ok(app_dir.join("overlay_geometry.json"))
}

#[tauri::command]
pub fn save_overlay_geometry(x: f64, y: f64, width: f64, height: f64) -> Result<(), String> {
    let geo = OverlayGeometry { x, y, width, height };
    let path = geometry_path()?;
    let content = serde_json::to_string(&geo).map_err(|e| e.to_string())?;
    std::fs::write(&path, content).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn load_overlay_geometry() -> Result<Option<OverlayGeometry>, String> {
    let path = geometry_path()?;
    if !path.exists() {
        return Ok(None);
    }
    let content = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
    let geo: OverlayGeometry = serde_json::from_str(&content).map_err(|e| e.to_string())?;
    Ok(Some(geo))
}

pub struct AppState {
    pub openai_auth: Arc<OpenAIAuth>,
    pub openai_service: Arc<RwLock<Option<OpenAIService>>>,
    pub settings: Arc<RwLock<Settings>>,
    pub current_track: Arc<RwLock<Option<TrackInfo>>>,
    pub lyrics_fetcher: LyricsFetcher,
}

// ============ Spotify Status ============

#[tauri::command]
pub async fn is_spotify_running() -> Result<bool, String> {
    tokio::task::spawn_blocking(|| spotify::applescript::is_spotify_running())
        .await
        .map_err(|e| e.to_string())
}

// ============ OpenAI Auth Commands ============

#[tauri::command]
pub async fn openai_is_authenticated(state: State<'_, AppState>) -> Result<bool, String> {
    Ok(state.openai_auth.is_authenticated().await)
}

/// Full login flow: generate auth URL, start callback server, wait for redirect, exchange code.
/// Returns the auth URL for the frontend to open in browser.
#[tauri::command]
pub async fn openai_login(state: State<'_, AppState>) -> Result<(), String> {
    let auth_url = state
        .openai_auth
        .get_auth_url()
        .await
        .map_err(|e| e.to_string())?;

    // Open in browser (synchronous — spawns the `open` command and returns immediately)
    open::that(&auth_url).map_err(|e| format!("Failed to open browser: {}", e))?;

    // Wait for OAuth callback on localhost:1455
    state
        .openai_auth
        .wait_for_callback()
        .await
        .map_err(|e| e.to_string())?;

    // Initialize OpenAI service after authentication
    let openai_service = OpenAIService::new(Arc::clone(&state.openai_auth));
    *state.openai_service.write().await = Some(openai_service);

    Ok(())
}

#[tauri::command]
pub async fn openai_logout(state: State<'_, AppState>) -> Result<(), String> {
    state
        .openai_auth
        .logout()
        .await
        .map_err(|e| e.to_string())?;
    *state.openai_service.write().await = None;
    Ok(())
}

// ============ Spotify Playback Commands ============

#[tauri::command]
pub async fn get_current_track(state: State<'_, AppState>) -> Result<Option<TrackInfo>, String> {
    let track_info = tokio::task::spawn_blocking(|| spotify::applescript::get_current_track())
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())?;

    if let Some(ref info) = track_info {
        *state.current_track.write().await = Some(info.clone());
    }

    Ok(track_info)
}

#[tauri::command]
pub async fn get_current_track_with_ai(
    state: State<'_, AppState>,
    force: Option<bool>,
) -> Result<Option<TrackInfo>, String> {
    let track_info = tokio::task::spawn_blocking(|| spotify::applescript::get_current_track())
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())?;

    let Some(mut info) = track_info else {
        return Ok(None);
    };

    // Get AI description if service is available
    let service = state.openai_service.read().await;
    if let Some(ref openai) = *service {
        let settings = state.settings.read().await;
        let model = settings.ai_model.clone();
        let prompt = settings.ai_prompt.clone();
        let web_search = settings.ai_web_search;
        drop(settings);
        match openai.get_track_description(&info, &model, &prompt, web_search, force.unwrap_or(false)).await {
            Ok((description, used_web_search)) => {
                info.ai_description = Some(description);
                info.ai_used_web_search = used_web_search;
            }
            Err(e) => {
                log::warn!("Failed to get AI description: {}", e);
            }
        }
    }

    *state.current_track.write().await = Some(info.clone());
    Ok(Some(info))
}

// ============ Spotify Playback Control ============

#[tauri::command]
pub async fn spotify_play_pause() -> Result<(), String> {
    tokio::task::spawn_blocking(|| spotify::applescript::spotify_play_pause())
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn spotify_next_track() -> Result<(), String> {
    tokio::task::spawn_blocking(|| spotify::applescript::spotify_next_track())
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn spotify_previous_track() -> Result<(), String> {
    tokio::task::spawn_blocking(|| spotify::applescript::spotify_previous_track())
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())
}

// ============ Settings Commands ============

#[tauri::command]
pub async fn get_settings(state: State<'_, AppState>) -> Result<Settings, String> {
    Ok(state.settings.read().await.clone())
}

#[tauri::command]
pub async fn update_settings(
    state: State<'_, AppState>,
    settings: Settings,
) -> Result<(), String> {
    settings.save().map_err(|e| e.to_string())?;
    *state.settings.write().await = settings;
    Ok(())
}

// ============ Auth Status ============

#[derive(serde::Serialize)]
pub struct AuthStatus {
    pub openai: bool,
}

#[tauri::command]
pub async fn get_auth_status(state: State<'_, AppState>) -> Result<AuthStatus, String> {
    Ok(AuthStatus {
        openai: state.openai_auth.is_authenticated().await,
    })
}

// ============ Lyrics Commands ============

#[tauri::command]
pub async fn get_lyrics(
    state: State<'_, AppState>,
    track_id: String,
    track_name: String,
    artist: String,
    album: String,
    duration_ms: u64,
    force: Option<bool>,
) -> Result<LyricsInfo, String> {
    state
        .lyrics_fetcher
        .get_lyrics(&track_id, &track_name, &artist, &album, duration_ms, force.unwrap_or(false))
        .await
        .map_err(|e| e.to_string())
}

// ============ Update Check ============

#[tauri::command]
pub async fn check_for_update() -> Result<crate::updater::UpdateInfo, String> {
    crate::updater::check_for_update()
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn open_url(url: String) -> Result<(), String> {
    open::that(&url).map_err(|e| format!("Failed to open URL: {}", e))
}

// ============ Window Commands ============

#[tauri::command]
pub async fn toggle_overlay(app: AppHandle) -> Result<(), String> {
    if let Some(window) = app.get_webview_window("overlay") {
        if window.is_visible().map_err(|e| e.to_string())? {
            window.hide().map_err(|e| e.to_string())?;
        } else {
            window.show().map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

#[tauri::command]
pub async fn show_main_window(app: AppHandle) -> Result<(), String> {
    if let Some(window) = app.get_webview_window("main") {
        window.show().map_err(|e| e.to_string())?;
        window.set_focus().map_err(|e| e.to_string())?;
    }
    Ok(())
}
