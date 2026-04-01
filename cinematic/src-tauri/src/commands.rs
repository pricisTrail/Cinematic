use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use tauri::State;
use crate::database::Database;
use crate::scanner;
use crate::models::*;

const PLAYBACK_POLL_INTERVAL: Duration = Duration::from_secs(2);
const HTTP_READY_TIMEOUT: Duration = Duration::from_secs(6);
const HTTP_CONNECT_TIMEOUT: Duration = Duration::from_millis(750);
const MIN_RESUME_SECS: f64 = 5.0;
const WATCHED_COMPLETION_RATIO: f64 = 0.98;
const WATCHED_COMPLETION_GRACE_SECS: f64 = 15.0;

#[derive(Debug, Clone)]
pub(crate) struct ManagedPlaybackSession {
    tracking_enabled: bool,
}

#[cfg(target_os = "windows")]
#[derive(Debug, Clone)]
struct VlcStatus {
    time_secs: f64,
    length_secs: Option<f64>,
    state: String,
}

pub struct AppState {
    pub(crate) db: Arc<Database>,
    pub(crate) thumbnails_dir: String,
    pub(crate) playback_sessions: Arc<Mutex<HashMap<String, ManagedPlaybackSession>>>,
}

fn resume_position_for(video_resume_secs: Option<f64>) -> f64 {
    video_resume_secs.unwrap_or(0.0).max(0.0)
}

fn should_mark_watched(time_secs: f64, length_secs: Option<f64>) -> bool {
    let Some(length_secs) = length_secs else {
        return false;
    };

    if length_secs <= 0.0 {
        return false;
    }

    let completion_threshold = (length_secs - WATCHED_COMPLETION_GRACE_SECS)
        .max(length_secs * WATCHED_COMPLETION_RATIO);

    time_secs >= completion_threshold
}

fn search_path_for_binary(binary_name: &str) -> Option<String> {
    let path_var = std::env::var_os("PATH")?;
    for path in std::env::split_paths(&path_var) {
        let candidate = path.join(binary_name);
        if candidate.exists() {
            return Some(candidate.to_string_lossy().to_string());
        }
    }
    None
}

#[cfg(target_os = "windows")]
fn find_vlc_path() -> Option<String> {
    let known_locations = [
        r"C:\Program Files\VideoLAN\VLC\vlc.exe",
        r"C:\Program Files (x86)\VideoLAN\VLC\vlc.exe",
    ];

    for candidate in known_locations {
        if std::path::Path::new(candidate).exists() {
            return Some(candidate.to_string());
        }
    }

    let registry_locations = [
        (r"HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\App Paths\vlc.exe", None),
        (r"HKCU\SOFTWARE\Microsoft\Windows\CurrentVersion\App Paths\vlc.exe", None),
        (r"HKLM\SOFTWARE\VideoLAN\VLC", None),
        (r"HKLM\SOFTWARE\VideoLAN\VLC", Some("InstallDir")),
    ];

    for (key, value_name) in registry_locations {
        if let Some(path) = query_registry_value(key, value_name) {
            let candidate = if value_name == Some("InstallDir") {
                std::path::Path::new(&path).join("vlc.exe").to_string_lossy().to_string()
            } else {
                path
            };

            if std::path::Path::new(&candidate).exists() {
                return Some(candidate);
            }
        }
    }

    search_path_for_binary("vlc.exe")
}

#[cfg(target_os = "windows")]
fn query_registry_value(key: &str, value_name: Option<&str>) -> Option<String> {
    let mut command = std::process::Command::new("reg");
    command.arg("query").arg(key);

    if let Some(name) = value_name {
        command.arg("/v").arg(name);
    } else {
        command.arg("/ve");
    }

    let output = command.output().ok()?;
    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    for line in stdout.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with("HKEY_") {
            continue;
        }

        if let Some((_, data)) = trimmed.split_once("REG_SZ") {
            let value = data.trim();
            if !value.is_empty() {
                return Some(value.to_string());
            }
        }
    }

    None
}

#[cfg(target_os = "macos")]
fn find_vlc_path() -> Option<String> {
    let app_bundle = "/Applications/VLC.app/Contents/MacOS/VLC";
    if std::path::Path::new(app_bundle).exists() {
        return Some(app_bundle.to_string());
    }

    search_path_for_binary("vlc")
}

#[cfg(target_os = "linux")]
fn find_vlc_path() -> Option<String> {
    search_path_for_binary("vlc")
}

