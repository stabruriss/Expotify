use crate::auth::{OpenAIAuth, SpotifyAuth};
use crate::spotify::{SpotifyApi, TrackInfo};
use crate::storage::Settings;
use crate::ai::OpenAIService;
use std::sync::Arc;
use tauri::State;
use tokio::sync::RwLock;

pub struct AppState {
    pub spotify_auth: Arc<SpotifyAuth>,
    pub openai_auth: Arc<OpenAIAuth>,
    pub spotify_api: SpotifyApi,
    pub openai_service: Arc<RwLock<Option<OpenAIService>>>,
    pub settings: Arc<RwLock<Settings>>,
    pub current_track: Arc<RwLock<Option<TrackInfo>>>,
}

// ============ Spotify Auth Commands ============

#[tauri::command]
pub async fn spotify_is_authenticated(state: State<'_, AppState>) -> Result<bool, String> {
    Ok(state.spotify_auth.is_authenticated().await)
}

#[tauri::command]
pub async fn spotify_get_auth_url(state: State<'_, AppState>) -> Result<String, String> {
    state
        .spotify_auth
        .get_auth_url()
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn spotify_exchange_code(state: State<'_, AppState>, code: String) -> Result<(), String> {
    state
        .spotify_auth
        .exchange_code(&code)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn spotify_logout(state: State<'_, AppState>) -> Result<(), String> {
    state.spotify_auth.logout().await.map_err(|e| e.to_string())
}

// ============ OpenAI Auth Commands ============

#[tauri::command]
pub async fn openai_is_authenticated(state: State<'_, AppState>) -> Result<bool, String> {
    Ok(state.openai_auth.is_authenticated().await)
}

#[tauri::command]
pub async fn openai_get_auth_url(state: State<'_, AppState>) -> Result<String, String> {
    state
        .openai_auth
        .get_auth_url()
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn openai_exchange_code(
    state: State<'_, AppState>,
    code: String,
    received_state: String,
) -> Result<(), String> {
    // Validate state first
    if !state.openai_auth.validate_state(&received_state).await {
        return Err("Invalid state parameter".to_string());
    }

    state
        .openai_auth
        .exchange_code(&code)
        .await
        .map_err(|e| e.to_string())?;

    // Initialize OpenAI service after authentication
    let openai_service = OpenAIService::new(Arc::clone(&state.openai_auth));
    *state.openai_service.write().await = Some(openai_service);

    Ok(())
}

#[tauri::command]
pub async fn openai_logout(state: State<'_, AppState>) -> Result<(), String> {
    state.openai_auth.logout().await.map_err(|e| e.to_string())?;
    *state.openai_service.write().await = None;
    Ok(())
}

// ============ Spotify Playback Commands ============

#[tauri::command]
pub async fn get_current_track(state: State<'_, AppState>) -> Result<Option<TrackInfo>, String> {
    if !state.spotify_auth.is_authenticated().await {
        return Err("Not authenticated with Spotify".to_string());
    }

    let currently_playing = state
        .spotify_api
        .get_currently_playing(&state.spotify_auth)
        .await
        .map_err(|e| e.to_string())?;

    let track_info = currently_playing.and_then(|cp| TrackInfo::from_currently_playing(&cp));

    if let Some(ref info) = track_info {
        *state.current_track.write().await = Some(info.clone());
    }

    Ok(track_info)
}

#[tauri::command]
pub async fn get_current_track_with_ai(
    state: State<'_, AppState>,
) -> Result<Option<TrackInfo>, String> {
    // First get current track
    let track_info = get_current_track(state.clone()).await?;

    let Some(mut info) = track_info else {
        return Ok(None);
    };

    // Get AI description if service is available
    let service = state.openai_service.read().await;
    if let Some(ref openai) = *service {
        match openai.get_track_description(&info).await {
            Ok(description) => {
                info.ai_description = Some(description);
            }
            Err(e) => {
                log::warn!("Failed to get AI description: {}", e);
            }
        }
    }

    *state.current_track.write().await = Some(info.clone());
    Ok(Some(info))
}

// ============ Settings Commands ============

#[tauri::command]
pub async fn get_settings(state: State<'_, AppState>) -> Result<Settings, String> {
    Ok(state.settings.read().await.clone())
}

#[tauri::command]
pub async fn update_settings(state: State<'_, AppState>, settings: Settings) -> Result<(), String> {
    settings.save().map_err(|e| e.to_string())?;
    *state.settings.write().await = settings;
    Ok(())
}

// ============ Auth Status ============

#[derive(serde::Serialize)]
pub struct AuthStatus {
    pub spotify: bool,
    pub openai: bool,
}

#[tauri::command]
pub async fn get_auth_status(state: State<'_, AppState>) -> Result<AuthStatus, String> {
    Ok(AuthStatus {
        spotify: state.spotify_auth.is_authenticated().await,
        openai: state.openai_auth.is_authenticated().await,
    })
}
