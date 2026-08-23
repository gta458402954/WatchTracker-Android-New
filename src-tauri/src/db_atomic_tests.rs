#[cfg(test)]
mod tests {
    use crate::db;
    use crate::db_atomic_crud::{
        delete_record_atomic, insert_record_atomic, replace_all_records_atomic,
    };
    use crate::db_atomic_helpers::{
        get_records_generation, get_sync_outbox, get_tombstones_tx, set_setting_tx, Tombstone,
    };
    use crate::db_atomic_update::update_record_atomic;
    use crate::models::{Patch, RecordStatus, UpdateWatchRecord, WatchRecord};
    use crate::sync_staging::get_staging;
    use rusqlite::Connection;

    fn record(id: &str) -> WatchRecord {
        WatchRecord {
            id: id.to_string(),
            original_name: "Original".to_string(),
            chinese_name: "原始记录".to_string(),
            progress: "0".to_string(),
            total_episodes: Some(10),
            episode_tracking_enabled: false,
            next_episode: None,
            movie_progress: None,
            movie_duration: None,
            release_year: Some("2026".to_string()),
            poster_path: None,
            status: RecordStatus::Unwatched,
            platform: String::new(),
            rating: None,
            start_date: None,
            end_date: None,
            notes: String::new(),
            created_at: "2026-07-28T00:00:00.000Z".to_string(),
            updated_at: Some("2026-07-28T00:00:00.000Z".to_string()),
            imdb_id: None,
            is_locked: Some(false),
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
            rev: 2,
            rev_actor: "old-actor".to_string(),
        }
    }

    fn database() -> Connection {
        let connection = Connection::open_in_memory().expect("open test database");
        db::setup_db(&connection).expect("create current schema");
        connection
    }