fn reserve_local_port() -> Result<u16, String> {
    let listener = TcpListener::bind("127.0.0.1:0").map_err(|e| e.to_string())?;
    let port = listener.local_addr().map_err(|e| e.to_string())?.port();
    drop(listener);
    Ok(port)
}

#[cfg(target_os = "windows")]
fn query_vlc_status(port: u16, password: &str) -> Result<VlcStatus, String> {
    use base64::Engine;

    let address = format!("127.0.0.1:{port}");
    let mut stream = TcpStream::connect_timeout(
        &address.parse().map_err(|e: std::net::AddrParseError| e.to_string())?,
        HTTP_CONNECT_TIMEOUT,
    )
    .map_err(|e| e.to_string())?;

    stream
        .set_read_timeout(Some(HTTP_CONNECT_TIMEOUT))
        .map_err(|e| e.to_string())?;
    stream
        .set_write_timeout(Some(HTTP_CONNECT_TIMEOUT))
        .map_err(|e| e.to_string())?;

    let auth = base64::engine::general_purpose::STANDARD.encode(format!(":{password}"));
    let request = format!(
        "GET /requests/status.xml HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nAuthorization: Basic {auth}\r\nConnection: close\r\n\r\n"
    );

    stream
        .write_all(request.as_bytes())
        .map_err(|e| e.to_string())?;

    let mut response = String::new();
    stream
        .read_to_string(&mut response)
        .map_err(|e| e.to_string())?;

    let (headers, body) = response
        .split_once("\r\n\r\n")
        .ok_or_else(|| "Invalid VLC HTTP response".to_string())?;

    if !headers.starts_with("HTTP/1.1 200") && !headers.starts_with("HTTP/1.0 200") {
        return Err("VLC HTTP interface is not ready".to_string());
    }

    let time_secs = extract_xml_tag(body, "time")
        .and_then(|value| value.parse::<f64>().ok())
        .unwrap_or(0.0);
    let length_secs = extract_xml_tag(body, "length").and_then(|value| value.parse::<f64>().ok());
    let state = extract_xml_tag(body, "state").unwrap_or_else(|| "unknown".to_string());

    if state == "unknown" && !body.contains("<root>") {
        return Err("Unexpected VLC HTTP payload".to_string());
    }

    Ok(VlcStatus {
        time_secs,
        length_secs,
        state,
    })
}

#[cfg(target_os = "windows")]
fn wait_for_vlc_http(port: u16, password: &str) -> bool {
    let deadline = Instant::now() + HTTP_READY_TIMEOUT;
    while Instant::now() < deadline {
        if query_vlc_status(port, password).is_ok() {
            return true;
        }
        thread::sleep(Duration::from_millis(250));
    }
    false
}

fn extract_xml_tag(body: &str, tag: &str) -> Option<String> {
    let open_tag = format!("<{tag}>");
    let close_tag = format!("</{tag}>");
    let start = body.find(&open_tag)? + open_tag.len();
    let end = body[start..].find(&close_tag)? + start;
    Some(body[start..end].trim().to_string())
}

fn open_with_system_default(video_path: &str) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("cmd")
            .args(["/C", "start", "", video_path])
            .spawn()
            .map_err(|e| e.to_string())?;
        return Ok(());
    }

    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg(video_path)
            .spawn()
            .map_err(|e| e.to_string())?;
        return Ok(());
    }

    #[cfg(target_os = "linux")]
    {
        std::process::Command::new("xdg-open")
            .arg(video_path)
            .spawn()
            .map_err(|e| e.to_string())?;
        return Ok(());
    }

    #[allow(unreachable_code)]
    Err("Unsupported platform".to_string())
}

