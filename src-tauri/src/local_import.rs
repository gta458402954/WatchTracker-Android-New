use crate::app_paths::AppPaths;
use crate::error::AppError;
use crate::models::{WatchRecord, MEDIA_TYPE_VALUES};
use chrono::DateTime;
use rusqlite::{backup::Backup, Connection};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;
use uuid::Uuid;

const STAGING_DIRECTORY: &str = "import-staging";
pub const MAX_IMPORT_BYTES: u64 = 128 * 1024 * 1024;

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct LocalBackupV4 {
    format_version: u32,
    exported_at: String,
    records: Vec<WatchRecord>,
    episode_completions: Vec<crate::episode_history::EpisodeCompletion>,
    collections: Vec<crate::collections::Collection>,
    collection_members: Vec<crate::collections::CollectionMember>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportEntityCounts {
    pub records: usize,
    pub episode_completions: usize,
    pub collections: usize,
    pub collection_members: usize,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportRecordDiff {
    pub added: usize,
    pub updated: usize,
    pub removed: usize,
    pub unchanged: usize,
    pub locked_preserved: usize,
    pub final_count: usize,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalImportPreview {
    pub stage_token: String,
    pub stage_sha256: String,
    pub current_library_fingerprint: String,
    pub file_name: String,
    pub size_bytes: u64,
    pub format_version: u32,
    pub exported_at: String,
    pub counts: ImportEntityCounts,
    pub records: ImportRecordDiff,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalImportResult {
    pub recovery_point_id: String,
    pub record_count: usize,
    pub episode_completion_count: usize,
    pub collection_count: usize,
    pub collection_member_count: usize,
    pub locked_preserved_count: usize,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct LibraryFingerprint<'a> {
    records: &'a [WatchRecord],
    episode_completions: &'a [crate::episode_history::EpisodeCompletion],
    collections: &'a [crate::collections::Collection],
    collection_members: &'a [crate::collections::CollectionMember],
}

fn error(code: &str) -> AppError {
    AppError::General(code.to_string())
}

fn staging_directory(paths: &AppPaths) -> PathBuf {
    paths.root().join(STAGING_DIRECTORY)
}

fn validated_token(token: &str) -> Result<Uuid, AppError> {
    let parsed = Uuid::parse_str(token).map_err(|_| error("invalid_import_stage_token"))?;
    if parsed.to_string() != token {
        return Err(error("invalid_import_stage_token"));
    }
    Ok(parsed)
}

fn stage_path(paths: &AppPaths, token: &str) -> Result<PathBuf, AppError> {
    let token = validated_token(token)?;
    let directory = staging_directory(paths);
    fs::create_dir_all(&directory)?;
    let canonical_directory = directory.canonicalize()?;
    let candidate = canonical_directory.join(format!("{token}.json"));
    if candidate.parent() != Some(canonical_directory.as_path()) {
        return Err(error("invalid_import_stage_token"));
    }
    Ok(candidate)
}

fn safe_file_name(file_name: &str) -> Result<String, AppError> {
    let trimmed = file_name.trim();
    if trimmed.is_empty()
        || trimmed.len() > 256
        || trimmed.chars().any(char::is_control)
        || trimmed.contains(['/', '\\'])
    {
        return Err(error("invalid_import_file_name"));
    }
    Ok(trimmed.to_string())
}

fn read_stage(paths: &AppPaths, token: &str) -> Result<(Vec<u8>, String), AppError> {
    let path = stage_path(paths, token)?;
    let metadata = fs::metadata(&path).map_err(|_| error("import_stage_missing"))?;
    if !metadata.is_file() {
        return Err(error("import_stage_missing"));
    }
    if metadata.len() == 0 {
        return Err(error("invalid_json"));
    }
    if metadata.len() > MAX_IMPORT_BYTES {
        return Err(error("import_file_too_large"));
    }
    let mut input = fs::File::open(path)?;
    let mut bytes = Vec::with_capacity(metadata.len().min(MAX_IMPORT_BYTES) as usize);
    input
        .by_ref()
        .take(MAX_IMPORT_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_IMPORT_BYTES {
        return Err(error("import_file_too_large"));
    }
    let sha256 = format!("{:x}", Sha256::digest(&bytes));
    Ok((bytes, sha256))
}

fn exact_v4_envelope(value: &Value) -> Result<(), AppError> {
    let object = value
        .as_object()
        .ok_or_else(|| error("invalid_backup_format"))?;
    let version = object
        .get("formatVersion")
        .and_then(Value::as_u64)
        .ok_or_else(|| error("missing_format_version"))?;
    if version > 4 {
        return Err(error("future_backup_version"));
    }
    if version != 4 {
        return Err(error("unsupported_backup_version"));
    }
    const FIELDS: [&str; 6] = [
        "formatVersion",
        "exportedAt",
        "records",
        "episodeCompletions",
        "collections",
        "collectionMembers",
    ];
    if object.len() != FIELDS.len()
        || object.keys().any(|key| !FIELDS.contains(&key.as_str()))
        || FIELDS.iter().any(|key| !object.contains_key(*key))
    {
        return Err(error("unknown_backup_field"));
    }
    let exported_at = object
        .get("exportedAt")
        .and_then(Value::as_str)
        .ok_or_else(|| error("invalid_backup_metadata"))?;
    DateTime::parse_from_rfc3339(exported_at).map_err(|_| error("invalid_backup_metadata"))?;
    for field in [
        "records",
        "episodeCompletions",
        "collections",
        "collectionMembers",
    ] {
        if !object.get(field).is_some_and(Value::is_array) {
            return Err(error("invalid_backup_format"));
        }
    }
    Ok(())
}

fn parse_backup(bytes: &[u8]) -> Result<LocalBackupV4, AppError> {
    let value: Value = serde_json::from_slice(bytes).map_err(|_| error("invalid_json"))?;
    exact_v4_envelope(&value)?;
    let object = value.as_object().expect("validated object");
    let records: Vec<WatchRecord> =
        serde_json::from_value(object["records"].clone()).map_err(|_| error("invalid_records"))?;
    let episode_completions = serde_json::from_value(object["episodeCompletions"].clone())
        .map_err(|_| error("invalid_episode_history"))?;
    let collections = serde_json::from_value(object["collections"].clone())
        .map_err(|_| error("invalid_collections"))?;
    let collection_members = serde_json::from_value(object["collectionMembers"].clone())
        .map_err(|_| error("invalid_collections"))?;
    let backup = LocalBackupV4 {
        format_version: 4,
        exported_at: object["exportedAt"]
            .as_str()
            .expect("validated timestamp")
            .to_string(),
        records,
        episode_completions,
        collections,
        collection_members,
    };
    validate_domain(&backup)?;
    Ok(backup)
}

fn validate_domain(backup: &LocalBackupV4) -> Result<(), AppError> {
    let valid_timestamp = |value: &str| DateTime::parse_from_rfc3339(value).is_ok();
    let mut record_ids = HashSet::new();
    for record in &backup.records {
        if record.id.trim().is_empty()
            || !record_ids.insert(record.id.clone())
            || (record.original_name.trim().is_empty() && record.chinese_name.trim().is_empty())
            || !MEDIA_TYPE_VALUES.contains(&record.media_type.as_str())
            || record.total_episodes.is_some_and(|value| value <= 0)
            || record.movie_progress.is_some_and(|value| value < 0)
            || record.movie_duration.is_some_and(|value| value <= 0)
            || record
                .rating
                .is_some_and(|value| !(1..=10).contains(&value))
            || record
                .interest_level
                .is_some_and(|value| !(1..=5).contains(&value))
            || record.episode_runtime.is_some_and(|value| value <= 0)
            || record
                .imdb_rating
                .is_some_and(|value| !value.is_finite() || !(0.0..=10.0).contains(&value))
            || record.rev < 0
            || !valid_timestamp(&record.created_at)
            || record
                .updated_at
                .as_deref()
                .is_some_and(|value| !valid_timestamp(value))
        {
            return Err(error("invalid_records"));
        }
    }
    crate::record_validation::prepare_import_batch(backup.records.clone())
        .map_err(|_| error("invalid_records"))?;

    let mut completion_ids = HashSet::new();
    if backup.episode_completions.iter().any(|item| {
        item.id.trim().is_empty()
            || item.record_id.trim().is_empty()
            || !completion_ids.insert(item.id.clone())
            || item.rev < 0
            || item.created_at.trim().is_empty()
            || item.updated_at.trim().is_empty()
            || !valid_timestamp(&item.created_at)
            || !valid_timestamp(&item.updated_at)
            || !record_ids.contains(&item.record_id)
    }) {
        return Err(error("invalid_episode_history"));
    }

    let mut collection_ids = HashSet::new();
    if backup.collections.iter().any(|item| {
        item.id.trim().is_empty()
            || item.name.trim().is_empty()
            || !collection_ids.insert(item.id.clone())
            || item.rev < 0
            || item.created_at.trim().is_empty()
            || item.updated_at.trim().is_empty()
            || !valid_timestamp(&item.created_at)
            || !valid_timestamp(&item.updated_at)
    }) {
        return Err(error("invalid_collections"));
    }
    let mut member_ids = HashSet::new();
    if backup.collection_members.iter().any(|item| {
        item.id.trim().is_empty()
            || !member_ids.insert(item.id.clone())
            || item.rev < 0
            || item.position < 0
            || item.created_at.trim().is_empty()
            || item.updated_at.trim().is_empty()
            || !valid_timestamp(&item.created_at)
            || !valid_timestamp(&item.updated_at)
            || !matches!(item.source_kind.as_str(), "manual" | "tmdb")
            || !record_ids.contains(&item.record_id)
            || !collection_ids.contains(&item.collection_id)
    }) {
        return Err(error("invalid_collections"));
    }
    Ok(())
}

fn snapshot(conn: &Connection) -> Result<crate::local_export::LocalExportSnapshot, AppError> {
    crate::local_export::snapshot(conn)
}

fn canonical_snapshot(
    mut value: crate::local_export::LocalExportSnapshot,
) -> crate::local_export::LocalExportSnapshot {
    value.records.sort_by(|left, right| left.id.cmp(&right.id));
    value
        .episode_completions
        .sort_by(|left, right| left.id.cmp(&right.id));
    value
        .collections
        .sort_by(|left, right| left.id.cmp(&right.id));
    value
        .collection_members
        .sort_by(|left, right| left.id.cmp(&right.id));
    value
}

fn fingerprint(conn: &Connection) -> Result<String, AppError> {
    let value = canonical_snapshot(snapshot(conn)?);
    let payload = LibraryFingerprint {
        records: &value.records,
        episode_completions: &value.episode_completions,
        collections: &value.collections,
        collection_members: &value.collection_members,
    };
    let bytes = serde_json::to_vec(&payload).map_err(|_| error("import_fingerprint_failed"))?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

fn copy_database(source: &Connection) -> Result<Connection, AppError> {
    let mut destination = Connection::open_in_memory()?;
    {
        let backup = Backup::new(source, &mut destination)?;
        backup.run_to_completion(64, Duration::from_millis(0), None)?;
    }
    destination.pragma_update(None, "foreign_keys", "ON")?;
    Ok(destination)
}

fn replacement_error(error_value: AppError) -> AppError {
    let message = error_value.to_string();
    if message.contains("episode") {
        error("invalid_episode_history")
    } else if message.contains("collection") || message.contains("UNIQUE constraint") {
        error("invalid_collections")
    } else {
        error("invalid_records")
    }
}

fn simulate(
    conn: &Connection,
    backup: &LocalBackupV4,
) -> Result<crate::local_export::LocalExportSnapshot, AppError> {
    let mut simulation = copy_database(conn)?;
    crate::collections::replace_library_atomic(
        &mut simulation,
        backup.records.clone(),
        backup.episode_completions.clone(),
        backup.collections.clone(),
        backup.collection_members.clone(),
    )
    .map_err(replacement_error)?;
    snapshot(&simulation)
}

fn record_map(records: &[WatchRecord]) -> Result<BTreeMap<String, Value>, AppError> {
    records
        .iter()
        .map(|record| {
            serde_json::to_value(record)
                .map(|value| (record.id.clone(), value))
                .map_err(|_| error("import_preview_failed"))
        })
        .collect()
}

fn diff_records(
    current: &[WatchRecord],
    final_records: &[WatchRecord],
    imported: &[WatchRecord],
    current_completions: &[crate::episode_history::EpisodeCompletion],
    imported_completions: &[crate::episode_history::EpisodeCompletion],
) -> Result<ImportRecordDiff, AppError> {
    let before = record_map(current)?;
    let after = record_map(final_records)?;
    let incoming = record_map(imported)?;
    let locked = current
        .iter()
        .filter(|item| item.is_locked.unwrap_or(false))
        .map(|item| item.id.as_str())
        .collect::<HashSet<_>>();
    let added = after.keys().filter(|id| !before.contains_key(*id)).count();
    let removed = before
        .keys()
        .filter(|id| !after.contains_key(*id) && !locked.contains(id.as_str()))
        .count();
    let mut updated = 0;
    let mut unchanged = 0;
    for (id, before_value) in &before {
        if locked.contains(id.as_str()) {
            continue;
        }
        if let Some(after_value) = after.get(id) {
            if before_value == after_value {
                unchanged += 1;
            } else {
                updated += 1;
            }
        }
    }
    let completion_values = |items: &[crate::episode_history::EpisodeCompletion], id: &str| {
        let mut values = items
            .iter()
            .filter(|item| item.record_id == id)
            .map(|item| serde_json::to_value(item).map_err(|_| error("import_preview_failed")))
            .collect::<Result<Vec<_>, _>>()?;
        values.sort_by_key(Value::to_string);
        Ok::<_, AppError>(values)
    };
    let locked_preserved = current
        .iter()
        .filter(|item| item.is_locked.unwrap_or(false))
        .map(|item| {
            Ok(incoming.get(&item.id) != before.get(&item.id)
                || completion_values(current_completions, &item.id)?
                    != completion_values(imported_completions, &item.id)?)
        })
        .collect::<Result<Vec<_>, AppError>>()?
        .into_iter()
        .filter(|protected| *protected)
        .count();
    Ok(ImportRecordDiff {
        added,
        updated,
        removed,
        unchanged,
        locked_preserved,
        final_count: after.len(),
    })
}

pub fn preview(
    conn: &Connection,
    paths: &AppPaths,
    token: &str,
    file_name: &str,
) -> Result<LocalImportPreview, AppError> {
    let token = validated_token(token)?.to_string();
    let result = (|| {
        let file_name = safe_file_name(file_name)?;
        let (bytes, stage_sha256) = read_stage(paths, &token)?;
        let backup = parse_backup(&bytes)?;
        let current = snapshot(conn)?;
        let final_state = simulate(conn, &backup)?;
        let records = diff_records(
            &current.records,
            &final_state.records,
            &backup.records,
            &current.episode_completions,
            &backup.episode_completions,
        )?;
        Ok(LocalImportPreview {
            stage_token: token.clone(),
            stage_sha256,
            current_library_fingerprint: fingerprint(conn)?,
            file_name,
            size_bytes: bytes.len() as u64,
            format_version: backup.format_version,
            exported_at: backup.exported_at,
            counts: ImportEntityCounts {
                records: backup.records.len(),
                episode_completions: backup.episode_completions.len(),
                collections: backup.collections.len(),
                collection_members: backup.collection_members.len(),
            },
            records,
        })
    })();
    if result.is_err() {
        let _ = discard(paths, &token);
    }
    result
}

pub fn commit(
    conn: &mut Connection,
    paths: &AppPaths,
    token: &str,
    expected_stage_sha256: &str,
    expected_library_fingerprint: &str,
) -> Result<LocalImportResult, AppError> {
    let token = validated_token(token)?.to_string();
    let (bytes, stage_sha256) = match read_stage(paths, &token) {
        Ok(value) => value,
        Err(value) => {
            let _ = discard(paths, &token);
            return Err(value);
        }
    };
    if stage_sha256 != expected_stage_sha256 {
        return Err(error("import_stage_changed"));
    }
    let backup = match parse_backup(&bytes) {
        Ok(value) => value,
        Err(value) => {
            let _ = discard(paths, &token);
            return Err(value);
        }
    };
    if fingerprint(conn)? != expected_library_fingerprint {
        return Err(error("import_preview_stale"));
    }
    let before = snapshot(conn)?;
    let simulated = match simulate(conn, &backup) {
        Ok(value) => value,
        Err(value) => {
            let _ = discard(paths, &token);
            return Err(value);
        }
    };
    let record_diff = diff_records(
        &before.records,
        &simulated.records,
        &backup.records,
        &before.episode_completions,
        &backup.episode_completions,
    )?;
    let recovery = crate::recovery_points::create(conn, paths, "import")?;
    crate::collections::replace_library_atomic(
        conn,
        backup.records,
        backup.episode_completions,
        backup.collections,
        backup.collection_members,
    )
    .map_err(replacement_error)?;
    let final_state = snapshot(conn)?;
    if let Err(remove_error) = discard(paths, &token) {
        log::warn!("Could not remove consumed local-import stage: {remove_error}");
    }
    Ok(LocalImportResult {
        recovery_point_id: recovery.id,
        record_count: final_state.records.len(),
        episode_completion_count: final_state.episode_completions.len(),
        collection_count: final_state.collections.len(),
        collection_member_count: final_state.collection_members.len(),
        locked_preserved_count: record_diff.locked_preserved,
    })
}

pub fn discard(paths: &AppPaths, token: &str) -> Result<(), AppError> {
    let path = stage_path(paths, token)?;
    if path.exists() {
        fs::remove_file(path)?;
    }
    Ok(())
}

fn is_stale_import_file(path: &Path) -> bool {
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
        if is_stale_import_file(&path) {
            fs::remove_file(path)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;
    use serde_json::{json, Value};
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_TEST_ID: AtomicU64 = AtomicU64::new(1);

    struct TestRoot(PathBuf);

    impl TestRoot {
        fn new(name: &str) -> (Self, AppPaths) {
            let id = NEXT_TEST_ID.fetch_add(1, Ordering::Relaxed);
            let root = std::env::temp_dir().join(format!(
                "watchtracker-local-import-{}-{name}-{id}",
                std::process::id()
            ));
            let paths = AppPaths::resolve_from(None, &root.join("app-data"))
                .expect("resolve import test paths");
            (Self(root), paths)
        }
    }

    impl Drop for TestRoot {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn database(paths: &AppPaths) -> Connection {
        let conn = Connection::open(paths.database()).expect("open test database");
        conn.pragma_update(None, "foreign_keys", "ON").unwrap();
        crate::db::setup_db(&conn).expect("set up test database");
        conn
    }

    fn record(id: &str, title: &str, locked: bool) -> WatchRecord {
        serde_json::from_value(json!({
            "id": id, "originalName": "", "chineseName": title, "progress": "",
            "totalEpisodes": 3, "episodeTrackingEnabled": true, "nextEpisode": 2,
            "status": "在看", "platform": "", "startDate": "2026-08-30",
            "endDate": "", "notes": "", "createdAt": "2026-08-30T00:00:00Z",
            "isLocked": locked, "mediaType": "剧集", "rev": 1, "revActor": "fixture"
        }))
        .unwrap()
    }

    fn completion(record_id: &str, episode: i32) -> crate::episode_history::EpisodeCompletion {
        let mut digest = Sha256::new();
        digest.update(b"episode-completion:v1\0");
        digest.update(record_id.as_bytes());
        digest.update(b"\0");
        digest.update(episode.to_string().as_bytes());
        crate::episode_history::EpisodeCompletion {
            id: format!("{:x}", digest.finalize()),
            record_id: record_id.to_string(),
            episode_number: episode,
            completed_at: Some("2026-08-30T01:00:00Z".into()),
            created_at: "2026-08-30T01:00:00Z".into(),
            updated_at: "2026-08-30T01:00:00Z".into(),
            rev: 1,
            rev_actor: "fixture".into(),
        }
    }

    fn collection(id: &str) -> crate::collections::Collection {
        crate::collections::Collection {
            id: id.into(),
            name: "测试收藏".into(),
            normalized_name: "测试收藏".into(),
            description: None,
            source_kind: "manual".into(),
            source_key: None,
            collection_kind: "manual".into(),
            order_mode: "manual".into(),
            created_at: "2026-08-30T00:00:00Z".into(),
            updated_at: "2026-08-30T00:00:00Z".into(),
            rev: 1,
            rev_actor: "fixture".into(),
        }
    }

    fn member(collection_id: &str, record_id: &str) -> crate::collections::CollectionMember {
        crate::collections::CollectionMember {
            id: crate::collections::member_id(collection_id, record_id),
            collection_id: collection_id.into(),
            record_id: record_id.into(),
            position: 0,
            source_kind: "manual".into(),
            created_at: "2026-08-30T00:00:00Z".into(),
            updated_at: "2026-08-30T00:00:00Z".into(),
            rev: 1,
            rev_actor: "fixture".into(),
        }
    }

    fn envelope(
        records: Vec<WatchRecord>,
        completions: Vec<crate::episode_history::EpisodeCompletion>,
        collections: Vec<crate::collections::Collection>,
        members: Vec<crate::collections::CollectionMember>,
    ) -> Value {
        json!({
            "formatVersion": 4,
            "exportedAt": "2026-08-30T00:00:00.000Z",
            "records": records,
            "episodeCompletions": completions,
            "collections": collections,
            "collectionMembers": members
        })
    }

    fn stage_bytes(paths: &AppPaths, bytes: &[u8]) -> String {
        let token = Uuid::new_v4().to_string();
        let directory = staging_directory(paths);
        fs::create_dir_all(&directory).unwrap();
        fs::write(directory.join(format!("{token}.json")), bytes).unwrap();
        token
    }

    fn stage_value(paths: &AppPaths, value: &Value) -> String {
        stage_bytes(paths, &serde_json::to_vec(value).unwrap())
    }

    fn error_contains(result: Result<LocalImportPreview, AppError>, code: &str) {
        assert!(result.unwrap_err().to_string().contains(code));
    }

    fn seed(conn: &Connection, value: WatchRecord) {
        let value = crate::record_validation::prepare_import_batch(vec![value])
            .unwrap()
            .remove(0);
        crate::db::insert_record(conn, value).unwrap();
    }

    #[test]
    fn valid_v4_parses_and_previews_all_entity_counts() {
        let (_root, paths) = TestRoot::new("valid-v4");
        let conn = database(&paths);
        let item = record("imported", "导入", false);
        let c = collection("collection");
        let token = stage_value(
            &paths,
            &envelope(
                vec![item.clone()],
                vec![completion(&item.id, 1)],
                vec![c.clone()],
                vec![member(&c.id, &item.id)],
            ),
        );
        let result = preview(&conn, &paths, &token, "backup.json").unwrap();
        assert_eq!(result.format_version, 4);
        assert_eq!(result.counts.records, 1);
        assert_eq!(result.counts.episode_completions, 1);
        assert_eq!(result.counts.collections, 1);
        assert_eq!(result.counts.collection_members, 1);
    }

    #[test]
    fn malformed_json_is_rejected_and_discarded() {
        let (_root, paths) = TestRoot::new("malformed");
        let conn = database(&paths);
        let token = stage_bytes(&paths, b"{not-json");
        error_contains(
            preview(&conn, &paths, &token, "backup.json"),
            "invalid_json",
        );
        assert!(!stage_path(&paths, &token).unwrap().exists());
    }

    #[test]
    fn missing_format_version_is_rejected() {
        let (_root, paths) = TestRoot::new("missing-version");
        let conn = database(&paths);
        let mut value = envelope(vec![], vec![], vec![], vec![]);
        value.as_object_mut().unwrap().remove("formatVersion");
        let token = stage_value(&paths, &value);
        error_contains(
            preview(&conn, &paths, &token, "backup.json"),
            "missing_format_version",
        );
    }

    #[test]
    fn future_backup_version_is_rejected_closed() {
        let (_root, paths) = TestRoot::new("future-version");
        let conn = database(&paths);
        let mut value = envelope(vec![], vec![], vec![], vec![]);
        value["formatVersion"] = json!(5);
        let token = stage_value(&paths, &value);
        error_contains(
            preview(&conn, &paths, &token, "backup.json"),
            "future_backup_version",
        );
    }

    #[test]
    fn older_backup_version_is_not_treated_as_legacy_migration() {
        let (_root, paths) = TestRoot::new("old-version");
        let conn = database(&paths);
        let mut value = envelope(vec![], vec![], vec![], vec![]);
        value["formatVersion"] = json!(3);
        let token = stage_value(&paths, &value);
        error_contains(
            preview(&conn, &paths, &token, "backup.json"),
            "unsupported_backup_version",
        );
    }

    #[test]
    fn unknown_v4_top_level_field_is_rejected() {
        let (_root, paths) = TestRoot::new("unknown-field");
        let conn = database(&paths);
        let mut value = envelope(vec![], vec![], vec![], vec![]);
        value["credentials"] = json!({"password": "secret"});
        let token = stage_value(&paths, &value);
        error_contains(
            preview(&conn, &paths, &token, "backup.json"),
            "unknown_backup_field",
        );
    }

    #[test]
    fn invalid_exported_at_is_rejected_without_fallback() {
        let (_root, paths) = TestRoot::new("bad-exported-at");
        let conn = database(&paths);
        let mut value = envelope(vec![], vec![], vec![], vec![]);
        value["exportedAt"] = json!("today");
        let token = stage_value(&paths, &value);
        error_contains(
            preview(&conn, &paths, &token, "backup.json"),
            "invalid_backup_metadata",
        );
    }

    #[test]
    fn duplicate_record_ids_are_rejected() {
        let (_root, paths) = TestRoot::new("duplicate-record");
        let conn = database(&paths);
        let token = stage_value(
            &paths,
            &envelope(
                vec![record("same", "一", false), record("same", "二", false)],
                vec![],
                vec![],
                vec![],
            ),
        );
        error_contains(
            preview(&conn, &paths, &token, "backup.json"),
            "invalid_records",
        );
    }

    #[test]
    fn invalid_numeric_record_values_are_rejected_not_normalized() {
        let (_root, paths) = TestRoot::new("invalid-record-number");
        let conn = database(&paths);
        let mut item = record("record", "记录", false);
        item.rating = Some(99);
        let token = stage_value(&paths, &envelope(vec![item], vec![], vec![], vec![]));
        error_contains(
            preview(&conn, &paths, &token, "backup.json"),
            "invalid_records",
        );
    }

    #[test]
    fn invalid_episode_completion_is_rejected() {
        let (_root, paths) = TestRoot::new("invalid-episode");
        let conn = database(&paths);
        let item = record("record", "记录", false);
        let mut history = completion(&item.id, 1);
        history.id = "wrong".into();
        let token = stage_value(&paths, &envelope(vec![item], vec![history], vec![], vec![]));
        error_contains(
            preview(&conn, &paths, &token, "backup.json"),
            "invalid_episode_history",
        );
    }

    #[test]
    fn episode_reference_to_missing_record_is_rejected() {
        let (_root, paths) = TestRoot::new("missing-episode-record");
        let conn = database(&paths);
        let token = stage_value(
            &paths,
            &envelope(vec![], vec![completion("missing", 1)], vec![], vec![]),
        );
        error_contains(
            preview(&conn, &paths, &token, "backup.json"),
            "invalid_episode_history",
        );
    }

    #[test]
    fn invalid_collection_is_rejected() {
        let (_root, paths) = TestRoot::new("invalid-collection");
        let conn = database(&paths);
        let mut c = collection("collection");
        c.source_kind = "credentials".into();
        let token = stage_value(&paths, &envelope(vec![], vec![], vec![c], vec![]));
        error_contains(
            preview(&conn, &paths, &token, "backup.json"),
            "invalid_collections",
        );
    }

    #[test]
    fn collection_member_missing_relationship_is_rejected() {
        let (_root, paths) = TestRoot::new("invalid-member-reference");
        let conn = database(&paths);
        let item = record("record", "记录", false);
        let c = collection("collection");
        let token = stage_value(
            &paths,
            &envelope(
                vec![item],
                vec![],
                vec![c.clone()],
                vec![member(&c.id, "missing")],
            ),
        );
        error_contains(
            preview(&conn, &paths, &token, "backup.json"),
            "invalid_collections",
        );
    }

    #[test]
    fn preview_does_not_modify_library_settings_or_sync_bookkeeping() {
        let (_root, paths) = TestRoot::new("preview-read-only");
        let conn = database(&paths);
        seed(&conn, record("current", "当前", false));
        crate::db::set_setting(&conn, "custom_setting".into(), "keep".into()).unwrap();
        let before = snapshot(&conn).unwrap();
        let settings_before: Vec<(String, String)> = conn
            .prepare("SELECT key,value FROM settings ORDER BY key")
            .unwrap()
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        let token = stage_value(
            &paths,
            &envelope(vec![record("next", "导入", false)], vec![], vec![], vec![]),
        );
        preview(&conn, &paths, &token, "backup.json").unwrap();
        assert_eq!(
            serde_json::to_value(snapshot(&conn).unwrap()).unwrap(),
            serde_json::to_value(before).unwrap()
        );
        let settings_after: Vec<(String, String)> = conn
            .prepare("SELECT key,value FROM settings ORDER BY key")
            .unwrap()
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(settings_after, settings_before);
        assert!(crate::recovery_points::list(&paths)
            .unwrap()
            .points
            .is_empty());
    }

    #[test]
    fn preview_reports_add_update_remove_unchanged_and_locked() {
        let (_root, paths) = TestRoot::new("preview-diff");
        let conn = database(&paths);
        seed(&conn, record("update", "旧标题", false));
        seed(&conn, record("remove", "删除", false));
        seed(&conn, record("same", "不变", false));
        seed(&conn, record("locked", "锁定", true));
        let token = stage_value(
            &paths,
            &envelope(
                vec![
                    record("update", "新标题", false),
                    record("same", "不变", false),
                    record("locked", "试图覆盖", false),
                    record("add", "新增", false),
                ],
                vec![],
                vec![],
                vec![],
            ),
        );
        let result = preview(&conn, &paths, &token, "backup.json").unwrap();
        assert_eq!(result.records.added, 1);
        assert_eq!(result.records.updated, 1);
        assert_eq!(result.records.removed, 1);
        assert_eq!(result.records.unchanged, 1);
        assert_eq!(result.records.locked_preserved, 1);
    }

    #[test]
    fn locked_episode_history_difference_is_counted_as_protected() {
        let (_root, paths) = TestRoot::new("locked-history-preview");
        let conn = database(&paths);
        let locked = record("locked", "锁定", true);
        seed(&conn, locked.clone());
        crate::episode_history::replace_completions_tx(
            &conn,
            &[completion(&locked.id, 1)],
            &HashSet::new(),
        )
        .unwrap();
        let token = stage_value(&paths, &envelope(vec![locked], vec![], vec![], vec![]));
        let result = preview(&conn, &paths, &token, "backup.json").unwrap();
        assert_eq!(result.records.locked_preserved, 1);
    }

    #[test]
    fn commit_creates_recovery_with_actual_pre_import_database_content() {
        let (_root, paths) = TestRoot::new("recovery-content");
        let mut conn = database(&paths);
        seed(&conn, record("before", "导入前", false));
        crate::db::set_setting(
            &conn,
            "mobile_library_preferences_v1".into(),
            "keep-local".into(),
        )
        .unwrap();
        let token = stage_value(
            &paths,
            &envelope(
                vec![record("after", "导入后", false)],
                vec![],
                vec![],
                vec![],
            ),
        );
        let p = preview(&conn, &paths, &token, "backup.json").unwrap();
        let result = commit(
            &mut conn,
            &paths,
            &p.stage_token,
            &p.stage_sha256,
            &p.current_library_fingerprint,
        )
        .unwrap();
        assert_eq!(crate::db::get_all_records(&conn).unwrap()[0].id, "after");
        assert_eq!(
            crate::db::get_setting(&conn, "mobile_library_preferences_v1".into())
                .unwrap()
                .as_deref(),
            Some("keep-local")
        );
        let recovery = Connection::open(paths.backups().join(result.recovery_point_id)).unwrap();
        let recovered = crate::db::get_all_records(&recovery).unwrap();
        assert_eq!(recovered.len(), 1);
        assert_eq!(recovered[0].id, "before");
    }

    #[test]
    fn m21_four_entity_snapshot_round_trips_through_m22_without_locked_interference() {
        let (_source_root, source_paths) = TestRoot::new("roundtrip-source");
        let source = database(&source_paths);
        let item = record("roundtrip", "往返记录", false);
        let c = collection("roundtrip-collection");
        seed(&source, item.clone());
        crate::episode_history::replace_completions_tx(
            &source,
            &[completion(&item.id, 1)],
            &HashSet::new(),
        )
        .unwrap();
        crate::collections::replace_all_tx(
            &source,
            std::slice::from_ref(&c),
            &[member(&c.id, &item.id)],
            &[],
            &[],
        )
        .unwrap();
        let exported = crate::local_export::snapshot(&source).unwrap();

        let (_target_root, target_paths) = TestRoot::new("roundtrip-target");
        let mut target = database(&target_paths);
        let token = stage_value(
            &target_paths,
            &envelope(
                exported.records.clone(),
                exported.episode_completions.clone(),
                exported.collections.clone(),
                exported.collection_members.clone(),
            ),
        );
        let p = preview(&target, &target_paths, &token, "m21.json").unwrap();
        commit(
            &mut target,
            &target_paths,
            &p.stage_token,
            &p.stage_sha256,
            &p.current_library_fingerprint,
        )
        .unwrap();
        assert_eq!(
            serde_json::to_value(canonical_snapshot(snapshot(&target).unwrap())).unwrap(),
            serde_json::to_value(canonical_snapshot(exported)).unwrap()
        );
    }

    #[test]
    fn commit_atomically_replaces_all_four_entity_sets() {
        let (_root, paths) = TestRoot::new("four-entities");
        let mut conn = database(&paths);
        seed(&conn, record("before", "导入前", false));
        let imported = record("after", "导入后", false);
        let c = collection("collection");
        let token = stage_value(
            &paths,
            &envelope(
                vec![imported.clone()],
                vec![completion(&imported.id, 1)],
                vec![c.clone()],
                vec![member(&c.id, &imported.id)],
            ),
        );
        let p = preview(&conn, &paths, &token, "backup.json").unwrap();
        let result = commit(
            &mut conn,
            &paths,
            &p.stage_token,
            &p.stage_sha256,
            &p.current_library_fingerprint,
        )
        .unwrap();
        assert_eq!(result.record_count, 1);
        assert_eq!(result.episode_completion_count, 1);
        assert_eq!(result.collection_count, 1);
        assert_eq!(result.collection_member_count, 1);
        assert_eq!(snapshot(&conn).unwrap().records[0].id, "after");
    }

    #[test]
    fn locked_record_and_episode_history_are_preserved_on_commit() {
        let (_root, paths) = TestRoot::new("locked-commit");
        let mut conn = database(&paths);
        let locked = record("locked", "本机锁定", true);
        seed(&conn, locked.clone());
        crate::episode_history::replace_completions_tx(
            &conn,
            &[completion(&locked.id, 1)],
            &HashSet::new(),
        )
        .unwrap();
        let token = stage_value(
            &paths,
            &envelope(
                vec![
                    record("locked", "备份覆盖", false),
                    record("new", "新增", false),
                ],
                vec![completion("locked", 2)],
                vec![],
                vec![],
            ),
        );
        let p = preview(&conn, &paths, &token, "backup.json").unwrap();
        commit(
            &mut conn,
            &paths,
            &p.stage_token,
            &p.stage_sha256,
            &p.current_library_fingerprint,
        )
        .unwrap();
        assert_eq!(
            crate::db::get_record(&conn, "locked")
                .unwrap()
                .unwrap()
                .chinese_name,
            "本机锁定"
        );
        let history = crate::episode_history::completions(&conn, "locked").unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].episode_number, 1);
    }

    #[test]
    fn successful_commit_marks_local_mutation_and_rebuilds_sync_staging() {
        let (_root, paths) = TestRoot::new("sync-queue");
        let mut conn = database(&paths);
        let token = stage_value(
            &paths,
            &envelope(vec![record("new", "新增", false)], vec![], vec![], vec![]),
        );
        let p = preview(&conn, &paths, &token, "backup.json").unwrap();
        commit(
            &mut conn,
            &paths,
            &p.stage_token,
            &p.stage_sha256,
            &p.current_library_fingerprint,
        )
        .unwrap();
        let outbox = crate::db::get_setting(&conn, "sync_outbox_v1".into()).unwrap();
        let staging = crate::db::get_setting(&conn, "sync_staging_v1".into()).unwrap();
        assert!(outbox.is_some_and(|value| value.contains("library-import-v3")));
        assert!(staging.is_some());
    }

    #[test]
    fn invalid_commit_does_not_modify_library_or_create_recovery() {
        let (_root, paths) = TestRoot::new("invalid-commit");
        let mut conn = database(&paths);
        seed(&conn, record("before", "之前", false));
        let token = stage_value(
            &paths,
            &envelope(vec![record("after", "之后", false)], vec![], vec![], vec![]),
        );
        let p = preview(&conn, &paths, &token, "backup.json").unwrap();
        fs::write(stage_path(&paths, &token).unwrap(), b"not json").unwrap();
        let result = commit(
            &mut conn,
            &paths,
            &p.stage_token,
            &format!("{:x}", Sha256::digest(b"not json")),
            &p.current_library_fingerprint,
        );
        assert!(result.unwrap_err().to_string().contains("invalid_json"));
        assert_eq!(crate::db::get_all_records(&conn).unwrap()[0].id, "before");
        assert!(crate::recovery_points::list(&paths)
            .unwrap()
            .points
            .is_empty());
    }

    #[test]
    fn local_state_change_after_preview_is_rejected_as_stale() {
        let (_root, paths) = TestRoot::new("stale-local");
        let mut conn = database(&paths);
        let token = stage_value(
            &paths,
            &envelope(
                vec![record("import", "导入", false)],
                vec![],
                vec![],
                vec![],
            ),
        );
        let p = preview(&conn, &paths, &token, "backup.json").unwrap();
        seed(&conn, record("edited", "预览后编辑", false));
        let result = commit(
            &mut conn,
            &paths,
            &p.stage_token,
            &p.stage_sha256,
            &p.current_library_fingerprint,
        );
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("import_preview_stale"));
        assert!(crate::recovery_points::list(&paths)
            .unwrap()
            .points
            .is_empty());
    }

    #[test]
    fn stage_change_after_preview_is_rejected() {
        let (_root, paths) = TestRoot::new("changed-stage");
        let mut conn = database(&paths);
        let token = stage_value(
            &paths,
            &envelope(vec![record("one", "一", false)], vec![], vec![], vec![]),
        );
        let p = preview(&conn, &paths, &token, "backup.json").unwrap();
        fs::write(
            stage_path(&paths, &token).unwrap(),
            serde_json::to_vec(&envelope(
                vec![record("two", "二", false)],
                vec![],
                vec![],
                vec![],
            ))
            .unwrap(),
        )
        .unwrap();
        let result = commit(
            &mut conn,
            &paths,
            &p.stage_token,
            &p.stage_sha256,
            &p.current_library_fingerprint,
        );
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("import_stage_changed"));
    }

    #[test]
    fn successful_commit_removes_stage() {
        let (_root, paths) = TestRoot::new("stage-cleanup");
        let mut conn = database(&paths);
        let token = stage_value(&paths, &envelope(vec![], vec![], vec![], vec![]));
        let p = preview(&conn, &paths, &token, "backup.json").unwrap();
        commit(
            &mut conn,
            &paths,
            &p.stage_token,
            &p.stage_sha256,
            &p.current_library_fingerprint,
        )
        .unwrap();
        assert!(!stage_path(&paths, &token).unwrap().exists());
    }

    #[test]
    fn explicit_discard_removes_only_safe_stage() {
        let (_root, paths) = TestRoot::new("discard");
        let token = stage_value(&paths, &envelope(vec![], vec![], vec![], vec![]));
        let unrelated = staging_directory(&paths).join("keep.txt");
        fs::write(&unrelated, b"keep").unwrap();
        discard(&paths, &token).unwrap();
        assert!(!stage_path(&paths, &token).unwrap().exists());
        assert!(unrelated.exists());
    }

    #[test]
    fn unsafe_stage_tokens_are_rejected() {
        let (_root, paths) = TestRoot::new("unsafe-token");
        let conn = database(&paths);
        for token in ["../outside", "/absolute", "nested/token", "ABCDEF"] {
            assert!(preview(&conn, &paths, token, "backup.json")
                .unwrap_err()
                .to_string()
                .contains("invalid_import_stage_token"));
        }
    }

    #[test]
    fn oversized_stage_is_rejected_without_reading_it() {
        let (_root, paths) = TestRoot::new("oversized");
        let conn = database(&paths);
        let token = Uuid::new_v4().to_string();
        fs::create_dir_all(staging_directory(&paths)).unwrap();
        let file = fs::File::create(stage_path(&paths, &token).unwrap()).unwrap();
        file.set_len(MAX_IMPORT_BYTES + 1).unwrap();
        error_contains(
            preview(&conn, &paths, &token, "backup.json"),
            "import_file_too_large",
        );
    }

    #[test]
    fn startup_cleanup_removes_only_uuid_json_and_tmp_files() {
        let (_root, paths) = TestRoot::new("startup-cleanup");
        let token = Uuid::new_v4().to_string();
        let directory = staging_directory(&paths);
        fs::create_dir_all(&directory).unwrap();
        fs::write(directory.join(format!("{token}.json")), b"json").unwrap();
        fs::write(directory.join(format!("{}.tmp", Uuid::new_v4())), b"tmp").unwrap();
        fs::write(directory.join("important.json"), b"keep").unwrap();
        cleanup_stale(&paths).unwrap();
        assert!(directory.join("important.json").exists());
        assert_eq!(fs::read_dir(directory).unwrap().count(), 1);
    }
}
