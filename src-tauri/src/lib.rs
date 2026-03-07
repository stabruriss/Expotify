mod ai;
mod auth;
mod commands;
mod lyrics;
mod spotify;
mod storage;
mod tts;
mod updater;

use ai::{AnthropicService, OpenAIService};
use auth::{AnthropicAuth, OpenAIAuth, SpotifyAuth};
use commands::{load_overlay_geometry, AppState};
use spotify::SpotifyWebApi;
use std::sync::Arc;
use storage::Settings;
use tauri::menu::{Menu, MenuItem};
use tauri::Manager;
use tauri::RunEvent;
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
                Arc::new(RwLock::new(Some(OpenAIService::new(Arc::clone(
                    &openai_auth,
                )))))
            } else {
                Arc::new(RwLock::new(None))
            };

            // Anthropic auth: read API key from ~/.claude/anthropic_key.sh or env
            let anthropic_auth = Arc::new(AnthropicAuth::new());

            // Load settings early to check anthropic_enabled
            let settings = Settings::load().unwrap_or_default();

            let anthropic_service = if anthropic_auth.is_authenticated() && settings.anthropic_enabled {
                log::info!("[setup] Anthropic API key detected and enabled, creating service");
                Arc::new(RwLock::new(Some(AnthropicService::new(
                    Arc::clone(&anthropic_auth),
                ))))
            } else {
                if anthropic_auth.is_authenticated() {
                    log::info!("[setup] Anthropic API key detected but not activated by user");
                } else {
                    log::info!("[setup] Anthropic API key not found");
                }
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
            };

            app.manage(state);

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
            commands::anthropic_activate,
            commands::anthropic_deactivate,
            // Agent Chat
            commands::agent_chat,
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