fn spawn_playback_monitor(
    db: Arc<Database>,
    playback_sessions: Arc<Mutex<HashMap<String, ManagedPlaybackSession>>>,
    video_id: String,
    tracking_enabled: bool,
    control_port: Option<u16>,
    control_password: Option<String>,
    mut child: std::process::Child,
) {
    thread::spawn(move || {
        #[cfg(target_os = "windows")]
        let mut last_status: Option<VlcStatus> = None;

        loop {
            #[cfg(target_os = "windows")]
            if tracking_enabled {
                if let (Some(port), Some(password)) = (control_port, control_password.as_deref()) {
                    if let Ok(status) = query_vlc_status(port, password) {
                        let is_finished = should_mark_watched(status.time_secs, status.length_secs)
                            || status.state == "stopped";

                        if status.time_secs >= MIN_RESUME_SECS || is_finished {
                            let _ = db.apply_playback_progress(
                                &video_id,
                                if is_finished { 0.0 } else { status.time_secs },
                                is_finished,
                            );
                        }

                        last_status = Some(status);
                    }
                }
            }

            match child.try_wait() {
                Ok(Some(_)) => {
                    #[cfg(target_os = "windows")]
                    if tracking_enabled {
                        if let Some(status) = last_status {
                            let is_finished =
                                should_mark_watched(status.time_secs, status.length_secs);
                            if status.time_secs >= MIN_RESUME_SECS || is_finished {
                                let _ = db.apply_playback_progress(
                                    &video_id,
                                    if is_finished { 0.0 } else { status.time_secs },
                                    is_finished,
                                );
                            }
                        }
                    }
                    break;
                }
                Ok(None) => thread::sleep(PLAYBACK_POLL_INTERVAL),
                Err(_) => break,
            }
        }

        if let Ok(mut sessions) = playback_sessions.lock() {
            sessions.remove(&video_id);
        }
    });
}

// ─── Library Commands ───

#[tauri::command]
pub fn add_library(state: State<'_, AppState>, path: String, name: String) -> Result<Library, String> {
    // Validate path exists
    if !std::path::Path::new(&path).is_dir() {
        return Err("Directory does not exist".to_string());
    }
    state.db.add_library(&path, &name)
}

#[tauri::command]
pub fn get_libraries(state: State<'_, AppState>) -> Result<Vec<Library>, String> {
    state.db.get_libraries()
}

#[tauri::command]
pub fn remove_library(state: State<'_, AppState>, library_id: String) -> Result<(), String> {
    state.db.remove_library(&library_id)
}

// ─── Video Commands ───

#[tauri::command]
pub fn get_all_videos(state: State<'_, AppState>) -> Result<Vec<VideoRecord>, String> {
    state.db.get_all_videos()
}

#[tauri::command]
pub fn get_videos_by_library(state: State<'_, AppState>, library_id: String) -> Result<Vec<VideoRecord>, String> {
    state.db.get_videos_by_library(&library_id)
}

#[tauri::command]
pub fn set_video_watched(state: State<'_, AppState>, video_id: String, watched: bool) -> Result<(), String> {
    state.db.set_watched(&video_id, watched)
}

#[tauri::command]
pub fn set_video_progress(state: State<'_, AppState>, video_id: String, progress_secs: f64) -> Result<(), String> {
    state.db.set_progress(&video_id, progress_secs)
}

#[tauri::command]
pub fn set_video_favorite(state: State<'_, AppState>, video_id: String, favorite: bool) -> Result<(), String> {
    state.db.set_favorite(&video_id, favorite)
}

#[tauri::command]
pub fn delete_video_file(state: State<'_, AppState>, video_id: String) -> Result<(), String> {
    {
        let sessions = state.playback_sessions.lock().map_err(|e| e.to_string())?;
        if sessions.contains_key(&video_id) {
            return Err("Cannot delete a video while it is playing in a managed VLC session.".to_string());
        }
    }

    let video = state
        .db
        .get_video_by_id(&video_id)?
        .ok_or_else(|| "Video not found".to_string())?;

    match std::fs::remove_file(&video.path) {
        Ok(()) => {}
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => {
            return Err(format!("Failed to delete file from disk: {err}"));
        }
    }

    if let Some(thumbnail_path) = &video.thumbnail_path {
        match std::fs::remove_file(thumbnail_path) {
            Ok(()) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => {}
        }
    }

    state.db.delete_video_record(&video_id)
}

#[tauri::command]
pub fn get_library_stats(state: State<'_, AppState>) -> Result<LibraryStats, String> {
    state.db.get_stats()
}

// ─── Scan Commands ───

