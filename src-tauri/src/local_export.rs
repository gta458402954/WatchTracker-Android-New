use crate::app_paths::AppPaths;
use crate::error::AppError;
use crate::models::WatchRecord;
use rusqlite::Connection;
use serde::Serialize;
use serde_json::Value;
use std::fs::{self, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use uuid::Uuid;

const STAGING_DIRECTORY: &str = "export-staging";
const MAX_EXPORT_BYTES: usize = 128 * 1024 * 1024;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalExportSnapshot {
    pub records: Vec<WatchRecord>,
    pub episode_completions: Vec<crate::episode_history::EpisodeCompletion>,
    pub collections: Vec<crate::collections::Collection>,
    pub collection_members: Vec<crate::collections::CollectionMember>,
}

/// Reads every exported entity while the caller holds the application's one
/// database mutex. Unlike the sync snapshot, this does not initialize or
/// update settings, generation, outbox, staging, or device identity state.
pub fn snapshot(conn: &Connection) -> Result<LocalExportSnapshot, AppError> {
    Ok(LocalExportSnapshot {
        records: crate::db::get_all_records(conn)?,
        episode_completions: crate::episode_history::all_completions(conn)?,
        collections: crate::collections::all(conn)?,
        collection_members: crate::collections::all_members(conn)?,
    })
}

fn general(message: impl Into<String>) -> AppError {
    AppError::General(message.into())
}

fn staging_directory(paths: &AppPaths) -> PathBuf {
    paths.root().join(STAGING_DIRECTORY)
}

fn validated_token(token: &str) -> Result<Uuid, AppError> {
    let parsed = Uuid::parse_str(token).map_err(|_| general("invalid_export_stage_token"))?;
    if parsed.to_string() != token {
        return Err(general("invalid_export_stage_token"));
    }
    Ok(parsed)
}

fn stage_path(paths: &AppPaths, token: &str) -> Result<PathBuf, AppError> {
    let token = validated_token(token)?;
    Ok(staging_directory(paths).join(format!("{token}.json")))
}

fn valid_export_envelope(value: &Value) -> bool {
    let Some(object) = value.as_object() else {
        return false;
    };
    let allowed = [
        "formatVersion",
        "exportedAt",
        "records",
        "episodeCompletions",
        "collections",
        "collectionMembers",
    ];
    object.len() == allowed.len()
        && object.keys().all(|key| allowed.contains(&key.as_str()))
        && object.get("formatVersion").and_then(Value::as_u64) == Some(4)
        && object.get("exportedAt").and_then(Value::as_str).is_some()
        && [
            "records",
            "episodeCompletions",
            "collections",
            "collectionMembers",
        ]
        .iter()
        .all(|key| object.get(*key).is_some_and(Value::is_array))
}

pub fn stage(paths: &AppPaths, json: &str) -> Result<String, AppError> {
    if json.is_empty() || json.len() > MAX_EXPORT_BYTES {
        return Err(general("invalid_export_size"));
    }
    let value: Value = serde_json::from_str(json).map_err(|_| general("invalid_export_json"))?;
    if !valid_export_envelope(&value) {
        return Err(general("invalid_export_envelope"));
    }

    let directory = staging_directory(paths);
    fs::create_dir_all(&directory)?;
    let token = Uuid::new_v4().to_string();
    let target = stage_path(paths, &token)?;
    let temporary = directory.join(format!("{token}.tmp"));
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)?;
    let mut writer = BufWriter::new(file);
    if let Err(error) = (|| -> std::io::Result<()> {
        writer.write_all(json.as_bytes())?;
        writer.flush()?;
        writer.get_ref().sync_all()
    })() {
        drop(writer);
        let _ = fs::remove_file(&temporary);
        return Err(error.into());
    }
    drop(writer);
    if let Err(error) = fs::rename(&temporary, &target) {
        let _ = fs::remove_file(&temporary);
        return Err(error.into());
    }
    Ok(token)
}

pub fn discard(paths: &AppPaths, token: &str) -> Result<(), AppError> {
    let path = stage_path(paths, token)?;
    if path.exists() {
        fs::remove_file(path)?;
    }
    Ok(())
}

fn is_stale_export_file(path: &Path) -> bool {
    if !path.is_file() {
        return false;
    }
    let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
        return false;
    };
    let token = name
        .strip_suffix(".json")
        .or_else(|| name.strip_suffix(".tmp"));
    token
        .is_some_and(|value| Uuid::parse_str(value).is_ok_and(|parsed| parsed.to_string() == value))
}

