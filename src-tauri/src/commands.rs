use crate::ai::{AgentResponse, AnthropicService, ChatMessage, OpenAIService};
use crate::auth::{AnthropicAuth, OpenAIAuth, SpotifyAuth};
use crate::lyrics::{LyricsFetcher, LyricsInfo};
use crate::spotify::{self, SearchResult, SpotifyDevice, SpotifyWebApi, TrackInfo};
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

    // Get AI description — route by model prefix
    let settings = state.settings.read().await;
    let model = settings.ai_model.clone();
    let prompt = settings.ai_prompt.clone();
    let web_search = settings.ai_web_search;
    let memories = settings.memories.clone();
    drop(settings);

    let ai_result = if model.starts_with("claude-") {
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
    let audio_bytes = tokio::task::spawn_blocking(move || crate::tts::synthesize(&text))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())?;
    Ok(base64::engine::general_purpose::STANDARD.encode(&audio_bytes))
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
    pub openai: bool,
    /// Claude is activated and ready to use
    pub anthropic: bool,
    /// Claude API key was found (but may not be activated yet)
    pub anthropic_available: bool,
    pub spotify: bool,
}

#[tauri::command]
pub async fn get_auth_status(state: State<'_, AppState>) -> Result<AuthStatus, String> {
    let key_available = state.anthropic_auth.is_authenticated();
    let service_active = state.anthropic_service.read().await.is_some();
    Ok(AuthStatus {
        openai: state.openai_auth.is_authenticated().await,
        anthropic: key_available && service_active,
        anthropic_available: key_available,
        spotify: state.spotify_auth.is_authenticated().await,
    })
}

// ============ Anthropic Activation ============