#[tauri::command]
pub fn scan_library(state: State<'_, AppState>, library_id: String, library_path: String) -> Result<Vec<VideoRecord>, String> {
    // Phase 1: Scan for video files
    let video_paths = scanner::scan_directory(&library_path);
    
    // Phase 2: Get existing paths so we can skip already-indexed files
    let existing_paths = state.db.get_existing_paths(&library_id)?;
    let existing_set: std::collections::HashSet<&str> = existing_paths.iter().map(|s| s.as_str()).collect();
    
    // Phase 3: Create and insert new video records
    let mut new_count = 0;
    for path in &video_paths {
        if !existing_set.contains(path.as_str()) {
            if let Some(record) = scanner::create_video_record(path, &library_id) {
                state.db.upsert_video(&record)?;
                new_count += 1;
            }
        }
    }

    // Phase 4: Remove videos that no longer exist on disk
    let valid_paths: Vec<String> = video_paths.clone();
    let removed = state.db.remove_videos_not_in_paths(&library_id, &valid_paths)?;
    
    if new_count > 0 || removed > 0 {
        // Return updated list
        state.db.get_videos_by_library(&library_id)
    } else {
        state.db.get_videos_by_library(&library_id)
    }
}

#[tauri::command]
pub fn scan_all_libraries(state: State<'_, AppState>) -> Result<Vec<VideoRecord>, String> {
    let libraries = state.db.get_libraries()?;
    for lib in &libraries {
        let video_paths = scanner::scan_directory(&lib.path);
        let existing_paths = state.db.get_existing_paths(&lib.id)?;
        let existing_set: std::collections::HashSet<&str> = existing_paths.iter().map(|s| s.as_str()).collect();
        
        for path in &video_paths {
            if !existing_set.contains(path.as_str()) {
                if let Some(record) = scanner::create_video_record(path, &lib.id) {
                    state.db.upsert_video(&record)?;
                }
            }
        }
        
        state.db.remove_videos_not_in_paths(&lib.id, &video_paths)?;
    }
    state.db.get_all_videos()
}

// ─── Thumbnail Commands ───

#[tauri::command]
pub fn generate_thumbnails(state: State<'_, AppState>) -> Result<usize, String> {
    let videos = state.db.get_videos_without_thumbnails()?;
    let mut generated = 0;

    for video in &videos {
        // Pick a timestamp: 10% into the video or 5 seconds
        let timestamp = video.duration_secs
            .map(|d| (d * 0.1).min(d - 1.0).max(0.0))
            .unwrap_or(5.0);

        let thumb_filename = format!("{}.jpg", video.id);
        let thumb_path = std::path::Path::new(&state.thumbnails_dir).join(&thumb_filename);
        let thumb_path_str = thumb_path.to_string_lossy().to_string();

        if scanner::generate_thumbnail(&video.path, &thumb_path_str, timestamp).is_ok() {
            state.db.update_thumbnail(&video.id, &thumb_path_str)?;
            generated += 1;
        }
    }

    Ok(generated)
}

#[tauri::command]
pub fn extract_video_metadata(state: State<'_, AppState>, video_id: String, video_path: String) -> Result<(), String> {
    let (duration, width, height) = scanner::extract_metadata(&video_path);
    state.db.update_video_metadata(&video_id, duration, width, height)?;
    
    // Also try to generate thumbnail if missing
    if duration.is_some() {
        let timestamp = duration
            .map(|d| (d * 0.1).min(d - 1.0).max(0.0))
            .unwrap_or(5.0);
        let thumb_filename = format!("{}.jpg", video_id);
        let thumb_path = std::path::Path::new(&state.thumbnails_dir).join(&thumb_filename);
        let thumb_path_str = thumb_path.to_string_lossy().to_string();
        
        if !thumb_path.exists() {
            if scanner::generate_thumbnail(&video_path, &thumb_path_str, timestamp).is_ok() {
                state.db.update_thumbnail(&video_id, &thumb_path_str)?;
            }
        }
    }
    
    Ok(())
}

// ─── Playback Commands ───

