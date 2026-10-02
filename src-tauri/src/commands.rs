use crate::ai::events::{self, EventContext};
use crate::ai::models::{ModelProvider, ModelSelection, ProviderCatalog};
use crate::ai::tools::{
    self, ChatCancellation, ToolCall, ToolContext, ToolOutcome, ToolProtocol, ToolRunner,
};
use crate::ai::{AgentResponse, AnthropicService, ChatMessage, NativeChatOutcome, OpenAIService};
use crate::auth::{AnthropicAuth, OpenAIAuth, SpotifyAuth};
use crate::lyrics::{LyricsFetcher, LyricsInfo};
use crate::spotify::{self, SearchResult, SpotifyDevice, SpotifyWebApi, TrackInfo};
use crate::storage::Settings;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Instant;
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
    let config_dir =
        dirs::config_dir().ok_or_else(|| "Could not find config directory".to_string())?;
    let app_dir = config_dir.join("expotify");
    std::fs::create_dir_all(&app_dir).map_err(|e| e.to_string())?;
    Ok(app_dir.join("overlay_geometry.json"))
}

#[tauri::command]
pub fn save_overlay_geometry(x: f64, y: f64, width: f64, height: f64) -> Result<(), String> {
    let geo = OverlayGeometry {
        x,
        y,
        width,
        height,
    };
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
    pub anthropic_auth: Arc<AnthropicAuth>,
    pub anthropic_service: Arc<RwLock<Option<AnthropicService>>>,
    pub spotify_auth: Arc<SpotifyAuth>,
    pub spotify_webapi: Arc<RwLock<Option<SpotifyWebApi>>>,
    pub settings: Arc<RwLock<Settings>>,
    pub current_track: Arc<RwLock<Option<TrackInfo>>>,
    pub lyrics_fetcher: LyricsFetcher,
    /// The in-flight chat request (client request id + cancellation handle); one at a time.
    pub chat_cancel: tokio::sync::Mutex<Option<(String, ChatCancellation)>>,
}

