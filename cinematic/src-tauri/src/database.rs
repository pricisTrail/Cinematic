use crate::models::*;
use rusqlite::{params, Connection};
use std::path::PathBuf;
use std::sync::Mutex;

pub struct Database {
    conn: Mutex<Connection>,
}

impl Database {
    pub fn new(app_data_dir: PathBuf) -> Result<Self, String> {
        std::fs::create_dir_all(&app_data_dir).map_err(|e| e.to_string())?;
        let db_path = app_data_dir.join("cinematic.db");
        let conn = Connection::open(&db_path).map_err(|e| e.to_string())?;

        let db = Database {
            conn: Mutex::new(conn),
        };
        db.initialize_tables()?;
        Ok(db)
    }

    fn initialize_tables(&self) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute_batch(
            "
            PRAGMA journal_mode=WAL;
            PRAGMA synchronous=NORMAL;
            PRAGMA cache_size=10000;
            PRAGMA temp_store=MEMORY;

            CREATE TABLE IF NOT EXISTS libraries (
                id TEXT PRIMARY KEY,
                path TEXT NOT NULL UNIQUE,
                name TEXT NOT NULL,
                created_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS videos (
                id TEXT PRIMARY KEY,
                library_id TEXT NOT NULL,
                path TEXT NOT NULL UNIQUE,
                title TEXT NOT NULL,
                file_name TEXT NOT NULL,
                file_size INTEGER NOT NULL,
                duration_secs REAL,
                width INTEGER,
                height INTEGER,
                thumbnail_path TEXT,
                watched INTEGER NOT NULL DEFAULT 0,
                watch_progress_secs REAL DEFAULT 0,
                favorite INTEGER NOT NULL DEFAULT 0,
                date_added TEXT NOT NULL,
                date_modified TEXT NOT NULL,
                FOREIGN KEY (library_id) REFERENCES libraries(id) ON DELETE CASCADE
            );

            CREATE TABLE IF NOT EXISTS collections (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                description TEXT DEFAULT '',
                cover_video_id TEXT,
                cover_image_path TEXT,
                created_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS collection_videos (
                collection_id TEXT NOT NULL,
                video_id TEXT NOT NULL,
                position INTEGER NOT NULL DEFAULT 0,
                PRIMARY KEY (collection_id, video_id),
                FOREIGN KEY (collection_id) REFERENCES collections(id) ON DELETE CASCADE,
                FOREIGN KEY (video_id) REFERENCES videos(id) ON DELETE CASCADE
            );

            CREATE INDEX IF NOT EXISTS idx_videos_library ON videos(library_id);
            CREATE INDEX IF NOT EXISTS idx_videos_watched ON videos(watched);
            CREATE INDEX IF NOT EXISTS idx_videos_date_added ON videos(date_added);
            CREATE INDEX IF NOT EXISTS idx_videos_title ON videos(title);
        ",
        )
        .map_err(|e| e.to_string())?;

        if let Err(e) = conn.execute(
            "ALTER TABLE collections ADD COLUMN cover_image_path TEXT",
            [],
        ) {
            if !e.to_string().contains("duplicate column name") {
                return Err(e.to_string());
            }
        }
        Ok(())
    }

    // ─── Library Operations ───

    pub fn add_library(&self, path: &str, name: &str) -> Result<Library, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let id = uuid::Uuid::new_v4().to_string();
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO libraries (id, path, name, created_at) VALUES (?1, ?2, ?3, ?4)",
            params![id, path, name, now],
        )
        .map_err(|e| e.to_string())?;
        Ok(Library {
            id,
            path: path.to_string(),
            name: name.to_string(),
            created_at: now,
        })
    }

    pub fn get_libraries(&self) -> Result<Vec<Library>, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let mut stmt = conn
            .prepare("SELECT id, path, name, created_at FROM libraries ORDER BY name")
            .map_err(|e| e.to_string())?;
        let libs = stmt
            .query_map([], |row| {
                Ok(Library {
                    id: row.get(0)?,
                    path: row.get(1)?,
                    name: row.get(2)?,
                    created_at: row.get(3)?,
                })
            })
            .map_err(|e| e.to_string())?
            .filter_map(|r| r.ok())
            .collect();
        Ok(libs)
    }

    pub fn remove_library(&self, id: &str) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute("DELETE FROM videos WHERE library_id = ?1", params![id])
            .map_err(|e| e.to_string())?;
        conn.execute("DELETE FROM libraries WHERE id = ?1", params![id])
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    // ─── Video Operations ───

    pub fn upsert_video(&self, video: &VideoRecord) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "INSERT INTO videos (id, library_id, path, title, file_name, file_size, duration_secs, width, height, thumbnail_path, watched, watch_progress_secs, favorite, date_added, date_modified)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)
             ON CONFLICT(path) DO UPDATE SET
                file_size = excluded.file_size,
                date_modified = excluded.date_modified,
                duration_secs = COALESCE(excluded.duration_secs, duration_secs),
                width = COALESCE(excluded.width, width),
                height = COALESCE(excluded.height, height),
                thumbnail_path = COALESCE(excluded.thumbnail_path, thumbnail_path)",
            params![
                video.id, video.library_id, video.path, video.title, video.file_name,
                video.file_size, video.duration_secs, video.width, video.height,
                video.thumbnail_path, video.watched, video.watch_progress_secs,
                video.favorite, video.date_added, video.date_modified,
            ],
        ).map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn get_all_videos(&self) -> Result<Vec<VideoRecord>, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let mut stmt = conn.prepare(
            "SELECT id, library_id, path, title, file_name, file_size, duration_secs, width, height, thumbnail_path, watched, watch_progress_secs, favorite, date_added, date_modified FROM videos ORDER BY date_added DESC"
        ).map_err(|e| e.to_string())?;
        let videos = stmt
            .query_map([], |row| {
                Ok(VideoRecord {
                    id: row.get(0)?,
                    library_id: row.get(1)?,
                    path: row.get(2)?,
                    title: row.get(3)?,
                    file_name: row.get(4)?,
                    file_size: row.get(5)?,
                    duration_secs: row.get(6)?,
                    width: row.get(7)?,
                    height: row.get(8)?,
                    thumbnail_path: row.get(9)?,
                    watched: row.get(10)?,
                    watch_progress_secs: row.get(11)?,
                    favorite: row.get(12)?,
                    date_added: row.get(13)?,
                    date_modified: row.get(14)?,
                })
            })
            .map_err(|e| e.to_string())?
            .filter_map(|r| r.ok())
            .collect();
        Ok(videos)
    }

    pub fn get_videos_by_library(&self, library_id: &str) -> Result<Vec<VideoRecord>, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let mut stmt = conn.prepare(
            "SELECT id, library_id, path, title, file_name, file_size, duration_secs, width, height, thumbnail_path, watched, watch_progress_secs, favorite, date_added, date_modified FROM videos WHERE library_id = ?1 ORDER BY date_added DESC"
        ).map_err(|e| e.to_string())?;
        let videos = stmt
            .query_map(params![library_id], |row| {
                Ok(VideoRecord {
                    id: row.get(0)?,
                    library_id: row.get(1)?,
                    path: row.get(2)?,
                    title: row.get(3)?,
                    file_name: row.get(4)?,
                    file_size: row.get(5)?,
                    duration_secs: row.get(6)?,
                    width: row.get(7)?,
                    height: row.get(8)?,
                    thumbnail_path: row.get(9)?,
                    watched: row.get(10)?,
                    watch_progress_secs: row.get(11)?,
                    favorite: row.get(12)?,
                    date_added: row.get(13)?,
                    date_modified: row.get(14)?,
                })
            })
            .map_err(|e| e.to_string())?
            .filter_map(|r| r.ok())
            .collect();
        Ok(videos)
    }

    pub fn get_video_by_id(&self, video_id: &str) -> Result<Option<VideoRecord>, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let mut stmt = conn.prepare(
            "SELECT id, library_id, path, title, file_name, file_size, duration_secs, width, height, thumbnail_path, watched, watch_progress_secs, favorite, date_added, date_modified FROM videos WHERE id = ?1 LIMIT 1"
        ).map_err(|e| e.to_string())?;

        match stmt.query_row(params![video_id], |row| {
            Ok(VideoRecord {
                id: row.get(0)?,
                library_id: row.get(1)?,
                path: row.get(2)?,
                title: row.get(3)?,
                file_name: row.get(4)?,
                file_size: row.get(5)?,
                duration_secs: row.get(6)?,
                width: row.get(7)?,
                height: row.get(8)?,
                thumbnail_path: row.get(9)?,
                watched: row.get(10)?,
                watch_progress_secs: row.get(11)?,
                favorite: row.get(12)?,
                date_added: row.get(13)?,
                date_modified: row.get(14)?,
            })
        }) {
            Ok(video) => Ok(Some(video)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.to_string()),
        }
    }

    pub fn delete_video_record(&self, video_id: &str) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "DELETE FROM collection_videos WHERE video_id = ?1",
            params![video_id],
        )
        .map_err(|e| e.to_string())?;
        conn.execute("DELETE FROM videos WHERE id = ?1", params![video_id])
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn set_watched(&self, video_id: &str, watched: bool) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "UPDATE videos SET watched = ?1 WHERE id = ?2",
            params![watched as i32, video_id],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn set_progress(&self, video_id: &str, progress_secs: f64) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "UPDATE videos SET watch_progress_secs = ?1 WHERE id = ?2",
            params![progress_secs, video_id],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn apply_playback_progress(
        &self,
        video_id: &str,
        progress_secs: f64,
        mark_watched: bool,
    ) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        if mark_watched {
            conn.execute(
                "UPDATE videos SET watched = 1, watch_progress_secs = 0 WHERE id = ?1",
                params![video_id],
            )
            .map_err(|e| e.to_string())?;
        } else {
            conn.execute(
                "UPDATE videos SET watch_progress_secs = ?1 WHERE id = ?2",
                params![progress_secs.max(0.0), video_id],
            )
            .map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    pub fn set_favorite(&self, video_id: &str, favorite: bool) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "UPDATE videos SET favorite = ?1 WHERE id = ?2",
            params![favorite as i32, video_id],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn update_thumbnail(&self, video_id: &str, thumbnail_path: &str) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "UPDATE videos SET thumbnail_path = ?1 WHERE id = ?2",
            params![thumbnail_path, video_id],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn update_video_metadata(
        &self,
        video_id: &str,
        duration: Option<f64>,
        width: Option<i32>,
        height: Option<i32>,
    ) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "UPDATE videos SET duration_secs = COALESCE(?1, duration_secs), width = COALESCE(?2, width), height = COALESCE(?3, height) WHERE id = ?4",
            params![duration, width, height, video_id],
        ).map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn remove_videos_not_in_paths(
        &self,
        library_id: &str,
        valid_paths: &[String],
    ) -> Result<usize, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;

        if valid_paths.is_empty() {
            let count = conn
                .execute(
                    "DELETE FROM videos WHERE library_id = ?1",
                    params![library_id],
                )
                .map_err(|e| e.to_string())?;
            return Ok(count);
        }

        let placeholders: Vec<String> = valid_paths
            .iter()
            .enumerate()
            .map(|(i, _)| format!("?{}", i + 2))
            .collect();
        let sql = format!(
            "DELETE FROM videos WHERE library_id = ?1 AND path NOT IN ({})",
            placeholders.join(", ")
        );

        let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
        let mut param_values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
        param_values.push(Box::new(library_id.to_string()));
        for p in valid_paths {
            param_values.push(Box::new(p.clone()));
        }
        let params_refs: Vec<&dyn rusqlite::types::ToSql> =
            param_values.iter().map(|p| p.as_ref()).collect();
        let count = stmt
            .execute(params_refs.as_slice())
            .map_err(|e| e.to_string())?;
        Ok(count)
    }

    pub fn get_existing_paths(&self, library_id: &str) -> Result<Vec<String>, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let mut stmt = conn
            .prepare("SELECT path FROM videos WHERE library_id = ?1")
            .map_err(|e| e.to_string())?;
        let paths = stmt
            .query_map(params![library_id], |row| row.get::<_, String>(0))
            .map_err(|e| e.to_string())?
            .filter_map(|r| r.ok())
            .collect();
        Ok(paths)
    }

    pub fn get_videos_without_thumbnails(&self, limit: usize) -> Result<Vec<VideoRecord>, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let mut stmt = conn.prepare(
            "SELECT id, library_id, path, title, file_name, file_size, duration_secs, width, height, thumbnail_path, watched, watch_progress_secs, favorite, date_added, date_modified FROM videos WHERE thumbnail_path IS NULL OR thumbnail_path = '' LIMIT ?1"
        ).map_err(|e| e.to_string())?;
        let videos = stmt
            .query_map(params![limit as i64], |row| {
                Ok(VideoRecord {
                    id: row.get(0)?,
                    library_id: row.get(1)?,
                    path: row.get(2)?,
                    title: row.get(3)?,
                    file_name: row.get(4)?,
                    file_size: row.get(5)?,
                    duration_secs: row.get(6)?,
                    width: row.get(7)?,
                    height: row.get(8)?,
                    thumbnail_path: row.get(9)?,
                    watched: row.get(10)?,
                    watch_progress_secs: row.get(11)?,
                    favorite: row.get(12)?,
                    date_added: row.get(13)?,
                    date_modified: row.get(14)?,
                })
            })
            .map_err(|e| e.to_string())?
            .filter_map(|r| r.ok())
            .collect();
        Ok(videos)
    }

    // ─── Collection Operations ───

    pub fn create_collection(&self, name: &str, description: &str) -> Result<Collection, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let id = uuid::Uuid::new_v4().to_string();
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO collections (id, name, description, created_at) VALUES (?1, ?2, ?3, ?4)",
            params![id, name, description, now],
        )
        .map_err(|e| e.to_string())?;
        Ok(Collection {
            id,
            name: name.to_string(),
            description: description.to_string(),
            cover_video_id: None,
            cover_image_path: None,
            created_at: now,
            video_count: 0,
        })
    }

    pub fn get_collections(&self) -> Result<Vec<Collection>, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let mut stmt = conn.prepare(
            "SELECT c.id, c.name, c.description, c.cover_video_id, c.cover_image_path, c.created_at, COUNT(cv.video_id) as video_count
             FROM collections c
             LEFT JOIN collection_videos cv ON c.id = cv.collection_id
             GROUP BY c.id
             ORDER BY c.name"
        ).map_err(|e| e.to_string())?;
        let cols = stmt
            .query_map([], |row| {
                Ok(Collection {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    description: row.get(2)?,
                    cover_video_id: row.get(3)?,
                    cover_image_path: row.get(4)?,
                    created_at: row.get(5)?,
                    video_count: row.get(6)?,
                })
            })
            .map_err(|e| e.to_string())?
            .filter_map(|r| r.ok())
            .collect();
        Ok(cols)
    }

    pub fn get_collection_by_id(&self, collection_id: &str) -> Result<Option<Collection>, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let mut stmt = conn.prepare(
            "SELECT c.id, c.name, c.description, c.cover_video_id, c.cover_image_path, c.created_at, COUNT(cv.video_id) as video_count
             FROM collections c
             LEFT JOIN collection_videos cv ON c.id = cv.collection_id
             WHERE c.id = ?1
             GROUP BY c.id
             LIMIT 1"
        ).map_err(|e| e.to_string())?;

        match stmt.query_row(params![collection_id], |row| {
            Ok(Collection {
                id: row.get(0)?,
                name: row.get(1)?,
                description: row.get(2)?,
                cover_video_id: row.get(3)?,
                cover_image_path: row.get(4)?,
                created_at: row.get(5)?,
                video_count: row.get(6)?,
            })
        }) {
            Ok(collection) => Ok(Some(collection)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.to_string()),
        }
    }

    pub fn delete_collection(&self, id: &str) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "DELETE FROM collection_videos WHERE collection_id = ?1",
            params![id],
        )
        .map_err(|e| e.to_string())?;
        conn.execute("DELETE FROM collections WHERE id = ?1", params![id])
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn set_collection_cover(
        &self,
        collection_id: &str,
        cover_video_id: Option<&str>,
    ) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "UPDATE collections SET cover_video_id = ?1, cover_image_path = NULL WHERE id = ?2",
            params![cover_video_id, collection_id],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn set_collection_cover_image(
        &self,
        collection_id: &str,
        cover_image_path: Option<&str>,
    ) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "UPDATE collections SET cover_video_id = NULL, cover_image_path = ?1 WHERE id = ?2",
            params![cover_image_path, collection_id],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn add_video_to_collection(
        &self,
        collection_id: &str,
        video_id: &str,
    ) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let pos: i32 = conn.query_row(
            "SELECT COALESCE(MAX(position), -1) + 1 FROM collection_videos WHERE collection_id = ?1",
            params![collection_id],
            |row| row.get(0),
        ).map_err(|e| e.to_string())?;
        conn.execute(
            "INSERT OR IGNORE INTO collection_videos (collection_id, video_id, position) VALUES (?1, ?2, ?3)",
            params![collection_id, video_id, pos],
        ).map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn remove_video_from_collection(
        &self,
        collection_id: &str,
        video_id: &str,
    ) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "DELETE FROM collection_videos WHERE collection_id = ?1 AND video_id = ?2",
            params![collection_id, video_id],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn get_collection_videos(&self, collection_id: &str) -> Result<Vec<VideoRecord>, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let mut stmt = conn.prepare(
            "SELECT v.id, v.library_id, v.path, v.title, v.file_name, v.file_size, v.duration_secs, v.width, v.height, v.thumbnail_path, v.watched, v.watch_progress_secs, v.favorite, v.date_added, v.date_modified
             FROM videos v
             INNER JOIN collection_videos cv ON v.id = cv.video_id
             WHERE cv.collection_id = ?1
             ORDER BY cv.position"
        ).map_err(|e| e.to_string())?;
        let videos = stmt
            .query_map(params![collection_id], |row| {
                Ok(VideoRecord {
                    id: row.get(0)?,
                    library_id: row.get(1)?,
                    path: row.get(2)?,
                    title: row.get(3)?,
                    file_name: row.get(4)?,
                    file_size: row.get(5)?,
                    duration_secs: row.get(6)?,
                    width: row.get(7)?,
                    height: row.get(8)?,
                    thumbnail_path: row.get(9)?,
                    watched: row.get(10)?,
                    watch_progress_secs: row.get(11)?,
                    favorite: row.get(12)?,
                    date_added: row.get(13)?,
                    date_modified: row.get(14)?,
                })
            })
            .map_err(|e| e.to_string())?
            .filter_map(|r| r.ok())
            .collect();
        Ok(videos)
    }

    // ─── Stats ───

    pub fn get_stats(&self) -> Result<LibraryStats, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let total_videos: i64 = conn
            .query_row("SELECT COUNT(*) FROM videos", [], |row| row.get(0))
            .map_err(|e| e.to_string())?;
        let watched_videos: i64 = conn
            .query_row("SELECT COUNT(*) FROM videos WHERE watched = 1", [], |row| {
                row.get(0)
            })
            .map_err(|e| e.to_string())?;
        let total_size: i64 = conn
            .query_row(
                "SELECT COALESCE(SUM(file_size), 0) FROM videos",
                [],
                |row| row.get(0),
            )
            .map_err(|e| e.to_string())?;
        let total_duration: f64 = conn
            .query_row(
                "SELECT COALESCE(SUM(duration_secs), 0) FROM videos",
                [],
                |row| row.get(0),
            )
            .map_err(|e| e.to_string())?;
        let total_libraries: i64 = conn
            .query_row("SELECT COUNT(*) FROM libraries", [], |row| row.get(0))
            .map_err(|e| e.to_string())?;

        Ok(LibraryStats {
            total_videos,
            watched_videos,
            unwatched_videos: total_videos - watched_videos,
            total_size_bytes: total_size,
            total_duration_secs: total_duration,
            total_libraries,
        })
    }
}
