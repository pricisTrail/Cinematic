use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Library {
    pub id: String,
    pub path: String,
    pub name: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VideoRecord {
    pub id: String,
    pub library_id: String,
    pub path: String,
    pub title: String,
    pub file_name: String,
    pub file_size: i64,
    pub duration_secs: Option<f64>,
    pub width: Option<i32>,
    pub height: Option<i32>,
    pub thumbnail_path: Option<String>,
    pub watched: i32,
    pub watch_progress_secs: f64,
    pub favorite: i32,
    pub date_added: String,
    pub date_modified: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Collection {
    pub id: String,
    pub name: String,
    pub description: String,
    pub cover_video_id: Option<String>,
    pub cover_image_path: Option<String>,
    pub created_at: String,
    pub video_count: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LibraryStats {
    pub total_videos: i64,
    pub watched_videos: i64,
    pub unwatched_videos: i64,
    pub total_size_bytes: i64,
    pub total_duration_secs: f64,
    pub total_libraries: i64,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScanProgress {
    pub library_id: String,
    pub phase: String,
    pub current: usize,
    pub total: usize,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlaybackLaunchResult {
    pub tracking_enabled: bool,
    pub already_running: bool,
    pub resumed: bool,
    pub resume_position_secs: f64,
    pub fallback_used: bool,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlayerTrack {
    pub id: i64,
    pub kind: String,
    pub title: Option<String>,
    pub lang: Option<String>,
    pub codec: Option<String>,
    pub external: bool,
    pub selected: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlayerStateSnapshot {
    pub video_id: Option<String>,
    pub video_path: Option<String>,
    pub title: Option<String>,
    pub position_secs: f64,
    pub duration_secs: Option<f64>,
    pub paused: bool,
    pub volume: f64,
    pub fullscreen: bool,
    pub is_loaded: bool,
    pub subtitle_tracks: Vec<PlayerTrack>,
    pub audio_tracks: Vec<PlayerTrack>,
    pub active_subtitle_id: Option<i64>,
    pub active_audio_id: Option<i64>,
}

impl Default for PlayerStateSnapshot {
    fn default() -> Self {
        Self {
            video_id: None,
            video_path: None,
            title: None,
            position_secs: 0.0,
            duration_secs: None,
            paused: false,
            volume: 100.0,
            fullscreen: false,
            is_loaded: false,
            subtitle_tracks: Vec::new(),
            audio_tracks: Vec::new(),
            active_subtitle_id: None,
            active_audio_id: None,
        }
    }
}

/// Supported video extensions
pub const VIDEO_EXTENSIONS: &[&str] = &[
    "mp4", "mkv", "avi", "mov", "wmv", "flv", "webm", "m4v", "mpg", "mpeg", "3gp", "ts", "mts",
    "m2ts", "vob", "ogv",
];