    fn state(conn: &Connection, id: &str) -> (String, Option<String>, i64, String, i64) {
        conn.query_row(
            "SELECT progress, updatedAt, rev, revActor, \
             CAST((SELECT value FROM settings WHERE key = 'records_generation') AS INTEGER) \
             FROM records WHERE id = ?1",
            [id],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            },
        )
        .expect("read record state")
    }

    #[test]
    fn insert_commits_record_tombstone_cleanup_and_generation_together() {
        let mut conn = database();
        set_setting_tx(
            &conn,
            "sync_tombstones",
            r#"[{"id":"inserted","deletedAt":"2026-07-28T01:00:00Z"},{"id":"other","deletedAt":"2026-07-28T02:00:00Z"}]"#,
        )
        .expect("seed tombstones");

        insert_record_atomic(&mut conn, record("inserted"), "test-actor").expect("insert record");

        assert!(db::get_record(&conn, "inserted").unwrap().is_some());
        assert_eq!(get_records_generation(&conn).unwrap(), 1);
        let outbox = get_sync_outbox(&conn).unwrap().unwrap();
        assert!(outbox.pending);
        assert_eq!(outbox.dirty_generation, 1);
        assert_eq!(outbox.reasons, vec!["record-insert"]);
        assert_eq!(
            get_tombstones_tx(&conn).unwrap(),
            vec![Tombstone {
                id: "other".to_string(),
                deleted_at: "2026-07-28T02:00:00Z".to_string(),
                rev: 0,
                rev_actor: String::new(),
            }]
        );
        let staging = get_staging(&conn).unwrap();
        assert_eq!(staging.entries.len(), 1);
        assert_eq!(staging.entries[0].id, "inserted");
        assert!(staging.entries[0].base.is_none());
        assert!(staging.entries[0].local.is_some());
    }

    #[test]
    fn delete_commits_record_tombstone_and_generation_together() {
        let mut conn = database();
        let mut deleted = record("deleted");
        deleted.media_type = "剧集".to_string();
        db::insert_record(&conn, deleted).expect("seed record");
        let enabled = crate::episode_history::enable(&mut conn, "deleted", 2, 2, "test-actor")
            .expect("enable episode tracking");
        crate::episode_history::set_next(
            &mut conn,
            "deleted",
            Some(3),
            enabled.record.rev,
            "test-actor",
        )
        .expect("create completion");
        assert_eq!(
            crate::episode_history::completions(&conn, "deleted")
                .unwrap()
                .len(),
            1
        );

        delete_record_atomic(&mut conn, "deleted", "test-actor").expect("delete record");

        assert!(db::get_record(&conn, "deleted").unwrap().is_none());
        assert!(crate::episode_history::completions(&conn, "deleted")
            .unwrap()
            .is_empty());
        assert_eq!(get_records_generation(&conn).unwrap(), 3);
        let outbox = get_sync_outbox(&conn).unwrap().unwrap();
        assert!(outbox.pending);
        assert!(outbox.reasons.contains(&"record-delete".to_string()));
        let tombstones = get_tombstones_tx(&conn).unwrap();
        assert_eq!(tombstones.len(), 1);
        assert_eq!(tombstones[0].id, "deleted");
        assert!(!tombstones[0].deleted_at.is_empty());
    }

    #[test]
    fn insert_generation_failure_rolls_back_record_and_tombstone_cleanup() {
        let mut conn = database();
        set_setting_tx(
            &conn,
            "sync_tombstones",
            r#"[{"id":"insert-failure","deletedAt":"2026-07-28T01:00:00Z"}]"#,
        )
        .unwrap();
        let tombstones = get_tombstones_tx(&conn).unwrap();
        conn.execute_batch(
            "CREATE TRIGGER fail_insert_generation BEFORE UPDATE OF value ON settings
             WHEN OLD.key = 'records_generation'
             BEGIN SELECT RAISE(ABORT, 'injected generation failure'); END;",
        )
        .unwrap();

        assert!(insert_record_atomic(&mut conn, record("insert-failure"), "test-actor").is_err());
        assert!(db::get_record(&conn, "insert-failure").unwrap().is_none());
        assert_eq!(get_records_generation(&conn).unwrap(), 0);
        assert_eq!(get_tombstones_tx(&conn).unwrap(), tombstones);
    }

    #[test]
    fn delete_generation_failure_rolls_back_record_and_tombstone() {
        let mut conn = database();
        db::insert_record(&conn, record("delete-failure")).unwrap();
        conn.execute_batch(
            "CREATE TRIGGER fail_delete_generation BEFORE UPDATE OF value ON settings
             WHEN OLD.key = 'records_generation'
             BEGIN SELECT RAISE(ABORT, 'injected generation failure'); END;",
        )
        .unwrap();

        assert!(delete_record_atomic(&mut conn, "delete-failure", "test-actor").is_err());
        assert!(db::get_record(&conn, "delete-failure").unwrap().is_some());
        assert_eq!(get_records_generation(&conn).unwrap(), 0);
        assert!(get_tombstones_tx(&conn).unwrap().is_empty());
    }

    #[test]
    fn replace_all_records_commits_once_and_preserves_locked_records() {
        let mut conn = database();
        let mut locked = record("locked");
        locked.is_locked = Some(true);
        db::insert_record(&conn, locked).unwrap();
        db::insert_record(&conn, record("old")).unwrap();

        replace_all_records_atomic(&mut conn, vec![record("locked"), record("imported")]).unwrap();

        assert!(db::get_record(&conn, "locked").unwrap().is_some());
        assert!(db::get_record(&conn, "imported").unwrap().is_some());
        assert!(db::get_record(&conn, "old").unwrap().is_none());
        assert_eq!(get_records_generation(&conn).unwrap(), 1);
    }

    #[test]
    fn legacy_replacement_preserves_matching_episode_tracking_and_history() {
        let mut conn = database();
        let mut existing = record("series");
        existing.media_type = "剧集".to_string();
        db::insert_record(&conn, existing).unwrap();
        let enabled = crate::episode_history::enable(&mut conn, "series", 2, 2, "actor").unwrap();
        crate::episode_history::set_next(&mut conn, "series", Some(3), enabled.record.rev, "actor")
            .unwrap();

        let mut legacy = record("series");
        legacy.media_type = "剧集".to_string();
        legacy.episode_tracking_enabled = false;
        legacy.next_episode = None;
        replace_all_records_atomic(&mut conn, vec![legacy]).unwrap();

        let tracking = crate::episode_history::tracking(&conn, "series").unwrap();
        assert!(tracking.record.episode_tracking_enabled);
        assert_eq!(tracking.record.next_episode, Some(3));
        assert_eq!(tracking.completions.len(), 1);
    }

    #[test]
    fn replace_all_records_failure_rolls_back_records_and_generation() {
        let mut conn = database();
        db::insert_record(&conn, record("original")).unwrap();
        conn.execute_batch(
            "CREATE TRIGGER fail_import BEFORE INSERT ON records
             WHEN NEW.id = 'bad-import'
             BEGIN SELECT RAISE(ABORT, 'injected import failure'); END;",
        )
        .unwrap();

        assert!(replace_all_records_atomic(
            &mut conn,
            vec![record("replacement"), record("bad-import")],
        )
        .is_err());
        assert!(db::get_record(&conn, "original").unwrap().is_some());
        assert!(db::get_record(&conn, "replacement").unwrap().is_none());
        assert_eq!(get_records_generation(&conn).unwrap(), 0);
    }

    #[test]
    fn update_payload_rejects_unknown_system_and_invalid_value_types() {
        for field in [
            "id",
            "createdAt",
            "updatedAt",
            "rev",
            "revActor",
            "unknownField",
        ] {
            let error = serde_json::from_value::<UpdateWatchRecord>(serde_json::json!({field: 1}))
                .expect_err("system and unknown fields must be rejected");
            assert!(
                error.to_string().contains("unknown field"),
                "{field}: {error}"
            );
        }
        for value in [serde_json::json!([]), serde_json::json!({"nested": true})] {
            assert!(serde_json::from_value::<UpdateWatchRecord>(
                serde_json::json!({"rating": value})
            )
            .is_err());
        }
        assert!(
            serde_json::from_value::<UpdateWatchRecord>(serde_json::json!({"rating": "ten"}))
                .is_err()
        );
    }

    #[test]
    fn empty_update_is_rejected_without_side_effects() {
        let mut conn = database();
        db::insert_record(&conn, record("empty")).expect("seed record");
        set_setting_tx(
            &conn,
            "sync_tombstones",
            r#"[{"id":"empty","deletedAt":"2026-07-28T01:00:00Z"}]"#,
        )
        .expect("seed tombstone");
        let before = state(&conn, "empty");
        let tombstones = get_tombstones_tx(&conn).expect("read tombstones");

        let error =
            update_record_atomic(&mut conn, "empty", &UpdateWatchRecord::default(), "actor")
                .expect_err("empty update must fail");

        assert!(error.to_string().contains("Empty update payload"));
        assert_eq!(state(&conn, "empty"), before);
        assert_eq!(get_tombstones_tx(&conn).unwrap(), tombstones);
    }

    #[test]
    fn update_uses_rust_time_and_commits_record_tombstone_and_generation_together() {
        let mut conn = database();
        db::insert_record(&conn, record("updated")).expect("seed record");
        set_setting_tx(
            &conn,
            "sync_tombstones",
            r#"[{"id":"updated","deletedAt":"2026-07-28T01:00:00Z"},{"id":"other","deletedAt":"2026-07-28T02:00:00Z"}]"#,
        )
        .expect("seed tombstones");
        let old_updated_at = db::get_record(&conn, "updated")
            .unwrap()
            .unwrap()
            .updated_at
            .unwrap();
        let updates = UpdateWatchRecord {
            progress: Some("5".to_string()),
            rating: Patch::Value(9),
            ..Default::default()
        };

        let persisted = update_record_atomic(&mut conn, "updated", &updates, "rust-actor")
            .expect("atomic update");

        assert_eq!(persisted.progress, "5");
        assert_eq!(persisted.rating, Some(9));
        assert_eq!(persisted.rev, 3);
        assert_eq!(persisted.rev_actor, "rust-actor");
        assert_ne!(
            persisted.updated_at.as_deref(),
            Some(old_updated_at.as_str())
        );
        assert_eq!(get_records_generation(&conn).unwrap(), 1);
        assert_eq!(
            get_tombstones_tx(&conn).unwrap(),
            vec![Tombstone {
                id: "other".to_string(),
                deleted_at: "2026-07-28T02:00:00Z".to_string(),
                rev: 0,
                rev_actor: String::new(),
            }]
        );
    }

    #[test]
    fn setting_failure_rolls_back_record_tombstone_and_generation() {
        let mut conn = database();
        db::insert_record(&conn, record("rollback")).expect("seed record");
        set_setting_tx(
            &conn,
            "sync_tombstones",
            r#"[{"id":"rollback","deletedAt":"2026-07-28T01:00:00Z"}]"#,
        )
        .expect("seed tombstone");
        conn.execute_batch(
            "CREATE TRIGGER fail_generation BEFORE UPDATE OF value ON settings
             WHEN OLD.key = 'records_generation'
             BEGIN SELECT RAISE(ABORT, 'injected generation failure'); END;",
        )
        .expect("install failure trigger");
        let before = state(&conn, "rollback");
        let tombstones = get_tombstones_tx(&conn).unwrap();

        let result = update_record_atomic(
            &mut conn,
            "rollback",
            &UpdateWatchRecord {
                progress: Some("9".to_string()),
                ..Default::default()
            },
            "actor",
        );

        assert!(result.is_err());
        assert_eq!(state(&conn, "rollback"), before);
        assert_eq!(get_tombstones_tx(&conn).unwrap(), tombstones);
    }

    #[test]
    fn outbox_failure_rolls_back_the_local_record_and_generation() {
        let mut conn = database();
        db::insert_record(&conn, record("outbox-rollback")).unwrap();
        let before = state(&conn, "outbox-rollback");
        conn.execute_batch(
            "CREATE TRIGGER fail_outbox BEFORE INSERT ON settings
             WHEN NEW.key = 'sync_outbox_v1'
             BEGIN SELECT RAISE(ABORT, 'injected outbox failure'); END;",
        )
        .unwrap();
        let result = update_record_atomic(
            &mut conn,
            "outbox-rollback",
            &UpdateWatchRecord {
                notes: Some("must roll back".to_string()),
                ..Default::default()
            },
            "actor",
        );
        assert!(result.is_err());
        assert_eq!(state(&conn, "outbox-rollback"), before);
        assert_eq!(get_records_generation(&conn).unwrap(), 0);
        assert!(get_sync_outbox(&conn).unwrap().is_none());
    }

    #[test]
    fn repeated_local_updates_coalesce_into_one_generation_high_watermark() {
        let mut conn = database();
        db::insert_record(&conn, record("coalesced")).unwrap();
        for progress in ["1", "2", "3"] {
            update_record_atomic(
                &mut conn,
                "coalesced",
                &UpdateWatchRecord {
                    progress: Some(progress.to_string()),
                    ..Default::default()
                },
                "actor",
            )
            .unwrap();
        }
        let outbox = get_sync_outbox(&conn).unwrap().unwrap();
        assert!(outbox.pending);
        assert_eq!(outbox.dirty_generation, 3);
        assert_eq!(outbox.reasons, vec!["record-update"]);
        assert!(outbox.first_queued_at.is_some());
        assert!(outbox.last_queued_at.is_some());
        let staging = get_staging(&conn).unwrap();
        assert_eq!(staging.entries.len(), 1);
        assert_eq!(staging.entries[0].first_generation, 1);
        assert_eq!(staging.entries[0].last_generation, 3);
        assert_eq!(
            staging.entries[0]
                .local
                .as_ref()
                .and_then(|value| value.get("progress"))
                .and_then(serde_json::Value::as_str),
            Some("3")
        );
    }

    #[test]
    fn record_upsert_never_deletes_and_rust_owns_revision_fields() {
        let mut conn = database();
        db::insert_record(&conn, record("upserted")).expect("seed existing record");
        conn.execute_batch(
            "CREATE TABLE delete_audit (id TEXT NOT NULL);
             CREATE TRIGGER audit_record_delete AFTER DELETE ON records
             BEGIN INSERT INTO delete_audit(id) VALUES (OLD.id); END;",
        )
        .expect("install delete audit");

        let mut replacement = record("upserted");
        replacement.chinese_name = "明确 UPSERT".to_string();
        replacement.updated_at = Some("1900-01-01T00:00:00Z".to_string());
        replacement.rev = 999;
        replacement.rev_actor = "untrusted".to_string();
        let persisted =
            insert_record_atomic(&mut conn, replacement, "test-actor").expect("upsert record");

        let deletes: i64 = conn
            .query_row("SELECT COUNT(*) FROM delete_audit", [], |row| row.get(0))
            .unwrap();
        assert_eq!(
            deletes, 0,
            "UPSERT must not use delete-and-reinsert semantics"
        );
        assert_eq!(persisted.chinese_name, "明确 UPSERT");
        assert_eq!(persisted.rev, 3);
        assert_eq!(persisted.rev_actor, "test-actor");
        assert_ne!(
            persisted.updated_at.as_deref(),
            Some("1900-01-01T00:00:00Z")
        );
    }

    #[test]
    fn local_insert_rejects_invalid_domain_values_without_side_effects() {
        let mut conn = database();
        let mut blank = record("blank-name");
        blank.original_name = "  ".to_string();
        blank.chinese_name = "".to_string();
        assert!(insert_record_atomic(&mut conn, blank, "test-actor").is_err());

        let mut invalid_type = record("invalid-type");
        invalid_type.media_type = "legacy-type".to_string();
        assert!(insert_record_atomic(&mut conn, invalid_type, "test-actor").is_err());

        let mut invalid_numbers = record("invalid-numbers");
        invalid_numbers.rating = Some(11);
        invalid_numbers.movie_duration = Some(-1);
        assert!(insert_record_atomic(&mut conn, invalid_numbers, "test-actor").is_err());

        assert_eq!(db::get_all_records(&conn).unwrap().len(), 0);
        assert_eq!(get_records_generation(&conn).unwrap(), 0);
    }

    #[test]
    fn update_validates_changed_fields_but_allows_unrelated_repairs_on_legacy_rows() {
        let mut conn = database();
        db::insert_record(&conn, record("legacy-row")).expect("seed record");
        conn.execute(
            "UPDATE records SET mediaType = 'legacy-type' WHERE id = 'legacy-row'",
            [],
        )
        .expect("seed compatible dirty field");

        let notes_update = UpdateWatchRecord {
            notes: Some("仍可修改备注".to_string()),
            ..Default::default()
        };
        let repaired = update_record_atomic(&mut conn, "legacy-row", &notes_update, "local")
            .expect("update unrelated field on legacy row");
        assert_eq!(repaired.notes, "仍可修改备注");
        assert_eq!(repaired.media_type, "legacy-type");

        let invalid_type = UpdateWatchRecord {
            media_type: Some("bad-type".to_string()),
            ..Default::default()
        };
        assert!(update_record_atomic(&mut conn, "legacy-row", &invalid_type, "local").is_err());

        let blank_names = UpdateWatchRecord {
            original_name: Some(" ".to_string()),
            chinese_name: Some("".to_string()),
            ..Default::default()
        };
        assert!(update_record_atomic(&mut conn, "legacy-row", &blank_names, "local").is_err());
        let unchanged = db::get_record(&conn, "legacy-row").unwrap().unwrap();
        assert_eq!(unchanged.chinese_name, "原始记录");
    }

    #[test]
    fn replacement_normalizes_legacy_values_and_rejects_duplicate_ids_before_deleting() {
        let mut conn = database();
        db::insert_record(&conn, record("existing")).expect("seed existing record");

        let mut legacy = record(" legacy ");
        legacy.original_name = "".to_string();
        legacy.chinese_name = "".to_string();
        legacy.media_type = "old-category".to_string();
        legacy.total_episodes = Some(0);
        legacy.movie_progress = Some(-1);
        legacy.movie_duration = Some(0);
        legacy.rating = Some(99);
        legacy.interest_level = Some(0);
        legacy.episode_runtime = Some(-1);
        legacy.imdb_rating = Some(11.0);
        legacy.rev = -5;
        legacy.imdb_id = Some("  ".to_string());
        replace_all_records_atomic(&mut conn, vec![legacy]).expect("normalize legacy batch");

        let normalized = db::get_record(&conn, "legacy").unwrap().unwrap();
        assert_eq!(normalized.media_type, "电影");
        assert_eq!(normalized.total_episodes, None);
        assert_eq!(normalized.movie_progress, None);
        assert_eq!(normalized.movie_duration, None);
        assert_eq!(normalized.rating, None);
        assert_eq!(normalized.interest_level, None);
        assert_eq!(normalized.episode_runtime, None);
        assert_eq!(normalized.imdb_rating, None);
        assert_eq!(normalized.imdb_id, None);
        assert_eq!(normalized.rev, 0);

        let before_generation = get_records_generation(&conn).unwrap();
        let mut duplicate_a = record(" duplicate ");
        duplicate_a.chinese_name = "第一条".to_string();
        let mut duplicate_b = record("duplicate");
        duplicate_b.chinese_name = "第二条".to_string();
        assert!(replace_all_records_atomic(&mut conn, vec![duplicate_a, duplicate_b]).is_err());
        assert!(db::get_record(&conn, "legacy").unwrap().is_some());
        assert_eq!(get_records_generation(&conn).unwrap(), before_generation);
    }

    #[test]
    fn record_sql_failure_leaves_all_atomic_state_unchanged() {
        let mut conn = database();
        db::insert_record(&conn, record("sql-failure")).expect("seed record");
        set_setting_tx(
            &conn,
            "sync_tombstones",
            r#"[{"id":"sql-failure","deletedAt":"2026-07-28T01:00:00Z"}]"#,
        )
        .expect("seed tombstone");
        conn.execute_batch(
            "CREATE TRIGGER fail_record_update BEFORE UPDATE ON records
             WHEN OLD.id = 'sql-failure'
             BEGIN SELECT RAISE(ABORT, 'injected record failure'); END;",
        )
        .expect("install record failure trigger");
        let before = state(&conn, "sql-failure");
        let tombstones = get_tombstones_tx(&conn).unwrap();

        let result = update_record_atomic(
            &mut conn,
            "sql-failure",
            &UpdateWatchRecord {
                progress: Some("8".to_string()),
                ..Default::default()
            },
            "actor",
        );

        assert!(result.is_err());
        assert_eq!(state(&conn, "sql-failure"), before);
        assert_eq!(get_tombstones_tx(&conn).unwrap(), tombstones);
    }

    #[test]
    fn missing_record_preserves_existing_tombstone_and_generation() {
        let mut conn = database();
        set_setting_tx(
            &conn,
            "sync_tombstones",
            r#"[{"id":"missing","deletedAt":"2026-07-28T01:00:00Z"}]"#,
        )
        .expect("seed tombstone");
        let tombstones = get_tombstones_tx(&conn).unwrap();
        let generation = get_records_generation(&conn).unwrap();

        let error = update_record_atomic(
            &mut conn,
            "missing",
            &UpdateWatchRecord {
                progress: Some("1".to_string()),
                ..Default::default()
            },
            "actor",
        )
        .expect_err("missing record must fail");

        assert!(error.to_string().contains("Record not found"));
        assert_eq!(get_tombstones_tx(&conn).unwrap(), tombstones);
        assert_eq!(get_records_generation(&conn).unwrap(), generation);
    }

    #[test]
    fn migration_version_write_failure_rolls_back_schema_and_can_retry() {
        let conn = database();
        conn.execute("ALTER TABLE records ADD COLUMN category TEXT", [])
            .expect("add legacy category");
        conn.execute("ALTER TABLE records ADD COLUMN sortOrder INTEGER", [])
            .expect("add legacy sort order");
        conn.execute(
            "UPDATE settings SET value = '13' WHERE key = 'db_version'",
            [],
        )
        .expect("set v13");
        conn.execute_batch(
            "CREATE TRIGGER fail_v14_version BEFORE INSERT ON settings
             WHEN NEW.key = 'db_version' AND NEW.value = '14'
             BEGIN SELECT RAISE(ABORT, 'injected migration failure'); END;",
        )
        .expect("install migration trigger");

        assert!(db::setup_db(&conn).is_err());
        let records_table: i64 = conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = 'records'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let temporary_table: i64 = conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = 'records_v14'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(records_table, 1);
        assert_eq!(temporary_table, 0);
        assert_eq!(
            db::get_setting(&conn, "db_version".to_string())
                .unwrap()
                .as_deref(),
            Some("13")
        );

        conn.execute("DROP TRIGGER fail_v14_version", [])
            .expect("remove migration trigger");
        db::setup_db(&conn).expect("retry migrations");
        assert_eq!(
            db::get_setting(&conn, "db_version".to_string())
                .unwrap()
                .as_deref(),
            Some("18")
        );
    }

    #[test]
    fn v17_migration_is_atomic_and_retryable() {
        let conn = database();
        conn.execute("ALTER TABLE records DROP COLUMN revActor", [])
            .expect("remove v18 column");
        conn.execute("ALTER TABLE records DROP COLUMN rev", [])
            .expect("remove v17 column");
        conn.execute("DELETE FROM settings WHERE key = 'records_generation'", [])
            .unwrap();
        conn.execute(
            "UPDATE settings SET value = '16' WHERE key = 'db_version'",
            [],
        )
        .unwrap();
        conn.execute_batch(
            "CREATE TRIGGER fail_v17_version BEFORE INSERT ON settings
             WHEN NEW.key = 'db_version' AND NEW.value = '17'
             BEGIN SELECT RAISE(ABORT, 'injected v17 failure'); END;",
        )
        .unwrap();

        assert!(db::setup_db(&conn).is_err());
        let rev_exists: i64 = conn
            .query_row(
                "SELECT count(*) FROM pragma_table_info('records') WHERE name = 'rev'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(rev_exists, 0);
        assert_eq!(
            db::get_setting(&conn, "db_version".to_string())
                .unwrap()
                .as_deref(),
            Some("16")
        );

        conn.execute("DROP TRIGGER fail_v17_version", []).unwrap();
        db::setup_db(&conn).expect("retry v17 and v18");
        assert_eq!(
            db::get_setting(&conn, "db_version".to_string())
                .unwrap()
                .as_deref(),
            Some("18")
        );
    }

    #[test]
    fn v18_migration_is_atomic_and_retryable() {
        let conn = database();
        conn.execute("ALTER TABLE records DROP COLUMN revActor", [])
            .expect("remove v18 column");
        conn.execute("DELETE FROM settings WHERE key = 'records_generation'", [])
            .unwrap();
        conn.execute(
            "UPDATE settings SET value = '17' WHERE key = 'db_version'",
            [],
        )
        .unwrap();
        conn.execute_batch(
            "CREATE TRIGGER fail_v18_version BEFORE INSERT ON settings
             WHEN NEW.key = 'db_version' AND NEW.value = '18'
             BEGIN SELECT RAISE(ABORT, 'injected v18 failure'); END;",
        )
        .unwrap();

        assert!(db::setup_db(&conn).is_err());
        let rev_actor_exists: i64 = conn
            .query_row(
                "SELECT count(*) FROM pragma_table_info('records') WHERE name = 'revActor'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(rev_actor_exists, 0);
        assert!(db::get_setting(&conn, "records_generation".to_string())
            .unwrap()
            .is_none());

        conn.execute("DROP TRIGGER fail_v18_version", []).unwrap();
        db::setup_db(&conn).expect("retry v18");
        assert_eq!(
            db::get_setting(&conn, "db_version".to_string())
                .unwrap()
                .as_deref(),
            Some("18")
        );
        assert_eq!(
            db::get_setting(&conn, "records_generation".to_string())
                .unwrap()
                .as_deref(),
            Some("0")
        );
    }

    #[test]
    fn get_setting_only_maps_missing_rows_to_none() {
        let conn = database();
        assert!(db::get_setting(&conn, "missing".to_string())
            .unwrap()
            .is_none());

        let broken = Connection::open_in_memory().expect("open database without schema");
        assert!(db::get_setting(&broken, "missing".to_string()).is_err());
    }
}