#[tauri::command]
pub async fn anthropic_activate(state: State<'_, AppState>) -> Result<(), String> {
    if !state.anthropic_auth.is_authenticated() {
        return Err("No Anthropic API key found".to_string());
    }
    // Create the service
    let service = crate::ai::AnthropicService::new(Arc::clone(&state.anthropic_auth));
    *state.anthropic_service.write().await = Some(service);
    // Persist the activation
    let mut settings = state.settings.write().await;
    settings.anthropic_enabled = true;
    settings.save().map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub async fn anthropic_deactivate(state: State<'_, AppState>) -> Result<(), String> {
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
    pub executed: bool,
    pub track_name: Option<String>,
    /// Error message when action execution fails
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[tauri::command]
pub async fn agent_chat(
    state: State<'_, AppState>,
    messages: Vec<ChatMessage>,
) -> Result<AgentChatResult, String> {
    let settings = state.settings.read().await;
    let model = if settings.chat_model.is_empty() {
        settings.ai_model.clone()
    } else {
        settings.chat_model.clone()
    };
    let chat_prompt = settings.chat_prompt.clone();
    let web_search = settings.ai_web_search;
    let memories = settings.memories.clone();
    drop(settings);

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

    // Route by model prefix
    let response = if model.starts_with("claude-") {
        let service = state.anthropic_service.read().await;
        let anthropic = service
            .as_ref()
            .ok_or("Anthropic not connected. API key not found.")?;
        anthropic
            .agent_chat(
                &messages,
                &model,
                &chat_prompt,
                &track_name,
                &artist,
                &album,
                volume,
                web_search,
                &memories,
            )
            .await
            .map_err(|e| e.to_string())?
    } else {
        let service = state.openai_service.read().await;
        let openai = service
            .as_ref()
            .ok_or("ChatGPT not connected. Please connect in Settings.")?;
        openai
            .agent_chat(
                &messages,
                &model,
                &chat_prompt,
                &track_name,
                &artist,
                &album,
                volume,
                web_search,
                &memories,
            )
            .await
            .map_err(|e| e.to_string())?
    };

    // Execute the action
    let mut executed = false;
    let mut result_track_name: Option<String> = None;
    let mut action_error: Option<String> = None;

    match response.action.as_str() {
        "search_and_play" => {
            if let Some(query) = response.args.get("query").and_then(|v| v.as_str()) {
                // Search via Web API, then play via AppleScript
                let search_result = {
                    let webapi = state.spotify_webapi.read().await;
                    match webapi.as_ref() {
                        Some(webapi_ref) => Some(webapi_ref.search_tracks(query, 1).await),
                        None => {
                            action_error = Some("Spotify not connected for search".to_string());
                            None
                        }
                    }
                };
                if let Some(search_result) = search_result {
                    match search_result {
                        Ok(results) if !results.is_empty() => {
                            let track = &results[0];
                            result_track_name =
                                Some(format!("{} - {}", track.name, track.artist));
                            let uri = track.uri.clone();
                            match tokio::task::spawn_blocking(move || {
                                spotify::applescript::spotify_play_track(&uri)
                            })
                            .await
                            {
                                Ok(Ok(())) => {
                                    executed = true;
                                }
                                Ok(Err(e)) => {
                                    log::warn!("AppleScript play failed: {}", e);
                                    action_error =
                                        Some(format!("Failed to play track: {}", e));
                                }
                                Err(e) => {
                                    log::warn!("AppleScript play spawn failed: {}", e);
                                    action_error =
                                        Some(format!("Failed to play track: {}", e));
                                }
                            }
                        }
                        Ok(_) => {
                            log::warn!("No search results for: {}", query);
                            action_error =
                                Some(format!("No search results for: {}", query));
                        }
                        Err(e) => {
                            log::warn!("Search failed: {}", e);
                            action_error = Some(format!("Search failed: {}", e));
                        }
                    }
                }
            } else {
                action_error = Some("Missing search query".to_string());
            }
        }
        "like_current" => {
            if let Some(tid) = &track_id {
                let webapi = state.spotify_webapi.read().await;
                if let Some(webapi) = webapi.as_ref() {
                    match webapi.like_track(tid).await {
                        Ok(()) => {
                            executed = true;
                        }
                        Err(e) => log::warn!("Like failed: {}", e),
                    }
                }
            }
        }
        "unlike_current" => {
            if let Some(tid) = &track_id {
                let webapi = state.spotify_webapi.read().await;
                if let Some(webapi) = webapi.as_ref() {
                    match webapi.unlike_track(tid).await {
                        Ok(()) => {
                            executed = true;
                        }
                        Err(e) => log::warn!("Unlike failed: {}", e),
                    }
                }
            }
        }
        "shuffle_liked" => {
            let shuffle_result = {
                let webapi = state.spotify_webapi.read().await;
                if let Some(webapi_ref) = webapi.as_ref() {
                    Some(webapi_ref.get_random_liked_track().await)
                } else {
                    None
                }
            };
            if let Some(Ok(track)) = shuffle_result {
                result_track_name = Some(format!("{} - {}", track.name, track.artist));
                let uri = track.uri.clone();
                match tokio::task::spawn_blocking(move || {
                    spotify::applescript::spotify_play_track(&uri)
                })
                .await
                {
                    Ok(Ok(())) => {
                        executed = true;
                    }
                    Ok(Err(e)) => log::warn!("AppleScript play failed: {}", e),
                    Err(e) => log::warn!("AppleScript spawn failed: {}", e),
                }
            } else if let Some(Err(e)) = shuffle_result {
                log::warn!("Get random liked track failed: {}", e);
            }
        }
        "set_volume" => {
            if let Some(level) = response.args.get("level").and_then(|v| v.as_u64()) {
                let vol = level.min(100) as u32;
                match tokio::task::spawn_blocking(move || {
                    spotify::applescript::set_spotify_volume(vol)
                })
                .await
                {
                    Ok(Ok(())) => {
                        executed = true;
                    }
                    Ok(Err(e)) => log::warn!("Set volume failed: {}", e),
                    Err(e) => log::warn!("Set volume spawn failed: {}", e),
                }
            }
        }
        "save_memory" => {
            if let Some(content) = response.args.get("content").and_then(|v| v.as_str()) {
                let mut settings = state.settings.write().await;
                settings.memories.push(content.to_string());
                // Cap at 50 memories
                if settings.memories.len() > 50 {
                    settings.memories.remove(0);
                }
                if let Err(e) = settings.save() {
                    log::warn!("Failed to save memory: {}", e);
                } else {
                    executed = true;
                }
            }
        }
        "update_prompt" => {
            if let (Some(prompt_type), Some(content)) = (
                response.args.get("type").and_then(|v| v.as_str()),
                response.args.get("content").and_then(|v| v.as_str()),
            ) {
                let mut settings = state.settings.write().await;
                match prompt_type {
                    "insight" => settings.ai_prompt = content.to_string(),
                    "chat" => settings.chat_prompt = content.to_string(),
                    _ => {
                        log::warn!("Unknown prompt type: {}", prompt_type);
                    }
                }
                if let Err(e) = settings.save() {
                    log::warn!("Failed to save updated prompt: {}", e);
                } else {
                    executed = true;
                }
            }
        }
        // ask, refuse, reply — no execution needed
        _ => {}
    }

    Ok(AgentChatResult {
        response,
        executed,
        track_name: result_track_name,
        error: action_error,
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
