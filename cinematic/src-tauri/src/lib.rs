mod commands;
mod database;
mod models;
mod player;
mod scanner;

use commands::AppState;
use std::sync::Arc;
use tauri::Manager;

fn reassert_main_window_presentation(window: &tauri::Window) {
    let _ = window.set_decorations(false);
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.maximize();
            }

            // Get app data directory for database and thumbnails
            let app_data_dir = app
                .path()
                .app_data_dir()
                .expect("Failed to get app data directory");

            let thumbnails_dir = app_data_dir.join("thumbnails");
            std::fs::create_dir_all(&thumbnails_dir)
                .expect("Failed to create thumbnails directory");
            let collection_covers_dir = app_data_dir.join("collection_covers");
            std::fs::create_dir_all(&collection_covers_dir)
                .expect("Failed to create collection covers directory");

            let db = database::Database::new(app_data_dir).expect("Failed to initialize database");

            app.manage(AppState {
                db: Arc::new(db),
                thumbnails_dir: thumbnails_dir.to_string_lossy().to_string(),
                collection_covers_dir: collection_covers_dir.to_string_lossy().to_string(),
                playback_sessions: Arc::new(
                    std::sync::Mutex::new(std::collections::HashMap::new()),
                ),
                player_manager: Arc::new(player::PlayerManager::new()),
            });

            Ok(())
        })
        .on_window_event(|window, event| match event {
            tauri::WindowEvent::Focused(focused) => {
                if *focused {
                    reassert_main_window_presentation(window);
                }
            }
            _ => {}
        })
        .invoke_handler(tauri::generate_handler![
            // Library
            commands::add_library,
            commands::get_libraries,
            commands::remove_library,
            // Videos
            commands::get_all_videos,
            commands::get_videos_by_library,
            commands::set_video_watched,
            commands::set_video_progress,
            commands::set_video_favorite,
            commands::delete_video_file,
            commands::get_library_stats,
            // Scanning
            commands::scan_library,
            commands::scan_all_libraries,
            // Thumbnails
            commands::generate_thumbnails,
            commands::extract_video_metadata,
            commands::get_thumbnail_base64,
            commands::get_image_base64,
            // Playback
            commands::open_internal_player,
            commands::get_player_snapshot,
            commands::player_set_surface_bounds,
            commands::player_toggle_pause,
            commands::player_seek_relative,
            commands::player_seek_to,
            commands::player_set_volume,
            commands::player_set_subtitle_track,
            commands::player_set_audio_track,
            commands::player_set_speed,
            commands::player_toggle_fullscreen,
            commands::close_internal_player,
            commands::open_external_player,
            commands::open_in_player,
            // Collections
            commands::create_collection,
            commands::get_collections,
            commands::delete_collection,
            commands::set_collection_cover,
            commands::set_collection_cover_image,
            commands::add_video_to_collection,
            commands::remove_video_from_collection,
            commands::get_collection_videos,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
