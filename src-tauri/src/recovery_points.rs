use crate::app_paths::AppPaths;
use crate::db::CURRENT_DB_VERSION;
use crate::db_atomic_helpers::mark_local_records_mutated;
use crate::error::AppError;
use chrono::{SecondsFormat, Utc};
use rusqlite::{backup::Backup, Connection, DatabaseName, OpenFlags, OptionalExtension};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

const MAX_AUTOMATIC_POINTS: usize = 10;
const CAPACITY_BYTES: u64 = 500 * 1024 * 1024;
const FILE_PREFIX: &str = "watchtracker-recovery-";
static NEXT_POINT_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecoveryPoint {
    pub id: String,
    pub created_at: String,
    pub reason: String,
    pub database_version: i32,
    pub record_count: i64,
    pub size_bytes: u64,
    pub sha256: String,
    pub retained: bool,
    pub integrity_ok: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecoveryPointList {
    pub points: Vec<RecoveryPoint>,
    pub total_bytes: u64,
    pub capacity_bytes: u64,
    pub capacity_exceeded: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecoveryResult {
    pub pre_restore_point_id: String,
    pub record_count: i64,
}

fn general(message: impl Into<String>) -> AppError {
    AppError::General(message.into())
}

fn validate_reason(reason: &str) -> Result<&str, AppError> {
    match reason {
        "import"
        | "sync"
        | "batch-metadata"
        | "migration"
        | "target-migration"
        | "episode-history-migration"
        | "collections-migration"
        | "series-identity-migration"
        | "series-completion"
        | "pre-restore" => Ok(reason),
        _ => Err(general("Invalid recovery point reason")),
    }
}

fn safe_point_path(paths: &AppPaths, id: &str) -> Result<PathBuf, AppError> {
    let candidate = Path::new(id);
    let safe = candidate.file_name().and_then(|name| name.to_str()) == Some(id)
        && id.starts_with(FILE_PREFIX)
        && id.ends_with(".db");
    if !safe {
        return Err(general("Invalid recovery point ID"));
    }
    Ok(paths.backups().join(id))
}

fn manifest_path(database_path: &Path) -> PathBuf {
    database_path.with_extension("json")
}

fn database_version(conn: &Connection) -> Result<i32, AppError> {
    let value = conn
        .query_row(
            "SELECT value FROM settings WHERE key = 'db_version'",
            [],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    Ok(value.and_then(|raw| raw.parse().ok()).unwrap_or(0))
}

fn record_count(conn: &Connection) -> Result<i64, AppError> {
    Ok(conn.query_row("SELECT COUNT(*) FROM records", [], |row| row.get(0))?)
}

fn verify_database(path: &Path) -> Result<(i32, i64), AppError> {
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let integrity: String = conn.query_row("PRAGMA integrity_check", [], |row| row.get(0))?;
    if integrity != "ok" {
        return Err(general("Recovery point integrity check failed"));
    }
    let foreign_key_violations: i64 =
        conn.query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |row| {
            row.get(0)
        })?;
    if foreign_key_violations != 0 {
        return Err(general("Recovery point contains invalid relationships"));
    }
    Ok((database_version(&conn)?, record_count(&conn)?))
}

fn file_sha256(path: &Path) -> Result<String, AppError> {
    let mut file = fs::File::open(path)?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Ok(format!("{:X}", digest.finalize()))
}

fn write_manifest(point: &RecoveryPoint, database_path: &Path) -> Result<(), AppError> {
    let target = manifest_path(database_path);
    let temporary = target.with_extension("json.tmp");
    fs::write(
        &temporary,
        serde_json::to_vec_pretty(point).map_err(|error| general(error.to_string()))?,
    )?;
    if target.exists() {
        fs::remove_file(&target)?;
    }
    fs::rename(temporary, target)?;
    Ok(())
}

fn read_manifest(database_path: &Path) -> Result<RecoveryPoint, AppError> {
    let bytes = fs::read(manifest_path(database_path))?;
    serde_json::from_slice(&bytes).map_err(|error| general(error.to_string()))
}

fn create_raw(
    conn: &Connection,
    paths: &AppPaths,
    reason: &str,
) -> Result<RecoveryPoint, AppError> {
    let reason = validate_reason(reason)?;
    let sequence = NEXT_POINT_ID.fetch_add(1, Ordering::Relaxed);
    let timestamp = Utc::now();
    let id = format!(
        "{FILE_PREFIX}{}-{}-{sequence}.db",
        timestamp.format("%Y%m%d-%H%M%S-%3f"),
        reason,
    );
    let target = safe_point_path(paths, &id)?;
    let temporary = target.with_extension("db.tmp");
    conn.backup(DatabaseName::Main, &temporary, None)?;

    let verified = verify_database(&temporary);
    let (version, records) = match verified {
        Ok(result) => result,
        Err(error) => {
            let _ = fs::remove_file(&temporary);
            return Err(error);
        }
    };
    fs::rename(&temporary, &target)?;
    let point = RecoveryPoint {
        id,
        created_at: timestamp.to_rfc3339_opts(SecondsFormat::Millis, true),
        reason: reason.to_string(),
        database_version: version,
        record_count: records,
        size_bytes: fs::metadata(&target)?.len(),
        sha256: file_sha256(&target)?,
        retained: false,
        integrity_ok: true,
    };
    if let Err(error) = write_manifest(&point, &target) {
        let _ = fs::remove_file(&target);
        return Err(error);
    }
    Ok(point)
}

fn remove_files(paths: &AppPaths, id: &str) -> Result<(), AppError> {
    let database = safe_point_path(paths, id)?;
    if database.exists() {
        fs::remove_file(&database)?;
    }
    let manifest = manifest_path(&database);
    if manifest.exists() {
        fs::remove_file(manifest)?;
    }
    Ok(())
}

fn rotate(paths: &AppPaths) -> Result<(), AppError> {
    let mut points = list(paths)?.points;
    points.sort_by(|left, right| left.created_at.cmp(&right.created_at));
    let mut automatic: Vec<_> = points
        .iter()
        .filter(|point| !point.retained)
        .cloned()
        .collect();
    while automatic.len() > MAX_AUTOMATIC_POINTS {
        let point = automatic.remove(0);
        remove_files(paths, &point.id)?;
        points.retain(|candidate| candidate.id != point.id);
    }

    let mut total: u64 = points.iter().map(|point| point.size_bytes).sum();
    while total > CAPACITY_BYTES && automatic.len() > 1 {
        let point = automatic.remove(0);
        remove_files(paths, &point.id)?;
        points.retain(|candidate| candidate.id != point.id);
        total = points.iter().map(|candidate| candidate.size_bytes).sum();
    }
    Ok(())
}

pub fn cleanup_temporary_files(paths: &AppPaths) -> Result<(), AppError> {
    for entry in fs::read_dir(paths.backups())? {
        let path = entry?.path();
        let name = path
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or_default();
        if name.starts_with(FILE_PREFIX) && name.ends_with(".tmp") && path.is_file() {
            fs::remove_file(path)?;
        }
    }
    Ok(())
}

pub fn create(
    conn: &Connection,
    paths: &AppPaths,
    reason: &str,
) -> Result<RecoveryPoint, AppError> {
    let point = create_raw(conn, paths, reason)?;
    rotate(paths)?;
    Ok(point)
}

pub fn list(paths: &AppPaths) -> Result<RecoveryPointList, AppError> {
    let mut points = Vec::new();
    for entry in fs::read_dir(paths.backups())? {
        let database = entry?.path();
        let id = database
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or_default();
        if !database.is_file() || !id.starts_with(FILE_PREFIX) || !id.ends_with(".db") {
            continue;
        }
        let mut point = match read_manifest(&database) {
            Ok(point) => point,
            Err(_) => continue,
        };
        point.size_bytes = fs::metadata(&database)?.len();
        point.integrity_ok =
            verify_database(&database).is_ok() && file_sha256(&database)? == point.sha256;
        points.push(point);
    }
    points.sort_by(|left, right| right.created_at.cmp(&left.created_at));
    let total_bytes = points.iter().map(|point| point.size_bytes).sum();
    Ok(RecoveryPointList {
        points,
        total_bytes,
        capacity_bytes: CAPACITY_BYTES,
        capacity_exceeded: total_bytes > CAPACITY_BYTES,
    })
}

pub fn set_retained(paths: &AppPaths, id: &str, retained: bool) -> Result<(), AppError> {
    let database = safe_point_path(paths, id)?;
    let mut point = read_manifest(&database)?;
    point.retained = retained;
    write_manifest(&point, &database)?;
    if !retained {
        rotate(paths)?;
    }
    Ok(())
}

pub fn delete(paths: &AppPaths, id: &str) -> Result<(), AppError> {
    remove_files(paths, id)
}

fn restore_from(source_path: &Path, destination: &mut Connection) -> Result<(), AppError> {
    let source = Connection::open_with_flags(source_path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let backup = Backup::new(&source, destination)?;
    backup.run_to_completion(5, Duration::from_millis(25), None)?;
    Ok(())
}

pub fn restore(
    conn: &mut Connection,
    paths: &AppPaths,
    id: &str,
) -> Result<RecoveryResult, AppError> {
    let target = safe_point_path(paths, id)?;
    let point = read_manifest(&target)?;
    let (version, _) = verify_database(&target)?;
    if !point.integrity_ok || file_sha256(&target)? != point.sha256 || version != CURRENT_DB_VERSION
    {
        return Err(general("Recovery point is damaged or incompatible"));
    }

    let pre_restore = create_raw(conn, paths, "pre-restore")?;
    if let Err(error) = restore_from(&target, conn) {
        let pre_restore_path = safe_point_path(paths, &pre_restore.id)?;
        let _ = restore_from(&pre_restore_path, conn);
        return Err(error);
    }
    let restored_state = (|| -> Result<i64, AppError> {
        let integrity: String = conn.query_row("PRAGMA integrity_check", [], |row| row.get(0))?;
        let foreign_key_violations: i64 =
            conn.query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |row| {
                row.get(0)
            })?;
        if integrity != "ok"
            || foreign_key_violations != 0
            || database_version(conn)? != CURRENT_DB_VERSION
        {
            return Err(general("Restored database failed verification"));
        }
        record_count(conn)
    })();
    let restored_record_count = match restored_state {
        Ok(count) => count,
        Err(_) => {
            let pre_restore_path = safe_point_path(paths, &pre_restore.id)?;
            restore_from(&pre_restore_path, conn)?;
            return Err(general(
                "Restored database failed verification; current data was recovered",
            ));
        }
    };
    let queue_result = (|| -> Result<(), AppError> {
        let transaction = conn.transaction()?;
        mark_local_records_mutated(&transaction, "recovery-restore")?;
        transaction.commit()?;
        Ok(())
    })();
    if queue_result.is_err() {
        let pre_restore_path = safe_point_path(paths, &pre_restore.id)?;
        restore_from(&pre_restore_path, conn)?;
        return Err(general(
            "Restored database could not be queued for synchronization; current data was recovered",
        ));
    }
    rotate(paths)?;
    Ok(RecoveryResult {
        pre_restore_point_id: pre_restore.id,
        record_count: restored_record_count,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app_paths::AppPaths;
    use crate::db;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_TEST_ID: AtomicU64 = AtomicU64::new(1);

    struct TestDatabase {
        root: PathBuf,
        paths: AppPaths,
        conn: Connection,
    }

    impl TestDatabase {
        fn new(name: &str) -> Self {
            let id = NEXT_TEST_ID.fetch_add(1, Ordering::Relaxed);
            let root = std::env::temp_dir().join(format!(
                "watchtracker-recovery-tests-{}-{name}-{id}",
                std::process::id()
            ));
            let paths = AppPaths::resolve_from(None, &root).expect("resolve test paths");
            let conn = Connection::open(paths.database()).expect("open test database");
            db::setup_db(&conn).expect("set up test database");
            Self { root, paths, conn }
        }

        fn insert_record(&self, id: &str) {
            self.conn.execute(
                "INSERT INTO records (
                    id, originalName, chineseName, progress, status, platform, startDate,
                    endDate, notes, createdAt, mediaType, rev, revActor
                 ) VALUES (?1, '', ?1, '', '未看', '', '', '', '', '2026-01-01T00:00:00Z', '电影', 0, '')",
                [id],
            ).expect("insert fixture record");
        }
    }

    impl Drop for TestDatabase {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn snapshot_is_verified_and_restore_recovers_complete_database_state() {
        let mut test = TestDatabase::new("restore");
        test.insert_record("before");
        test.conn
            .execute(
                "INSERT INTO settings (key, value) VALUES ('records_generation', '7')
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                [],
            )
            .unwrap();

        let point = create(&test.conn, &test.paths, "import").expect("create recovery point");
        assert!(point.integrity_ok);
        assert_eq!(point.database_version, CURRENT_DB_VERSION);
        assert_eq!(point.record_count, 1);
        assert_eq!(list(&test.paths).unwrap().points.len(), 1);

        test.conn.execute("DELETE FROM records", []).unwrap();
        test.insert_record("after");
        test.conn
            .execute(
                "UPDATE settings SET value = '99' WHERE key = 'records_generation'",
                [],
            )
            .unwrap();

        let result = restore(&mut test.conn, &test.paths, &point.id).expect("restore point");
        assert_eq!(result.record_count, 1);
        assert_eq!(record_count(&test.conn).unwrap(), 1);
        assert_eq!(
            test.conn
                .query_row("SELECT id FROM records", [], |row| row.get::<_, String>(0))
                .unwrap(),
            "before"
        );
        assert_eq!(
            test.conn
                .query_row(
                    "SELECT value FROM settings WHERE key = 'records_generation'",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .unwrap(),
            "8"
        );
        let outbox = crate::db_atomic_helpers::get_sync_outbox(&test.conn)
            .unwrap()
            .unwrap();
        assert!(outbox.pending);
        assert_eq!(outbox.dirty_generation, 8);
        assert_eq!(outbox.reasons, vec!["recovery-restore"]);
        assert!(list(&test.paths)
            .unwrap()
            .points
            .iter()
            .any(|item| item.reason == "pre-restore"));
    }

    #[test]
    fn damaged_snapshot_is_rejected_without_changing_current_database() {
        let mut test = TestDatabase::new("corrupt");
        test.insert_record("protected");
        let point = create(&test.conn, &test.paths, "sync").unwrap();
        fs::write(safe_point_path(&test.paths, &point.id).unwrap(), b"damaged").unwrap();

        assert!(restore(&mut test.conn, &test.paths, &point.id).is_err());
        assert_eq!(record_count(&test.conn).unwrap(), 1);
        assert_eq!(
            test.conn
                .query_row("SELECT id FROM records", [], |row| row.get::<_, String>(0))
                .unwrap(),
            "protected"
        );
    }

    #[test]
    fn rotation_keeps_ten_automatic_points_plus_retained_points() {
        let test = TestDatabase::new("rotation");
        test.insert_record("fixture");
        let retained = create(&test.conn, &test.paths, "import").unwrap();
        set_retained(&test.paths, &retained.id, true).unwrap();
        for _ in 0..12 {
            create(&test.conn, &test.paths, "sync").unwrap();
        }

        let points = list(&test.paths).unwrap().points;
        assert_eq!(points.iter().filter(|point| !point.retained).count(), 10);
        assert!(points
            .iter()
            .any(|point| point.id == retained.id && point.retained));
    }

    #[test]
    fn retained_state_is_persisted_and_delete_removes_database_and_manifest() {
        let test = TestDatabase::new("manage");
        test.insert_record("fixture");
        let point = create(&test.conn, &test.paths, "import").unwrap();

        set_retained(&test.paths, &point.id, true).unwrap();
        assert!(list(&test.paths).unwrap().points[0].retained);

        delete(&test.paths, &point.id).unwrap();
        assert!(!safe_point_path(&test.paths, &point.id).unwrap().exists());
        assert!(!manifest_path(&safe_point_path(&test.paths, &point.id).unwrap()).exists());
        assert!(list(&test.paths).unwrap().points.is_empty());
    }

    #[test]
    fn failed_snapshot_prevents_the_guarded_operation() {
        let test = TestDatabase::new("blocked");
        test.insert_record("untouched");
        fs::remove_dir_all(test.paths.backups()).unwrap();
        fs::write(test.paths.backups(), b"not a directory").unwrap();

        let guarded = (|| -> Result<(), AppError> {
            create(&test.conn, &test.paths, "import")?;
            test.conn.execute("DELETE FROM records", [])?;
            Ok(())
        })();
        assert!(guarded.is_err());
        assert_eq!(record_count(&test.conn).unwrap(), 1);
    }

    #[test]
    fn cleanup_removes_only_recovery_temporary_files() {
        let test = TestDatabase::new("cleanup");
        let temporary = test
            .paths
            .backups()
            .join("watchtracker-recovery-stale.db.tmp");
        let unrelated = test.paths.backups().join("keep.tmp");
        fs::write(&temporary, b"partial").unwrap();
        fs::write(&unrelated, b"unrelated").unwrap();

        cleanup_temporary_files(&test.paths).unwrap();
        assert!(!temporary.exists());
        assert!(unrelated.exists());
    }
}
