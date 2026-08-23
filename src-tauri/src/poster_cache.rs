use crate::app_paths::AppPaths;
use crate::models::WatchRecord;
use serde::Serialize;
use std::collections::HashMap;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use tokio::sync::{Mutex, Semaphore};

pub const POSTER_MAX_BYTES: u64 = 10 * 1024 * 1024;
pub const CACHE_CAPACITY_BYTES: u64 = 500 * 1024 * 1024;
pub const CACHE_TARGET_BYTES: u64 = 400 * 1024 * 1024;
const TEMP_MAX_AGE: Duration = Duration::from_secs(24 * 60 * 60);

pub struct PosterDownloadState {
    pub permits: Semaphore,
    locks: Mutex<HashMap<String, Arc<Mutex<()>>>>,
}

impl Default for PosterDownloadState {
    fn default() -> Self {
        Self {
            permits: Semaphore::new(4),
            locks: Mutex::new(HashMap::new()),
        }
    }
}

impl PosterDownloadState {
    pub async fn file_lock(&self, file_name: &str) -> Arc<Mutex<()>> {
        let mut locks = self.locks.lock().await;
        locks
            .entry(file_name.to_string())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PosterDownloadResult {
    pub status: String,
    pub file_name: String,
}

#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct PosterCacheStats {
    pub total_bytes: u64,
    pub valid_count: usize,
    pub referenced_count: usize,
    pub orphan_count: usize,
    pub invalid_count: usize,
    pub temporary_count: usize,
    pub capacity_bytes: u64,
    pub capacity_exceeded: bool,
}

pub fn normalized_file_name(poster_path: &str, size: &str) -> Result<String, String> {
    if !matches!(size, "w92" | "w342") {
        return Err("poster_size_invalid".to_string());
    }
    let Some(name) = poster_path.strip_prefix('/') else {
        return Err("poster_path_invalid".to_string());
    };
    if name.is_empty()
        || name.contains(['/', '\\', '?', '#'])
        || name.contains("..")
        || name.contains(':')
    {
        return Err("poster_path_invalid".to_string());
    }
    let Some((stem, extension)) = name.rsplit_once('.') else {
        return Err("poster_path_invalid".to_string());
    };
    if stem.is_empty()
        || !stem
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_'))
        || !matches!(
            extension.to_ascii_lowercase().as_str(),
            "jpg" | "jpeg" | "png" | "webp"
        )
    {
        return Err("poster_path_invalid".to_string());
    }
    Ok(if size == "w92" {
        format!("w92_{name}")
    } else {
        name.to_string()
    })
}

pub fn image_mime(bytes: &[u8]) -> Option<&'static str> {
    let mime = infer::get(bytes)?.mime_type();
    matches!(mime, "image/jpeg" | "image/png" | "image/webp").then_some(mime)
}

pub fn read_valid(path: &Path) -> Result<(Vec<u8>, &'static str), String> {
    let metadata = fs::metadata(path).map_err(|_| "poster_cache_read_failed".to_string())?;
    if !metadata.is_file() || metadata.len() == 0 || metadata.len() > POSTER_MAX_BYTES {
        return Err("poster_signature_invalid".to_string());
    }
    let bytes = fs::read(path).map_err(|_| "poster_cache_read_failed".to_string())?;
    let mime = image_mime(&bytes).ok_or_else(|| "poster_signature_invalid".to_string())?;
    Ok((bytes, mime))
}

pub fn referenced_file_names(records: &[WatchRecord]) -> HashSet<String> {
    let mut names = HashSet::new();
    for record in records {
        let Some(path) = record.poster_path.as_deref() else {
            continue;
        };
        for size in ["w342", "w92"] {
            if let Ok(name) = normalized_file_name(path, size) {
                names.insert(name);
            }
        }
    }
    names
}

fn is_temporary(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with(".poster-") && name.ends_with(".part"))
}

fn is_stale_temporary(path: &Path) -> bool {
    is_temporary(path)
        && fs::metadata(path)
            .ok()
            .and_then(|metadata| metadata.modified().ok())
            .and_then(|modified| SystemTime::now().duration_since(modified).ok())
            .is_some_and(|age| age >= TEMP_MAX_AGE)
}

pub fn cleanup_stale_temporary_files(paths: &AppPaths) -> Result<usize, String> {
    let mut removed = 0;
    for entry in fs::read_dir(paths.posters()).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let path = entry.path();
        if !is_temporary(&path) {
            continue;
        }
        if is_stale_temporary(&path) && fs::remove_file(path).is_ok() {
            removed += 1;
        }
    }
    Ok(removed)
}

pub fn stats(paths: &AppPaths, records: &[WatchRecord]) -> Result<PosterCacheStats, String> {
    let referenced = referenced_file_names(records);
    let mut result = PosterCacheStats {
        capacity_bytes: CACHE_CAPACITY_BYTES,
        ..PosterCacheStats::default()
    };
    for entry in fs::read_dir(paths.posters()).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let size = entry.metadata().map(|value| value.len()).unwrap_or(0);
        result.total_bytes = result.total_bytes.saturating_add(size);
        if is_temporary(&path) {
            result.temporary_count += 1;
            continue;
        }
        let file_name = entry.file_name().to_string_lossy().to_string();
        if read_valid(&path).is_err() {
            result.invalid_count += 1;
        } else {
            result.valid_count += 1;
            if referenced.contains(&file_name) {
                result.referenced_count += 1;
            } else {
                result.orphan_count += 1;
            }
        }
    }
    result.capacity_exceeded = result.total_bytes > CACHE_CAPACITY_BYTES;
    Ok(result)
}

