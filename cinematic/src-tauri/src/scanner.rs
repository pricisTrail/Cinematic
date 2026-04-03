use std::path::Path;
use walkdir::WalkDir;
use crate::models::*;
use serde_json::Value;

#[derive(Debug, Clone, Default)]
pub struct ProbedVideoMetadata {
    pub duration_secs: Option<f64>,
    pub width: Option<i32>,
    pub height: Option<i32>,
    pub embedded_artwork_stream_index: Option<i32>,
}

/// Scan a directory for video files. Returns list of file paths.
pub fn scan_directory(dir_path: &str) -> Vec<String> {
    let mut video_paths = Vec::new();
    
    let path = Path::new(dir_path);
    if !path.exists() || !path.is_dir() {
        return video_paths;
    }

    for entry in WalkDir::new(dir_path)
        .follow_links(true)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        let path = entry.path();
        if path.is_file() {
            if let Some(ext) = path.extension() {
                let ext_lower = ext.to_string_lossy().to_lowercase();
                if VIDEO_EXTENSIONS.contains(&ext_lower.as_str()) {
                    if let Some(path_str) = path.to_str() {
                        video_paths.push(path_str.to_string());
                    }
                }
            }
        }
    }

    video_paths
}

/// Create a VideoRecord from a file path
pub fn create_video_record(file_path: &str, library_id: &str) -> Option<VideoRecord> {
    let path = Path::new(file_path);
    if !path.exists() {
        return None;
    }

    let metadata = std::fs::metadata(path).ok()?;
    let file_name = path.file_name()?.to_string_lossy().to_string();
    
    // Generate a clean title from filename
    let title = path.file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| file_name.clone())
        // Clean up common patterns
        .replace('_', " ")
        .replace('.', " ")
        .replace('-', " ");

    let now = chrono::Utc::now().to_rfc3339();
    let modified = metadata.modified()
        .map(|t| chrono::DateTime::<chrono::Utc>::from(t).to_rfc3339())
        .unwrap_or_else(|_| now.clone());

    Some(VideoRecord {
        id: uuid::Uuid::new_v4().to_string(),
        library_id: library_id.to_string(),
        path: file_path.to_string(),
        title,
        file_name,
        file_size: metadata.len() as i64,
        duration_secs: None,
        width: None,
        height: None,
        thumbnail_path: None,
        watched: 0,
        watch_progress_secs: 0.0,
        favorite: 0,
        date_added: now,
        date_modified: modified,
    })
}

fn probe_media(file_path: &str) -> Option<Value> {
    let output = std::process::Command::new("ffprobe")
        .args([
            "-v", "quiet",
            "-print_format", "json",
            "-show_format",
            "-show_streams",
            file_path,
        ])
        .output()
        .ok()?;

    let json_str = String::from_utf8(output.stdout).ok()?;
    serde_json::from_str::<Value>(&json_str).ok()
}

fn is_attached_picture_stream(stream: &Value) -> bool {
    stream["disposition"]["attached_pic"].as_i64() == Some(1)
}

fn stream_index(stream: &Value) -> Option<i32> {
    stream["index"].as_i64().map(|value| value as i32)
}

/// Extract metadata from a video file using ffprobe.
pub fn extract_metadata(file_path: &str) -> ProbedVideoMetadata {
    let Some(json) = probe_media(file_path) else {
        return ProbedVideoMetadata::default();
    };

    let duration = json["format"]["duration"]
        .as_str()
        .and_then(|value| value.parse::<f64>().ok());

    let video_streams: Vec<&Value> = json["streams"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|stream| stream["codec_type"].as_str() == Some("video"))
        .collect();

    let embedded_artwork_stream_index = video_streams
        .iter()
        .copied()
        .find(|stream| is_attached_picture_stream(stream))
        .and_then(stream_index);

    let primary_video_stream = video_streams
        .iter()
        .copied()
        .find(|stream| !is_attached_picture_stream(stream))
        .or_else(|| video_streams.first().copied());

    ProbedVideoMetadata {
        duration_secs: duration,
        width: primary_video_stream.and_then(|stream| stream["width"].as_i64().map(|value| value as i32)),
        height: primary_video_stream.and_then(|stream| stream["height"].as_i64().map(|value| value as i32)),
        embedded_artwork_stream_index,
    }
}

/// Generate a thumbnail for a video using ffmpeg
pub fn generate_thumbnail(video_path: &str, output_path: &str, timestamp_secs: f64) -> Result<(), String> {
    let timestamp = format!("{:.2}", timestamp_secs);
    
    // Ensure the output directory exists
    if let Some(parent) = Path::new(output_path).parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }

    let output = std::process::Command::new("ffmpeg")
        .args([
            "-y",
            "-ss", &timestamp,
            "-i", video_path,
            "-vframes", "1",
            "-q:v", "3",
            "-vf", "scale=480:-1",
            output_path,
        ])
        .output()
        .map_err(|e| format!("Failed to run ffmpeg: {}", e))?;

    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(format!("ffmpeg failed: {}", stderr))
    }
}

/// Extract embedded cover artwork into the thumbnail slot when the container has an attached picture stream.
pub fn extract_embedded_artwork(video_path: &str, output_path: &str, stream_index: i32) -> Result<(), String> {
    if let Some(parent) = Path::new(output_path).parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }

    let map_arg = format!("0:{stream_index}");
    let output = std::process::Command::new("ffmpeg")
        .args([
            "-y",
            "-i", video_path,
            "-map", &map_arg,
            "-frames:v", "1",
            "-q:v", "3",
            "-vf", "scale=480:-1",
            output_path,
        ])
        .output()
        .map_err(|e| format!("Failed to run ffmpeg: {}", e))?;

    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(format!("ffmpeg failed: {}", stderr))
    }
}