pub fn cleanup_stale(paths: &AppPaths) -> Result<(), AppError> {
    let directory = staging_directory(paths);
    if !directory.exists() {
        return Ok(());
    }
    for entry in fs::read_dir(directory)? {
        let path = entry?.path();
        if is_stale_export_file(&path) {
            fs::remove_file(path)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app_paths::AppPaths;
    use rusqlite::Connection;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_TEST_ID: AtomicU64 = AtomicU64::new(1);

    struct TestRoot(PathBuf);

    impl TestRoot {
        fn new(name: &str) -> (Self, AppPaths) {
            let id = NEXT_TEST_ID.fetch_add(1, Ordering::Relaxed);
            let root = std::env::temp_dir().join(format!(
                "watchtracker-local-export-{}-{name}-{id}",
                std::process::id()
            ));
            let paths = AppPaths::resolve_from(None, &root.join("app-data"))
                .expect("resolve export test paths");
            (Self(root), paths)
        }
    }

    impl Drop for TestRoot {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn settings(conn: &Connection) -> Vec<(String, String)> {
        let mut statement = conn
            .prepare("SELECT key, value FROM settings ORDER BY key")
            .expect("prepare settings query");
        statement
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .expect("query settings")
            .collect::<rusqlite::Result<_>>()
            .expect("collect settings")
    }

    #[test]
    fn snapshot_reads_all_entities_without_initializing_sync_state() {
        let conn = Connection::open_in_memory().expect("open database");
        crate::db::setup_db(&conn).expect("set up database");
        conn.execute(
            "INSERT INTO records (id,originalName,chineseName,progress,status,platform,startDate,endDate,notes,createdAt,mediaType,rev,revActor) VALUES ('record','Original','记录','', '未看','','','','','2026-08-29T00:00:00Z','电影',1,'device')",
            [],
        ).expect("insert record");
        conn.execute(
            "INSERT INTO episode_completions (id,recordId,episodeNumber,completedAt,createdAt,updatedAt,rev,revActor) VALUES ('completion','record',1,'2026-08-29T01:00:00Z','2026-08-29T01:00:00Z','2026-08-29T01:00:00Z',1,'device')",
            [],
        ).expect("insert completion");
        conn.execute(
            "INSERT INTO collections (id,name,normalizedName,description,sourceKind,sourceKey,collectionKind,orderMode,createdAt,updatedAt,rev,revActor) VALUES ('collection','收藏','收藏',NULL,'manual',NULL,'manual','manual','2026-08-29T00:00:00Z','2026-08-29T00:00:00Z',1,'device')",
            [],
        ).expect("insert collection");
        conn.execute(
            "INSERT INTO collection_members (id,collectionId,recordId,position,sourceKind,createdAt,updatedAt,rev,revActor) VALUES ('member','collection','record',0,'manual','2026-08-29T00:00:00Z','2026-08-29T00:00:00Z',1,'device')",
            [],
        ).expect("insert member");
        let before = settings(&conn);

        let result = snapshot(&conn).expect("read export snapshot");

        assert_eq!(result.records.len(), 1);
        assert_eq!(result.episode_completions.len(), 1);
        assert_eq!(result.collections.len(), 1);
        assert_eq!(result.collection_members.len(), 1);
        assert_eq!(settings(&conn), before);
        assert!(!before.iter().any(|(key, _)| {
            matches!(
                key.as_str(),
                "sync_device_id_v1" | "sync_outbox_v1" | "sync_staging_v1"
            )
        }));
    }

    #[test]
    fn staging_accepts_only_v4_export_envelopes_and_cleans_safe_files() {
        let (_root, paths) = TestRoot::new("staging");
        let json = r#"{"formatVersion":4,"exportedAt":"2026-08-29T00:00:00.000Z","records":[],"episodeCompletions":[],"collections":[],"collectionMembers":[]}"#;
        let token = stage(&paths, json).expect("stage valid export");
        let path = stage_path(&paths, &token).expect("resolve stage file");
        assert_eq!(fs::read_to_string(&path).unwrap(), json);
        fs::write(staging_directory(&paths).join("unrelated.txt"), "keep").unwrap();

        cleanup_stale(&paths).expect("clean stale export files");

        assert!(!path.exists());
        assert!(staging_directory(&paths).join("unrelated.txt").exists());
        assert!(stage(&paths, r#"{"formatVersion":3}"#).is_err());
        assert!(stage(
            &paths,
            r#"{"formatVersion":4,"exportedAt":"2026-08-29T00:00:00.000Z","records":[],"episodeCompletions":[],"collections":[],"collectionMembers":[],"password":"secret"}"#,
        )
        .is_err());
        assert!(discard(&paths, "../outside").is_err());
    }
}