async fn resolve_connected_model(
    state: &AppState,
    selection: &ModelSelection,
) -> Result<String, String> {
    let disconnected = || {
        format!(
            "{} is not connected. Select a connected model in Settings.",
            selection.provider().label()
        )
    };
    match selection.provider() {
        ModelProvider::Openai => state
            .openai_service
            .read()
            .await
            .as_ref()
            .ok_or_else(disconnected)?
            .resolve_model(selection)
            .await
            .map_err(|e| e.to_string()),
        ModelProvider::Anthropic => {
            if !state.anthropic_auth.is_authenticated().await {
                return Err(disconnected());
            }
            state
                .anthropic_service
                .read()
                .await
                .as_ref()
                .ok_or_else(disconnected)?
                .resolve_model(selection)
                .await
                .map_err(|e| e.to_string())
        }
    }
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
    let mut settings = state.settings.write().await;
    let mut updated = settings.clone();
    updated.initialize_model_defaults(ModelProvider::Openai);
    updated.save().map_err(|e| e.to_string())?;
    *settings = updated;
    drop(settings);
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

    // Route only to the selected provider; never silently change providers.
    let settings = state.settings.read().await;
    let preferred_model = settings.ai_model.clone();
    let prompt = settings.ai_prompt.clone();
    let web_search = settings.ai_web_search;
    let memories = settings.memories.clone();
    drop(settings);

    let model = match resolve_connected_model(state.inner(), &preferred_model).await {
        Ok(model) => model,
        Err(error) => {
            info.ai_error = Some(error);
            *state.current_track.write().await = Some(info.clone());
            return Ok(Some(info));
        }
    };

    let ai_result = if preferred_model.provider() == ModelProvider::Anthropic {
        let service = state.anthropic_service.read().await;
        if let Some(ref anthropic) = *service {
            Some(
                anthropic
                    .get_track_description(
                        &info,
                        &model,
                        &prompt,
                        web_search,
                        force.unwrap_or(false),
                        &memories,
                    )
                    .await,
            )
        } else {
            None
        }
    } else {
        let service = state.openai_service.read().await;
        if let Some(ref openai) = *service {
            Some(
                openai
                    .get_track_description(
                        &info,
                        &model,
                        &prompt,
                        web_search,
                        force.unwrap_or(false),
                        &memories,
                    )
                    .await,
            )
        } else {
            None
        }
    };

    if let Some(result) = ai_result {
        match result {
            Ok((description, used_web_search)) => {
                info.ai_description = Some(description);
                info.ai_error = None;
                info.ai_used_web_search = used_web_search;
            }
            Err(e) => {
                log::warn!("Failed to get AI description: {}", e);
                info.ai_error = Some(e.to_string());
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

#[tauri::command]
pub async fn spotify_pause() -> Result<(), String> {
    tokio::task::spawn_blocking(|| spotify::applescript::spotify_pause())
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn spotify_play() -> Result<(), String> {
    tokio::task::spawn_blocking(|| spotify::applescript::spotify_play())
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())
}

// ============ TTS Commands ============

#[tauri::command]
pub async fn tts_synthesize(text: String) -> Result<String, String> {
    use base64::Engine;
    let audio_bytes = crate::tts::synthesize(&text)
        .await
        .map_err(|e| format!("{e:#}"))?;
    Ok(base64::engine::general_purpose::STANDARD.encode(&audio_bytes))
}

#[tauri::command]
pub async fn tts_check_available() -> Result<(), String> {
    crate::tts::check_available()
        .await
        .map_err(|e| format!("{e:#}"))
}

// ============ Settings Commands ============

#[tauri::command]
pub async fn get_settings(state: State<'_, AppState>) -> Result<Settings, String> {
    Ok(state.settings.read().await.clone())
}

#[tauri::command]
pub async fn update_settings(
    state: State<'_, AppState>,
    mut settings: Settings,
) -> Result<(), String> {
    settings.ai_model.validate().map_err(|e| e.to_string())?;
    settings.chat_model.validate().map_err(|e| e.to_string())?;
    // Preserve anthropic_enabled — only Claude OAuth connect/disconnect should change it
    let mut current = state.settings.write().await;
    settings.anthropic_enabled = current.anthropic_enabled;
    settings.model_defaults_initialized = current.model_defaults_initialized
        || settings.ai_model != current.ai_model
        || settings.chat_model != current.chat_model;
    settings.save().map_err(|e| e.to_string())?;
    *current = settings;
    Ok(())
}

// ============ Auth Status ============

#[derive(serde::Serialize)]
pub struct AuthStatus {
    pub openai: bool,
    /// Claude OAuth is connected and ready to use
    pub anthropic: bool,
    /// Claude OAuth is supported in this build
    pub anthropic_available: bool,
    pub spotify: bool,
}

#[tauri::command]
pub async fn get_auth_status(state: State<'_, AppState>) -> Result<AuthStatus, String> {
    let service_active = state.anthropic_service.read().await.is_some();
    let authenticated = service_active && state.anthropic_auth.is_authenticated().await;
    Ok(AuthStatus {
        openai: state.openai_auth.is_authenticated().await,
        anthropic: authenticated && service_active,
        anthropic_available: true,
        spotify: state.spotify_auth.is_authenticated().await,
    })
}

// ============ Anthropic OAuth ============

#[tauri::command]
pub async fn anthropic_start_oauth(state: State<'_, AppState>) -> Result<(), String> {
    state
        .anthropic_auth
        .login()
        .await
        .map_err(|e| e.to_string())?;
    let service = AnthropicService::new(Arc::clone(&state.anthropic_auth));
    let mut settings = state.settings.write().await;
    let mut updated = settings.clone();
    updated.anthropic_enabled = true;
    updated.initialize_model_defaults(ModelProvider::Anthropic);
    updated.save().map_err(|e| e.to_string())?;
    *settings = updated;
    drop(settings);
    *state.anthropic_service.write().await = Some(service);
    Ok(())
}

#[tauri::command]
pub async fn anthropic_cancel_oauth(state: State<'_, AppState>) -> Result<(), String> {
    state.anthropic_auth.clear_pending_oauth().await;
    Ok(())
}

#[tauri::command]
pub async fn anthropic_logout(state: State<'_, AppState>) -> Result<(), String> {
    state
        .anthropic_auth
        .logout()
        .await
        .map_err(|e| e.to_string())?;
    *state.anthropic_service.write().await = None;
    let mut settings = state.settings.write().await;
    settings.anthropic_enabled = false;
    settings.save().map_err(|e| e.to_string())?;
    Ok(())
}

// ============ Spotify Web API Auth Commands ============

#[tauri::command]
pub async fn spotify_is_authenticated(state: State<'_, AppState>) -> Result<bool, String> {
    Ok(state.spotify_auth.is_authenticated().await)
}

#[tauri::command]
pub async fn spotify_connect(state: State<'_, AppState>, sp_dc: String) -> Result<(), String> {
    state
        .spotify_auth
        .set_sp_dc(&sp_dc)
        .await
        .map_err(|e| e.to_string())?;
    let webapi = SpotifyWebApi::new(Arc::clone(&state.spotify_auth));
    *state.spotify_webapi.write().await = Some(webapi);
    Ok(())
}

#[tauri::command]
pub async fn spotify_login(app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    // Close any existing login window
    if let Some(existing) = app.get_webview_window("spotify-login") {
        let _ = existing.close();
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    }

    let sp_dc_result: Arc<std::sync::Mutex<Option<String>>> = Arc::new(std::sync::Mutex::new(None));

    let window = tauri::WebviewWindowBuilder::new(
        &app,
        "spotify-login",
        tauri::WebviewUrl::External("https://accounts.spotify.com/login".parse().unwrap()),
    )
    .title("Connect Spotify")
    .inner_size(420.0, 700.0)
    .center()
    .min_inner_size(350.0, 500.0)
    .build()
    .map_err(|e| format!("Failed to create login window: {}", e))?;

    log::info!("[spotify_login] Webview window opened, starting cookie poll...");

    // Poll for up to 5 minutes (150 iterations × 2s)
    // Each iteration: check previous extraction result, then trigger new extraction
    for iteration in 0..150 {
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;

        // Check if we captured the sp_dc from a previous extraction cycle
        let captured = sp_dc_result.lock().unwrap().take();
        if let Some(sp_dc) = captured {
            log::info!(
                "[spotify_login] sp_dc captured (len={}), validating...",
                sp_dc.len()
            );
            state.spotify_auth.set_sp_dc(&sp_dc).await.map_err(|e| {
                log::error!("[spotify_login] set_sp_dc failed: {}", e);
                e.to_string()
            })?;
            let webapi = SpotifyWebApi::new(Arc::clone(&state.spotify_auth));
            *state.spotify_webapi.write().await = Some(webapi);

            log::info!("[spotify_login] Spotify connected successfully!");
            let _ = window.close();
            return Ok(());
        }

        // Check if the window was closed by the user (cancellation)
        if app.get_webview_window("spotify-login").is_none() {
            log::info!("[spotify_login] Login window closed by user");
            return Err("Login cancelled".to_string());
        }

        // Trigger native cookie extraction from WKWebView cookie store
        let sp_dc_for_extraction = sp_dc_result.clone();
        if let Err(e) = window.with_webview(move |platform_webview| {
            extract_sp_dc_cookie(platform_webview, sp_dc_for_extraction);
        }) {
            log::warn!(
                "[spotify_login] with_webview failed (iteration {}): {}",
                iteration,
                e
            );
        }
    }

    let _ = window.close();
    log::warn!("[spotify_login] Timed out after 5 minutes");
    Err("Login timed out. Please try again.".to_string())
}

/// Extract sp_dc cookie from WKWebView's native cookie store (macOS).
/// Called on the main thread via `with_webview`. Result is stored in the Arc.
#[cfg(target_os = "macos")]
fn extract_sp_dc_cookie(
    platform_webview: tauri::webview::PlatformWebview,
    result: Arc<std::sync::Mutex<Option<String>>>,
) {
    use core::ptr::NonNull;
    use objc2_foundation::{NSArray, NSHTTPCookie};
    use objc2_web_kit::WKWebView;

    unsafe {
        let ptr = platform_webview.inner();
        let wkwebview = &*(ptr as *const WKWebView);
        let config = wkwebview.configuration();
        let data_store = config.websiteDataStore();
        let cookie_store = data_store.httpCookieStore();

        let block = block2::RcBlock::new(move |cookies: NonNull<NSArray<NSHTTPCookie>>| {
            let cookies = unsafe { cookies.as_ref() };
            let count = cookies.count();
            log::info!("[extract_sp_dc] getAllCookies returned {} cookies", count);
            let mut found = false;
            for i in 0..count {
                let cookie = unsafe { cookies.objectAtIndex(i) };
                let name = cookie.name().to_string();
                // Log spotify-related cookies for debugging
                if name.starts_with("sp_") {
                    log::info!(
                        "[extract_sp_dc] Found cookie: {} (len={})",
                        name,
                        cookie.value().to_string().len()
                    );
                }
                if name == "sp_dc" {
                    let value = cookie.value().to_string();
                    if value.len() > 20 {
                        log::info!(
                            "[extract_sp_dc] sp_dc cookie captured! (len={})",
                            value.len()
                        );
                        *result.lock().unwrap() = Some(value);
                        found = true;
                    } else {
                        log::warn!(
                            "[extract_sp_dc] sp_dc too short (len={}), skipping",
                            value.len()
                        );
                    }
                    break;
                }
            }
            if !found {
                log::info!("[extract_sp_dc] sp_dc not found among {} cookies", count);
            }
        });

        cookie_store.getAllCookies(&block);
    }
}

#[cfg(not(target_os = "macos"))]
fn extract_sp_dc_cookie(
    _platform_webview: tauri::webview::PlatformWebview,
    _result: Arc<std::sync::Mutex<Option<String>>>,
) {
    // Cookie extraction only supported on macOS
}

#[tauri::command]
pub async fn spotify_disconnect(state: State<'_, AppState>) -> Result<(), String> {
    state
        .spotify_auth
        .remove_sp_dc()
        .await
        .map_err(|e| e.to_string())?;
    *state.spotify_webapi.write().await = None;
    Ok(())
}

// ============ Spotify Web API Feature Commands ============

#[tauri::command]
pub async fn spotify_search(
    state: State<'_, AppState>,
    query: String,
    limit: Option<u32>,
) -> Result<Vec<SearchResult>, String> {
    let webapi = state.spotify_webapi.read().await;
    let webapi = webapi
        .as_ref()
        .ok_or("Spotify not connected. Please add your sp_dc cookie in Settings.")?;
    webapi
        .search_tracks(&query, limit.unwrap_or(5))
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn spotify_is_track_liked(
    state: State<'_, AppState>,
    track_id: String,
) -> Result<bool, String> {
    log::info!("[cmd] spotify_is_track_liked: {}", track_id);
    let webapi = state.spotify_webapi.read().await;
    let webapi = webapi.as_ref().ok_or_else(|| {
        log::error!("[cmd] spotify_is_track_liked: webapi is None");
        "Spotify not connected".to_string()
    })?;
    webapi.is_track_liked(&track_id).await.map_err(|e| {
        log::error!("[cmd] spotify_is_track_liked error: {}", e);
        e.to_string()
    })
}

#[tauri::command]
pub async fn spotify_like_track(
    state: State<'_, AppState>,
    track_id: String,
) -> Result<(), String> {
    log::info!("[cmd] spotify_like_track: {}", track_id);
    let webapi = state.spotify_webapi.read().await;
    let webapi = webapi.as_ref().ok_or_else(|| {
        log::error!("[cmd] spotify_like_track: webapi is None");
        "Spotify not connected".to_string()
    })?;
    webapi.like_track(&track_id).await.map_err(|e| {
        log::error!("[cmd] spotify_like_track error: {}", e);
        e.to_string()
    })
}

#[tauri::command]
pub async fn spotify_unlike_track(
    state: State<'_, AppState>,
    track_id: String,
) -> Result<(), String> {
    log::info!("[cmd] spotify_unlike_track: {}", track_id);
    let webapi = state.spotify_webapi.read().await;
    let webapi = webapi.as_ref().ok_or_else(|| {
        log::error!("[cmd] spotify_unlike_track: webapi is None");
        "Spotify not connected".to_string()
    })?;
    webapi.unlike_track(&track_id).await.map_err(|e| {
        log::error!("[cmd] spotify_unlike_track error: {}", e);
        e.to_string()
    })
}

#[tauri::command]
pub async fn spotify_shuffle_liked() -> Result<(), String> {
    log::info!("[cmd] spotify_shuffle_liked");
    tokio::task::spawn_blocking(spotify::applescript::spotify_shuffle_collection)
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub async fn spotify_get_devices(state: State<'_, AppState>) -> Result<Vec<SpotifyDevice>, String> {
    log::info!("[cmd] spotify_get_devices");
    let webapi = state.spotify_webapi.read().await;
    let webapi = webapi.as_ref().ok_or_else(|| {
        log::error!("[cmd] spotify_get_devices: webapi is None");
        "Spotify not connected".to_string()
    })?;
    webapi.get_devices().await.map_err(|e| {
        log::error!("[cmd] spotify_get_devices error: {}", e);
        e.to_string()
    })
}

#[tauri::command]
pub async fn spotify_transfer_playback(
    state: State<'_, AppState>,
    device_id: String,
) -> Result<(), String> {
    log::info!("[cmd] spotify_transfer_playback: {}", device_id);
    let webapi = state.spotify_webapi.read().await;
    let webapi = webapi.as_ref().ok_or_else(|| {
        log::error!("[cmd] spotify_transfer_playback: webapi is None");
        "Spotify not connected".to_string()
    })?;
    webapi.transfer_playback(&device_id).await.map_err(|e| {
        log::error!("[cmd] spotify_transfer_playback error: {}", e);
        e.to_string()
    })
}

// ============ Spotify Volume (AppleScript) ============

#[tauri::command]
pub async fn spotify_get_volume() -> Result<u32, String> {
    tokio::task::spawn_blocking(spotify::applescript::get_spotify_volume)
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn spotify_set_volume(volume: u32) -> Result<(), String> {
    tokio::task::spawn_blocking(move || spotify::applescript::set_spotify_volume(volume))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())
}

// ============ Spotify Play Track by URI ============

#[tauri::command]
pub async fn spotify_play_track(uri: String) -> Result<(), String> {
    // Play via AppleScript with window hiding
    tokio::task::spawn_blocking(move || spotify::applescript::spotify_play_track(&uri))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())
}

// ============ Agent Chat ============

#[derive(serde::Serialize)]
pub struct AgentChatResult {
    pub response: AgentResponse,
    /// True only when the executor completed the requested action.
    pub executed: bool,
    pub track_name: Option<String>,
    /// Error message when action execution fails
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Structured outcome of every tool call made for this request, in order.
    #[serde(default)]
    pub tool_results: Vec<ToolOutcome>,
}

/// One chat request. `request_id` is chosen by the client and is what `agent_chat_cancel`
/// targets, so a late cancel for an earlier request can never abort a newer one.
#[tauri::command]
pub async fn agent_chat(
    state: State<'_, AppState>,
    request_id: String,
    messages: Vec<ChatMessage>,
) -> Result<AgentChatResult, String> {
    let cancellation = ChatCancellation::new();
    {
        let mut slot = state.chat_cancel.lock().await;
        if let Some((_, previous)) = slot.take() {
            previous.cancel();
        }
        *slot = Some((request_id.clone(), cancellation.clone()));
    }
    // Cancellation is cooperative: the provider wait is aborted, a tool that is already
    // running finishes, and the request returns with everything the executor actually did.
    let result = run_agent_chat(state.inner(), messages, &cancellation).await;
    let mut slot = state.chat_cancel.lock().await;
    if slot.as_ref().is_some_and(|(id, _)| *id == request_id) {
        *slot = None;
    }
    result
}

/// Cancel the chat request with this id: aborts the provider call (and the Claude helper
/// process) and prevents further tool execution. Actions already performed stay done. A
/// cancel for a request that already finished or was replaced is ignored.
#[tauri::command]
pub async fn agent_chat_cancel(
    state: State<'_, AppState>,
    request_id: String,
) -> Result<(), String> {
    let mut slot = state.chat_cancel.lock().await;
    if slot.as_ref().is_some_and(|(id, _)| *id == request_id) {
        if let Some((_, cancellation)) = slot.take() {
            cancellation.cancel();
        }
    }
    Ok(())
}

async fn run_agent_chat(
    state: &AppState,
    messages: Vec<ChatMessage>,
    cancellation: &ChatCancellation,
) -> Result<AgentChatResult, String> {
    let started = Instant::now();
    let settings = state.settings.read().await;
    let preferred_model = settings.chat_model.clone();
    let chat_prompt = settings.chat_prompt.clone();
    let web_search = settings.ai_web_search;
    let memories = settings.memories.clone();
    let protocols = settings.tool_protocol;
    drop(settings);

    let model = resolve_connected_model(state, &preferred_model).await?;

    // Get current track info for context
    let current = state.current_track.read().await;
    let (track_name, artist, album) = match current.as_ref() {
        Some(t) => (t.name.clone(), t.artist.clone(), t.album.clone()),
        None => (
            "(nothing playing)".to_string(),
            "".to_string(),
            "".to_string(),
        ),
    };
    let track_id = current.as_ref().map(|t| t.id.clone());
    drop(current);

    // Get current volume
    let volume = tokio::task::spawn_blocking(|| spotify::applescript::get_spotify_volume())
        .await
        .map_err(|e| e.to_string())?
        .unwrap_or(50);

    let provider = preferred_model.provider();
    let (provider_name, protocol) = match provider {
        ModelProvider::Anthropic => ("anthropic", protocols.anthropic),
        ModelProvider::Openai => ("openai", protocols.openai),
    };
    let events = EventContext::new(provider_name, &model, protocol.label());
    let ctx = ToolContext::new(&state.spotify_webapi, &state.settings, track_id.clone());
    let mut runner = ToolRunner::new(cancellation.flag.clone()).with_events(events.clone());

    let completed: Result<(u32, AgentResponse), String> = match (provider, protocol) {
        (ModelProvider::Anthropic, ToolProtocol::Native) => {
            let service = state.anthropic_service.read().await;
            let anthropic = service
                .as_ref()
                .ok_or("Claude not connected. Please sign in first.")?;
            anthropic
                .agent_chat_native(
                    &messages,
                    &model,
                    &chat_prompt,
                    &track_name,
                    &artist,
                    &album,
                    volume,
                    &memories,
                    &ctx,
                    &mut runner,
                    cancellation,
                )
                .await
                .map_err(|e| e.to_string())
                .map(|outcome| {
                    (
                        outcome.turns.unwrap_or(1),
                        native_response(outcome, &runner),
                    )
                })
        }
        (ModelProvider::Openai, ToolProtocol::Native) => {
            let service = state.openai_service.read().await;
            let openai = service
                .as_ref()
                .ok_or("ChatGPT not connected. Please connect in Settings.")?;
            openai
                .agent_chat_native(
                    &messages,
                    &model,
                    &chat_prompt,
                    &track_name,
                    &artist,
                    &album,
                    volume,
                    web_search,
                    &memories,
                    &ctx,
                    &mut runner,
                    cancellation,
                )
                .await
                .map_err(|e| e.to_string())
                .map(|outcome| {
                    (
                        outcome.turns.unwrap_or(1),
                        native_response(outcome, &runner),
                    )
                })
        }
        (_, ToolProtocol::Legacy) => {
            let response = tokio::select! {
                response = legacy_chat(
                    state, provider, &messages, &model, &chat_prompt, &track_name, &artist, &album,
                    volume, web_search, &memories,
                ) => response,
                _ = cancellation.cancelled() => Err(tools::CANCELLED.to_string()),
            };
            match response {
                Ok(response) => {
                    let mut parse_event = events.event("parse");
                    parse_event.parse_via = response.parse_via;
                    parse_event.tool = ToolCall::from_legacy(&response, 0).map(|call| call.name);
                    events::record(parse_event);
                    // Legacy text protocol yields at most one action per reply; every branch of the
                    // executor reports an explicit outcome instead of failing silently.
                    if let Some(call) = ToolCall::from_legacy(&response, 1) {
                        runner.run(&ctx, &call).await;
                    }
                    Ok((1, response))
                }
                Err(error) => Err(error),
            }
        }
    };

    // Whatever happened to the model request, what the executor did is reported and logged
    // (each `exec` event was already written by the runner when the call finished).
    let tool_results = runner.outcomes();
    let cancelled = cancellation.is_cancelled();
    let mut turn_event = events.event("turn");
    turn_event.ok = Some(completed.is_ok() && tool_results.iter().all(|outcome| outcome.ok));
    turn_event.error_code = if cancelled {
        Some("cancelled".to_string())
    } else if completed.is_err() {
        Some("provider_error".to_string())
    } else if tool_results.iter().any(|outcome| !outcome.ok) {
        Some("tool_failed".to_string())
    } else {
        None
    };
    turn_event.turns = completed.as_ref().ok().map(|(turns, _)| *turns);
    turn_event.duration_ms = Some(started.elapsed().as_millis() as u64);
    events::record(turn_event);

    match completed {
        Ok((_, response)) => Ok(finish(response, tool_results, None)),
        Err(error) => {
            let error = if cancelled {
                tools::CANCELLED.to_string()
            } else {
                error
            };
            if tool_results.is_empty() {
                return Err(error);
            }
            // The model never summarised, but actions ran: hand the UI the outcomes together
            // with the request-level error instead of hiding them behind a plain failure.
            let response = AgentResponse {
                action: tool_results
                    .last()
                    .map(|outcome| outcome.name.clone())
                    .unwrap_or_else(|| "reply".to_string()),
                message: String::new(),
                args: serde_json::Value::Null,
                parse_via: None,
            };
            Ok(finish(response, tool_results, Some(error)))
        }
    }
}

/// The legacy text-protocol provider call; the caller races it against cancellation.
#[allow(clippy::too_many_arguments)]
async fn legacy_chat(
    state: &AppState,
    provider: ModelProvider,
    messages: &[ChatMessage],
    model: &str,
    chat_prompt: &str,
    track_name: &str,
    artist: &str,
    album: &str,
    volume: u32,
    web_search: bool,
    memories: &[String],
) -> Result<AgentResponse, String> {
    if provider == ModelProvider::Anthropic {
        let service = state.anthropic_service.read().await;
        let anthropic = service
            .as_ref()
            .ok_or("Claude not connected. Please sign in first.")?;
        anthropic
            .agent_chat(
                messages,
                model,
                chat_prompt,
                track_name,
                artist,
                album,
                volume,
                web_search,
                memories,
            )
            .await
            .map_err(|e| e.to_string())
    } else {
        let service = state.openai_service.read().await;
        let openai = service
            .as_ref()
            .ok_or("ChatGPT not connected. Please connect in Settings.")?;
        openai
            .agent_chat(
                messages,
                model,
                chat_prompt,
                track_name,
                artist,
                album,
                volume,
                web_search,
                memories,
            )
            .await
            .map_err(|e| e.to_string())
    }
}

/// Aggregate the executor's outcomes for the UI. `executed` is true only when at least one
/// tool ran and every tool call succeeded; `track_name` is the track now playing because of
/// this request; `error` is a request-level failure (provider error or cancellation), never
/// a single tool's failure, which stays in `tool_results`.
fn finish(
    response: AgentResponse,
    tool_results: Vec<ToolOutcome>,
    error: Option<String>,
) -> AgentChatResult {
    let executed = !tool_results.is_empty() && tool_results.iter().all(|outcome| outcome.ok);
    let track_name = tool_results
        .iter()
        .rev()
        .filter(|outcome| outcome.ok)
        .find_map(|outcome| outcome.track_name.clone());
    AgentChatResult {
        response,
        executed,
        track_name,
        error,
        tool_results,
    }
}

/// Shape a native-protocol turn for the UI: the final text (or the last executor output
/// when the model said nothing) and the last tool name as the action badge.
fn native_response(outcome: NativeChatOutcome, runner: &ToolRunner) -> AgentResponse {
    let outcomes = runner.outcomes();
    let message = if outcome.text.is_empty() {
        outcomes
            .last()
            .map(|result| result.output.clone())
            .unwrap_or_default()
    } else {
        outcome.text
    };
    let action = outcomes
        .last()
        .map(|result| result.name.clone())
        .unwrap_or_else(|| "reply".to_string());
    AgentResponse {
        action,
        message,
        args: serde_json::Value::Null,
        parse_via: None,
    }
}

/// Debug-only: exercise one provider's native tool-calling path end to end with a dry-run
/// executor (calls are validated and reported, never executed). Verifies endpoint
/// compatibility before the native protocol is switched on for that provider.
#[tauri::command]
pub async fn tool_protocol_probe(
    state: State<'_, AppState>,
    provider: String,
) -> Result<serde_json::Value, String> {
    if !cfg!(debug_assertions) {
        return Err("The tool protocol probe is only available in debug builds.".to_string());
    }
    let provider = match provider.as_str() {
        "openai" => ModelProvider::Openai,
        "anthropic" => ModelProvider::Anthropic,
        other => return Err(format!("Unknown provider '{other}'")),
    };
    run_tool_probe(state.inner(), provider).await
}

pub async fn run_tool_probe(
    state: &AppState,
    provider: ModelProvider,
) -> Result<serde_json::Value, String> {
    let started = Instant::now();
    let provider_name = match provider {
        ModelProvider::Anthropic => "anthropic",
        ModelProvider::Openai => "openai",
    };
    // Every report, including one for a failure before the model was reached, carries the
    // run metadata so a poller can tell a real failure from a report that is not there yet.
    let mut report = serde_json::json!({
        "run_id": events::new_request_id(),
        "started_at": chrono::Utc::now().to_rfc3339(),
        "provider": provider_name,
        "dry_run": true,
        "pass_criteria": PROBE_PASS_CRITERIA,
        "tool_calls": [],
    });
    match probe_provider(state, provider).await {
        Ok(run) => {
            report["model"] = serde_json::json!(run.model);
            report["tool_calls"] = serde_json::json!(run.tool_calls);
            match run.outcome {
                Ok(outcome) => {
                    report["ok"] = serde_json::json!(true);
                    report["turns"] = serde_json::json!(outcome.turns);
                    report["tool_uses"] = serde_json::json!(outcome.tool_uses);
                    report["text"] = serde_json::json!(outcome.text);
                }
                Err(error) => {
                    report["ok"] = serde_json::json!(false);
                    report["error"] = serde_json::json!(error.to_string());
                }
            }
        }
        Err(error) => {
            report["ok"] = serde_json::json!(false);
            report["error"] = serde_json::json!(error);
        }
    }
    report["finished_at"] = serde_json::json!(chrono::Utc::now().to_rfc3339());
    report["duration_ms"] = serde_json::json!(started.elapsed().as_millis() as u64);
    report["pass"] = serde_json::json!(probe_passed(&report));
    Ok(report)
}

struct ProbeRun {
    model: String,
    tool_calls: Vec<serde_json::Value>,
    outcome: anyhow::Result<NativeChatOutcome>,
}

async fn probe_provider(state: &AppState, provider: ModelProvider) -> Result<ProbeRun, String> {
    let chat_prompt = state.settings.read().await.chat_prompt.clone();
    let selection = ModelSelection::Default { provider };
    let model = resolve_connected_model(state, &selection).await?;
    let ctx = ToolContext::new(&state.spotify_webapi, &state.settings, None);
    let mut runner = ToolRunner::dry_run(Arc::new(AtomicBool::new(false)));
    let cancellation = ChatCancellation::new();
    let messages = vec![ChatMessage {
        role: "user".to_string(),
        content: "Probe: set the playback volume to 42 using the set_volume tool, then confirm in one short sentence.".to_string(),
        tool_results: vec![],
    }];
    let outcome = match provider {
        ModelProvider::Anthropic => {
            let service = state.anthropic_service.read().await;
            let anthropic = service
                .as_ref()
                .ok_or("Claude not connected. Please sign in first.")?;
            anthropic
                .agent_chat_native(
                    &messages,
                    &model,
                    &chat_prompt,
                    "Probe Track",
                    "Probe Artist",
                    "Probe Album",
                    60,
                    &[],
                    &ctx,
                    &mut runner,
                    &cancellation,
                )
                .await
        }
        ModelProvider::Openai => {
            let service = state.openai_service.read().await;
            let openai = service
                .as_ref()
                .ok_or("ChatGPT not connected. Please connect in Settings.")?;
            openai
                .agent_chat_native(
                    &messages,
                    &model,
                    &chat_prompt,
                    "Probe Track",
                    "Probe Artist",
                    "Probe Album",
                    60,
                    false,
                    &[],
                    &ctx,
                    &mut runner,
                    &cancellation,
                )
                .await
        }
    };
    // Validated arguments are reported here only (the event log never records arguments).
    let tool_calls = runner
        .outcomes()
        .iter()
        .map(|outcome| {
            let args = runner
                .dry_run_calls()
                .iter()
                .find(|call| call.call_id == outcome.call_id)
                .map(|call| call.args.clone())
                .unwrap_or(serde_json::Value::Null);
            serde_json::json!({
                "call_id": outcome.call_id,
                "name": outcome.name,
                "ok": outcome.ok,
                "args": args,
                "output": outcome.output,
            })
        })
        .collect();
    Ok(ProbeRun {
        model,
        tool_calls,
        outcome,
    })
}

const PROBE_PASS_CRITERIA: &str =
    "ok, exactly one tool call, it is set_volume with ok=true and numeric level 42, non-empty final text";

fn probe_passed(report: &serde_json::Value) -> bool {
    let calls = report["tool_calls"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    report["ok"] == true
        && calls.len() == 1
        && calls[0]["name"] == "set_volume"
        && calls[0]["ok"] == true
        && calls[0]["args"]["level"].as_f64() == Some(42.0)
        && report["text"]
            .as_str()
            .is_some_and(|text| !text.trim().is_empty())
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
        .get_lyrics(
            &track_id,
            &track_name,
            &artist,
            &album,
            duration_ms,
            force.unwrap_or(false),
        )
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

// ============ Model Listing ============

#[tauri::command]
pub async fn list_models(
    state: State<'_, AppState>,
    force: Option<bool>,
) -> Result<Vec<ProviderCatalog>, String> {
    let force = force.unwrap_or(false);
    let openai = async {
        let service = state.openai_service.read().await;
        match service.as_ref() {
            Some(service) => Some(service.list_models(force).await),
            None => None,
        }
    };
    let anthropic = async {
        let service = state.anthropic_service.read().await;
        if service.is_none() || !state.anthropic_auth.is_authenticated().await {
            return None;
        }
        match service.as_ref() {
            Some(service) => Some(service.list_models(force).await),
            None => None,
        }
    };
    let (openai, anthropic) = tokio::join!(openai, anthropic);
    Ok([openai, anthropic].into_iter().flatten().collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn outcome(name: &str, ok: bool, track: Option<&str>) -> ToolOutcome {
        ToolOutcome {
            call_id: format!("{name}-id"),
            name: name.into(),
            ok,
            output: if ok { "done".into() } else { "failed".into() },
            error_code: (!ok).then(|| "test_failure".to_string()),
            track_name: track.map(str::to_string),
        }
    }

    fn reply() -> AgentResponse {
        AgentResponse {
            action: "reply".into(),
            message: "ok".into(),
            args: serde_json::Value::Null,
            parse_via: None,
        }
    }

    #[test]
    fn multi_action_results_are_aggregated_from_every_outcome() {
        let result = finish(
            reply(),
            vec![
                outcome("set_volume", false, None),
                outcome("search_and_play", true, Some("Song")),
            ],
            None,
        );
        assert!(
            !result.executed,
            "a failed call means not everything was executed"
        );
        assert_eq!(result.track_name.as_deref(), Some("Song"));
        assert!(
            result.error.is_none(),
            "per-call failures stay in tool_results"
        );
        assert_eq!(result.tool_results.len(), 2);

        let result = finish(
            reply(),
            vec![
                outcome("search_and_play", true, Some("Song")),
                outcome("set_volume", true, None),
            ],
            None,
        );
        assert!(result.executed);
        assert_eq!(
            result.track_name.as_deref(),
            Some("Song"),
            "the track comes from the play call, not the last call"
        );

        let result = finish(reply(), vec![], None);
        assert!(!result.executed);
        assert!(result.track_name.is_none());
    }

    #[test]
    fn request_level_errors_keep_the_executed_outcomes() {
        let result = finish(
            reply(),
            vec![outcome("like_current", true, None)],
            Some(tools::CANCELLED.to_string()),
        );
        assert!(result.executed);
        assert_eq!(result.error.as_deref(), Some("Cancelled"));
        assert_eq!(result.tool_results[0].name, "like_current");
    }

    #[test]
    fn probe_passes_only_on_exactly_one_correct_set_volume_call() {
        let mut report = serde_json::json!({
            "ok": true,
            "text": "Volume set.",
            "tool_calls": [{"name": "set_volume", "ok": true, "args": {"level": 42}}],
        });
        assert!(probe_passed(&report));
        report["tool_calls"][0]["args"]["level"] = serde_json::json!(42.0);
        assert!(probe_passed(&report));
        report["tool_calls"][0]["args"]["level"] = serde_json::json!("42");
        assert!(!probe_passed(&report), "a numeric string is not a number");
        report["tool_calls"][0]["args"]["level"] = serde_json::json!(42);
        report["tool_calls"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({"name": "like_current", "ok": true, "args": {}}));
        assert!(!probe_passed(&report), "extra tool calls fail the probe");
        assert!(!probe_passed(
            &serde_json::json!({"ok": false, "error": "boom", "tool_calls": []})
        ));
    }
}