pub fn clean(
    paths: &AppPaths,
    records: &[WatchRecord],
    mode: &str,
) -> Result<PosterCacheStats, String> {
    if !matches!(mode, "unreferenced" | "all") {
        return Err("poster_cache_mode_invalid".to_string());
    }
    let referenced = referenced_file_names(records);
    for entry in fs::read_dir(paths.posters()).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        let remove = if is_temporary(&path) {
            is_stale_temporary(&path)
        } else {
            mode == "all" || !referenced.contains(&name)
        };
        if remove {
            fs::remove_file(path).map_err(|error| error.to_string())?;
        }
    }
    stats(paths, records)
}

pub fn enforce_capacity(
    paths: &AppPaths,
    records: &[WatchRecord],
) -> Result<PosterCacheStats, String> {
    let referenced = referenced_file_names(records);
    let current = stats(paths, records)?;
    if current.total_bytes <= CACHE_CAPACITY_BYTES {
        return Ok(current);
    }
    let mut candidates: Vec<(SystemTime, PathBuf, u64)> = fs::read_dir(paths.posters())
        .map_err(|error| error.to_string())?
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();
            if !path.is_file() || referenced.contains(&name) || is_temporary(&path) {
                return None;
            }
            let metadata = entry.metadata().ok()?;
            Some((
                metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH),
                path,
                metadata.len(),
            ))
        })
        .collect();
    candidates.sort_by_key(|candidate| candidate.0);
    let mut total = current.total_bytes;
    for (_, path, size) in candidates {
        if total <= CACHE_TARGET_BYTES {
            break;
        }
        if fs::remove_file(path).is_ok() {
            total = total.saturating_sub(size);
        }
    }
    stats(paths, records)
}

#[cfg(test)]
mod tests {
    use super::{clean, image_mime, normalized_file_name, stats};
    use crate::app_paths::AppPaths;
    use crate::models::{RecordStatus, WatchRecord};
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn record(poster_path: &str) -> WatchRecord {
        WatchRecord {
            id: "record-1".to_string(),
            original_name: "Title".to_string(),
            chinese_name: String::new(),
            progress: String::new(),
            total_episodes: None,
            episode_tracking_enabled: false,
            next_episode: None,
            movie_progress: None,
            movie_duration: None,
            release_year: None,
            poster_path: Some(poster_path.to_string()),
            status: RecordStatus::Unwatched,
            platform: String::new(),
            rating: None,
            start_date: None,
            end_date: None,
            notes: String::new(),
            created_at: String::new(),
            updated_at: None,
            imdb_id: None,
            is_locked: None,
            genres: None,
            origin_country: None,
            imdb_rating: None,
            tmdb_status: None,
            interest_level: None,
            episode_runtime: None,
            media_type: "电影".to_string(),
            content_tags: None,
            tmdb_media_kind: None,
            tmdb_id: None,
            tmdb_parent_id: None,
            tmdb_season_number: None,
            series_record_kind: None,
            rev: 0,
            rev_actor: String::new(),
        }
    }

    fn test_paths() -> AppPaths {
        let id = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        AppPaths::resolve_from(
            None,
            &std::env::temp_dir().join(format!("watchtracker-poster-cache-{id}")),
        )
        .unwrap()
    }

    #[test]
    fn poster_path_is_strict_and_size_namespaces_are_separate() {
        assert_eq!(
            normalized_file_name("/abc-_123.jpg", "w342").unwrap(),
            "abc-_123.jpg"
        );
        assert_eq!(
            normalized_file_name("/abc.jpg", "w92").unwrap(),
            "w92_abc.jpg"
        );
        assert_eq!(
            normalized_file_name("/2baf1e.jpg", "w92").unwrap(),
            "w92_2baf1e.jpg"
        );
        for value in [
            "abc.jpg",
            "/nested/a.jpg",
            "/../a.jpg",
            "/a.svg",
            "https://x/a.jpg",
        ] {
            assert!(normalized_file_name(value, "w342").is_err(), "{value}");
        }
    }

    #[test]
    fn image_signature_allows_only_supported_formats() {
        assert_eq!(
            image_mime(&[0xff, 0xd8, 0xff, 0xe0, 0, 0]),
            Some("image/jpeg")
        );
        assert_eq!(image_mime(b"not an image"), None);
    }

    #[test]
    fn automatic_style_cleanup_preserves_referenced_posters() {
        let paths = test_paths();
        let jpeg = [0xff, 0xd8, 0xff, 0xe0, 0, 0];
        fs::write(paths.posters().join("kept.jpg"), jpeg).unwrap();
        fs::write(paths.posters().join("orphan.jpg"), jpeg).unwrap();
        fs::write(paths.posters().join("w92_kept.jpg"), jpeg).unwrap();
        fs::write(paths.posters().join("w92_orphan.jpg"), jpeg).unwrap();
        let records = [record("/kept.jpg")];

        let before = stats(&paths, &records).unwrap();
        assert_eq!(before.referenced_count, 2);
        assert_eq!(before.orphan_count, 2);
        let after = clean(&paths, &records, "unreferenced").unwrap();
        assert!(paths.posters().join("kept.jpg").is_file());
        assert!(paths.posters().join("w92_kept.jpg").is_file());
        assert!(!paths.posters().join("orphan.jpg").exists());
        assert!(!paths.posters().join("w92_orphan.jpg").exists());
        assert_eq!(after.referenced_count, 2);

        clean(&paths, &records, "all").unwrap();
        assert!(!paths.posters().join("kept.jpg").exists());
        assert!(!paths.posters().join("w92_kept.jpg").exists());
        let _ = fs::remove_dir_all(paths.root());
    }
}