#[tauri::command]
pub fn open_in_player(
    state: State<'_, AppState>,
    video_id: String,
    video_path: String,
    resume_secs: Option<f64>,
) -> Result<PlaybackLaunchResult, String> {
    let resume_position_secs = resume_position_for(resume_secs);

    {
        let sessions = state.playback_sessions.lock().map_err(|e| e.to_string())?;
        if let Some(existing) = sessions.get(&video_id) {
            return Ok(PlaybackLaunchResult {
                tracking_enabled: existing.tracking_enabled,
                already_running: true,
                resumed: resume_position_secs >= MIN_RESUME_SECS,
                resume_position_secs,
                fallback_used: !existing.tracking_enabled,
                message: if existing.tracking_enabled {
                    "This video is already playing in a managed VLC session.".to_string()
                } else {
                    "This video is already open, but progress tracking is not available for that session.".to_string()
                },
            });
        }
    }

    #[cfg(target_os = "windows")]
    {
        let Some(vlc_path) = find_vlc_path() else {
            open_with_system_default(&video_path)?;
            return Ok(PlaybackLaunchResult {
                tracking_enabled: false,
                already_running: false,
                resumed: false,
                resume_position_secs: 0.0,
                fallback_used: true,
                message: "VLC was not found. Opened in the system player without progress tracking.".to_string(),
            });
        };

        let control_port = reserve_local_port()?;
        let control_password = uuid::Uuid::new_v4().simple().to_string();

        let mut command = std::process::Command::new(&vlc_path);
        command
            .arg("--play-and-exit")
            .arg("--no-one-instance")
            .arg("--extraintf=http")
            .arg("--http-host=127.0.0.1")
            .arg(format!("--http-port={control_port}"))
            .arg(format!("--http-password={control_password}"))
            .arg("--no-video-title-show");

        if resume_position_secs >= MIN_RESUME_SECS {
            command.arg(format!("--start-time={}", resume_position_secs.floor() as i64));
        }

        command.arg(&video_path);
        let child = command.spawn().map_err(|e| e.to_string())?;

        {
            let mut sessions = state.playback_sessions.lock().map_err(|e| e.to_string())?;
            sessions.insert(
                video_id.clone(),
                ManagedPlaybackSession {
                    tracking_enabled: false,
                },
            );
        }

        let tracking_enabled = wait_for_vlc_http(control_port, &control_password);

        {
            let mut sessions = state.playback_sessions.lock().map_err(|e| e.to_string())?;
            if let Some(session) = sessions.get_mut(&video_id) {
                session.tracking_enabled = tracking_enabled;
            }
        }

        spawn_playback_monitor(
            state.db.clone(),
            state.playback_sessions.clone(),
            video_id,
            tracking_enabled,
            Some(control_port),
            Some(control_password),
            child,
        );

        return Ok(PlaybackLaunchResult {
            tracking_enabled,
            already_running: false,
            resumed: resume_position_secs >= MIN_RESUME_SECS,
            resume_position_secs,
            fallback_used: !tracking_enabled,
            message: if tracking_enabled {
                if resume_position_secs >= MIN_RESUME_SECS {
                    "Opened in VLC and resumed from your last saved position.".to_string()
                } else {
                    "Opened in VLC with progress tracking enabled.".to_string()
                }
            } else {
                "Opened in VLC, but progress tracking could not be enabled for this session.".to_string()
            },
        });
    }

    #[cfg(not(target_os = "windows"))]
    {
        let _ = find_vlc_path();
        open_with_system_default(&video_path)?;
        Ok(PlaybackLaunchResult {
            tracking_enabled: false,
            already_running: false,
            resumed: false,
            resume_position_secs: 0.0,
            fallback_used: true,
            message: "Opened in the system player. Managed VLC progress tracking is only enabled on Windows right now.".to_string(),
        })
    }
}

// ─── Thumbnail serving ───

#[tauri::command]
pub fn get_thumbnail_base64(thumbnail_path: String) -> Result<String, String> {
    use base64::Engine;
    let data = std::fs::read(&thumbnail_path).map_err(|e| e.to_string())?;
    let encoded = base64::engine::general_purpose::STANDARD.encode(&data);
    Ok(format!("data:image/jpeg;base64,{}", encoded))
}

// ─── Collection Commands ───

#[tauri::command]
pub fn create_collection(state: State<'_, AppState>, name: String, description: String) -> Result<Collection, String> {
    state.db.create_collection(&name, &description)
}

#[tauri::command]
pub fn get_collections(state: State<'_, AppState>) -> Result<Vec<Collection>, String> {
    state.db.get_collections()
}

#[tauri::command]
pub fn delete_collection(state: State<'_, AppState>, collection_id: String) -> Result<(), String> {
    state.db.delete_collection(&collection_id)
}

#[tauri::command]
pub fn add_video_to_collection(state: State<'_, AppState>, collection_id: String, video_id: String) -> Result<(), String> {
    state.db.add_video_to_collection(&collection_id, &video_id)
}

#[tauri::command]
pub fn remove_video_from_collection(state: State<'_, AppState>, collection_id: String, video_id: String) -> Result<(), String> {
    state.db.remove_video_from_collection(&collection_id, &video_id)
}

#[tauri::command]
pub fn get_collection_videos(state: State<'_, AppState>, collection_id: String) -> Result<Vec<VideoRecord>, String> {
    state.db.get_collection_videos(&collection_id)
}
