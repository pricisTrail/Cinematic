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

/// Supported video extensions
pub const VIDEO_EXTENSIONS: &[&str] = &[
    "mp4", "mkv", "avi", "mov", "wmv", "flv", "webm", "m4v",
    "mpg", "mpeg", "3gp", "ts", "mts", "m2ts", "vob", "ogv",
];
