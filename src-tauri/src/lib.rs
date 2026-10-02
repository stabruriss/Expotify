mod ai;
mod auth;
mod claude_runtime;
mod commands;
mod faults;
mod lyrics;
mod spotify;
mod storage;
mod tts;
mod updater;

use ai::{AnthropicService, OpenAIService};
use auth::{AnthropicAuth, OpenAIAuth, SpotifyAuth};
use commands::{load_overlay_geometry, AppState};
use spotify::SpotifyWebApi;
use std::path::PathBuf;
use std::sync::Arc;
use storage::Settings;
use tauri::menu::{Menu, MenuItem};
use tauri::Manager;
use tauri::RunEvent;
use tokio::sync::RwLock;

fn claude_helper_path(app: &tauri::App) -> Result<PathBuf, String> {
    let resource_dir = app.path().resource_dir().map_err(|e| e.to_string())?;
    let bundled = resource_dir.join("claude/expotify-claude-helper");
    if bundled.is_file() {
        return Ok(bundled);
    }
    if cfg!(debug_assertions) {
        let staged = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../binaries/claude/expotify-claude-helper");
        if staged.is_file() {
            return Ok(staged);
        }
    }
    Err("Bundled Claude runtime is missing. Rebuild or reinstall Expotify.".into())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    env_logger::init();

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            // Token is loaded synchronously from keychain inside new()
            let openai_auth = Arc::new(OpenAIAuth::new());
            let openai_service = if openai_auth.has_stored_token() {
                Arc::new(RwLock::new(Some(OpenAIService::new(Arc::clone(
                    &openai_auth,
                )))))
            } else {
                Arc::new(RwLock::new(None))
            };

            let claude_runtime = Arc::new(claude_runtime::ClaudeRuntime::new(
                claude_helper_path(app)?,
                app.path().app_data_dir()?.join("claude-runtime"),
            )?);
            let anthropic_auth = Arc::new(AnthropicAuth::new(claude_runtime));

            // Load settings early to check anthropic_enabled
            let mut settings = Settings::load()?;
            if openai_auth.has_stored_token() {
                settings.initialize_model_defaults(ai::models::ModelProvider::Openai);
            }

            let anthropic_service = if settings.anthropic_enabled {
                Arc::new(RwLock::new(Some(AnthropicService::new(Arc::clone(
                    &anthropic_auth,
                )))))
            } else {
                Arc::new(RwLock::new(None))
            };

            // Spotify auth: sp_dc cookie loaded from keychain
            let spotify_auth = Arc::new(SpotifyAuth::new());
            let spotify_webapi = if spotify_auth.has_sp_dc() {
                Arc::new(RwLock::new(Some(SpotifyWebApi::new(Arc::clone(
                    &spotify_auth,
                )))))
            } else {
                Arc::new(RwLock::new(None))
            };

            let state = AppState {
                openai_auth,
                openai_service,
                anthropic_auth,
                anthropic_service,
                spotify_auth,
                spotify_webapi,
                settings: Arc::new(RwLock::new(settings)),
                current_track: Arc::new(RwLock::new(None)),
                lyrics_fetcher: lyrics::LyricsFetcher::new(),
                chat_cancel: tokio::sync::Mutex::new(None),
            };

            app.manage(state);

            // Debug-only endpoint probe: EXPOTIFY_TOOL_PROBE=openai|anthropic runs the native
            // tool-calling path once with a dry-run executor and writes the report next to
            // settings.json. Never compiled into release builds.
            #[cfg(debug_assertions)]
            if let Ok(provider) = std::env::var("EXPOTIFY_TOOL_PROBE") {
                let handle = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                    let state = handle.state::<commands::AppState>();
                    let target = match provider.as_str() {
                        "openai" => Some(ai::models::ModelProvider::Openai),
                        "anthropic" => Some(ai::models::ModelProvider::Anthropic),
                        _ => None,
                    };
                    // A report left by an earlier run is moved aside first, so the report file
                    // only appears once this run has finished (poll for it).
                    let report_path = dirs::config_dir().map(|dir| {
                        dir.join("expotify")
                            .join(format!("tool-probe-{provider}.json"))
                    });
                    if let Some(path) = report_path.as_ref().filter(|path| path.is_file()) {
                        let _ = std::fs::rename(path, path.with_extension("prev.json"));
                    }
                    let report = match target {
                        Some(target) => commands::run_tool_probe(state.inner(), target).await,
                        None => Err(format!(
                            "EXPOTIFY_TOOL_PROBE must be openai or anthropic, got '{provider}'"
                        )),
                    };
                    let report = report.unwrap_or_else(
                        |error| serde_json::json!({"ok": false, "pass": false, "error": error}),
                    );
                    log::info!("[tool-probe] {report}");
                    if let Some(path) = report_path {
                        if let Err(error) = std::fs::write(
                            &path,
                            serde_json::to_string_pretty(&report).unwrap_or_default(),
                        ) {
                            log::warn!("[tool-probe] could not write {}: {error}", path.display());
                        }
                    }
                });
            }

            #[cfg(debug_assertions)]
            if let Some(main) = app.get_webview_window("main") {
                let _ = main.set_title("Expotify (Local Test)");
                let _ = main.show();
            }

            // Restore overlay geometry before showing the window
            if let Some(overlay) = app.get_webview_window("overlay") {
                if let Ok(Some(geo)) = load_overlay_geometry() {
                    if geo.width > 0.0 && geo.height > 0.0 {
                        let _ = overlay.set_size(tauri::LogicalSize::new(geo.width, geo.height));

                        // Clamp position to ensure the overlay is on-screen
                        let mut x = geo.x;
                        let mut y = geo.y;
                        let w = geo.width;
                        let h = geo.height;

                        if let Ok(monitors) = overlay.available_monitors() {
                            let on_screen = monitors.iter().any(|m| {
                                let pos = m.position();
                                let size = m.size();
                                let sf = m.scale_factor();
                                let mx = pos.x as f64 / sf;
                                let my = pos.y as f64 / sf;
                                let mw = size.width as f64 / sf;
                                let mh = size.height as f64 / sf;
                                // At least 50px of the overlay must be visible on this monitor
                                x + 50.0 > mx
                                    && x < mx + mw - 50.0
                                    && y + 50.0 > my
                                    && y < my + mh - 50.0
                            });

                            if !on_screen {
                                // Reset to primary monitor or first available
                                if let Some(m) = overlay
                                    .primary_monitor()
                                    .ok()
                                    .flatten()
                                    .or_else(|| monitors.first().cloned())
                                {
                                    let pos = m.position();
                                    let size = m.size();
                                    let sf = m.scale_factor();
                                    let mx = pos.x as f64 / sf;
                                    let my = pos.y as f64 / sf;
                                    let mw = size.width as f64 / sf;
                                    let mh = size.height as f64 / sf;
                                    // Place at bottom-right with some margin
                                    x = mx + mw - w - 32.0;
                                    y = my + mh - h - 32.0;
                                    if x < mx {
                                        x = mx + 32.0;
                                    }
                                    if y < my {
                                        y = my + 32.0;
                                    }
                                }
                            }
                        }

                        let _ = overlay.set_position(tauri::LogicalPosition::new(x, y));
                    }
                }
                let _ = overlay.show();
            }

            // Build tray menu
            let toggle_overlay = MenuItem::with_id(
                app,
                "toggle_overlay",
                "Show/Hide Overlay",
                true,
                None::<&str>,
            )?;
            let open_expotify =
                MenuItem::with_id(app, "open_expotify", "Open Expotify", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;

            let menu = Menu::with_items(app, &[&toggle_overlay, &open_expotify, &quit])?;

            if let Some(tray) = app.tray_by_id("main") {
                tray.set_menu(Some(menu))?;
                tray.on_menu_event(move |app, event| match event.id.as_ref() {
                    "toggle_overlay" => {
                        if let Some(window) = app.get_webview_window("overlay") {
                            if window.is_visible().unwrap_or(false) {
                                let _ = window.hide();
                            } else {
                                let _ = window.show();
                            }
                        }
                    }
                    "open_expotify" => {
                        if let Some(window) = app.get_webview_window("main") {
                            let _ = window.show();
                            let _ = window.set_focus();
                        }
                    }
                    "quit" => {
                        app.exit(0);
                    }
                    _ => {}
                });
            }

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
            commands::toggle_overlay,
            commands::show_main_window,
            commands::save_overlay_geometry,
            commands::load_overlay_geometry,
            commands::spotify_play_pause,
            commands::spotify_next_track,
            commands::spotify_previous_track,
            commands::spotify_pause,
            commands::spotify_play,
            commands::tts_synthesize,
            commands::tts_check_available,
            commands::check_for_update,
            commands::open_url,
            // Spotify Web API
            commands::spotify_is_authenticated,
            commands::spotify_connect,
            commands::spotify_login,
            commands::spotify_disconnect,
            commands::spotify_search,
            commands::spotify_is_track_liked,
            commands::spotify_like_track,
            commands::spotify_unlike_track,
            commands::spotify_shuffle_liked,
            commands::spotify_get_devices,
            commands::spotify_transfer_playback,
            commands::spotify_get_volume,
            commands::spotify_set_volume,
            commands::spotify_play_track,
            // Anthropic
            commands::anthropic_start_oauth,
            commands::anthropic_cancel_oauth,
            commands::anthropic_logout,
            // Agent Chat
            commands::agent_chat,
            commands::agent_chat_cancel,
            commands::tool_protocol_probe,
            // Model listing
            commands::list_models,
        ])
        .on_window_event(|window, event| {
            // Hide windows on close instead of destroying, so they can be reopened
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                if window.label() == "main" || window.label() == "overlay" {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        })
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app, event| {
            if let RunEvent::Reopen {
                has_visible_windows,
                ..
            } = event
            {
                if !has_visible_windows {
                    if let Some(window) = app.get_webview_window("main") {
                        let _ = window.show();
                        let _ = window.set_focus();
                    }
                }
            }
        });
}
