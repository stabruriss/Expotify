mod auth;
mod ai;
mod commands;
mod lyrics;
mod spotify;
mod storage;
mod updater;

use ai::OpenAIService;
use auth::OpenAIAuth;
use commands::{AppState, load_overlay_geometry};
use storage::Settings;
use std::sync::Arc;
use tauri::Manager;
use tauri::menu::{Menu, MenuItem};
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

            // Restore overlay geometry before showing the window
            if let Some(overlay) = app.get_webview_window("overlay") {
                if let Ok(Some(geo)) = load_overlay_geometry() {
                    if geo.width > 0.0 && geo.height > 0.0 {
                        let _ = overlay.set_size(tauri::LogicalSize::new(geo.width, geo.height));
                        let _ = overlay.set_position(tauri::LogicalPosition::new(geo.x, geo.y));
                    }
                }
                let _ = overlay.show();
            }

            // Build tray menu
            let toggle_overlay = MenuItem::with_id(app, "toggle_overlay", "Show/Hide Overlay", true, None::<&str>)?;
            let open_expotify = MenuItem::with_id(app, "open_expotify", "Open Expotify", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;

            let menu = Menu::with_items(app, &[&toggle_overlay, &open_expotify, &quit])?;

            if let Some(tray) = app.tray_by_id("main") {
                tray.set_menu(Some(menu))?;
                tray.on_menu_event(move |app, event| {
                    match event.id.as_ref() {
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
                    }
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
            commands::check_for_update,
            commands::open_url,
        ])
        .on_window_event(|window, event| {
            // Hide main window on close instead of destroying, so it can be reopened
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                if window.label() == "main" {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        })
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app, event| {
            if let RunEvent::Reopen { has_visible_windows, .. } = event {
                if !has_visible_windows {
                    if let Some(window) = app.get_webview_window("main") {
                        let _ = window.show();
                        let _ = window.set_focus();
                    }
                }
            }
        });
}
