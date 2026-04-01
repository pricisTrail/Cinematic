use std::path::Path;
use walkdir::WalkDir;
use crate::models::*;

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

/// Extract metadata from a video file using ffprobe
pub fn extract_metadata(file_path: &str) -> (Option<f64>, Option<i32>, Option<i32>) {
    let output = std::process::Command::new("ffprobe")
        .args([
            "-v", "quiet",
            "-print_format", "json",
            "-show_format",
            "-show_streams",
            "-select_streams", "v:0",
            file_path,
        ])
        .output();

    match output {
        Ok(out) => {
            if let Ok(json_str) = String::from_utf8(out.stdout) {
                if let Ok(json) = serde_json::from_str::<serde_json::Value>(&json_str) {
                    let duration = json["format"]["duration"]
                        .as_str()
                        .and_then(|d| d.parse::<f64>().ok());

                    let streams = json["streams"].as_array();
                    let (width, height) = if let Some(streams) = streams {
                        if let Some(stream) = streams.first() {
                            (
                                stream["width"].as_i64().map(|v| v as i32),
                                stream["height"].as_i64().map(|v| v as i32),
                            )
                        } else {
                            (None, None)
                        }
                    } else {
                        (None, None)
                    };

                    return (duration, width, height);
                }
            }
            (None, None, None)
        }
        Err(_) => (None, None, None),
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
