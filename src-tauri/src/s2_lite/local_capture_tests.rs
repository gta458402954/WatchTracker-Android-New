use super::local_authority::{
    load_staged_descriptors, CapturedStagingDescriptorV1, StagingAnchorStateV1,
};
use super::ordinary_mutation::{
    map_ordinary_mutation_v1, OrdinaryCausalBaseV1, OrdinaryMutationRequestV1, OrdinaryPayloadV1,
};
use crate::{db, db_atomic_crud, episode_history, sync_staging};
use rusqlite::{params, Connection};
use serde_json::json;

fn record(id: &str) -> crate::models::WatchRecord {
    serde_json::from_value(
        json!({"id":id,"originalName":"Series","chineseName":"Series",
        "progress":"","totalEpisodes":6,"status":"未看","platform":"","notes":"",
        "createdAt":"2026-01-01T00:00:00.000Z","mediaType":"剧集","rev":3,"revActor":"seed"}),
    )
    .unwrap()
}

fn database() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    db::setup_db(&conn).unwrap();
    conn.execute_batch("PRAGMA foreign_keys=ON").unwrap();
    db::insert_record(&conn, record("r1")).unwrap();
    conn
}

fn seed_collection(conn: &Connection) -> String {
    let id = super::canonical::sha256_hex(b"collection-member:v1\0c1\0r1");
    conn.execute("INSERT INTO collections(id,name,normalizedName,description,sourceKind,sourceKey,collectionKind,orderMode,createdAt,updatedAt,rev,revActor) VALUES('c1','One','one',NULL,'manual',NULL,'manual','manual','2026-01-01T00:00:00.000Z','2026-01-01T00:00:00.000Z',3,'seed')", []).unwrap();
    conn.execute("INSERT INTO collection_members(id,collectionId,recordId,position,sourceKind,createdAt,updatedAt,rev,revActor) VALUES(?1,'c1','r1',0,'manual','2026-01-01T00:00:00.000Z','2026-01-01T00:00:00.000Z',3,'seed')", [&id]).unwrap();
    id
}

fn baseline(conn: &Connection) {
    let value = json!({"records":db::get_all_records(conn).unwrap(),
        "collections":crate::collections::all(conn).unwrap(),
        "collectionMembers":crate::collections::all_members(conn).unwrap()});
    db::set_setting(conn, "sync_v3_baseline".into(), value.to_string()).unwrap();
}

fn find(conn: &Connection, kind: &str, id: &str) -> CapturedStagingDescriptorV1 {
    load_staged_descriptors(conn)
        .unwrap()
        .into_iter()
        .find(|row| row.entity_kind == kind && row.entity_id == id)
        .unwrap()
}

fn assert_delete(row: &CapturedStagingDescriptorV1, actor: &str, rev: i64) {
    assert!(row.local.is_none());
    assert_eq!(row.causal_anchor, StagingAnchorStateV1::Unavailable);
    let delete = row.frozen_delete_evidence().unwrap();
    let mutation = map_ordinary_mutation_v1(&OrdinaryMutationRequestV1 {
        local_mutation_id: row.local_mutation_id.clone(),
        payload: OrdinaryPayloadV1::Tombstone(delete.clone()),
        causal_base: OrdinaryCausalBaseV1::Absent,
        base_frontier: vec![],
    })
    .unwrap()
    .unwrap();
    assert_eq!(mutation.value["rev"], rev.to_string());
    assert_eq!(mutation.value["revActor"], actor);
    super::canonical::validate_timestamp(mutation.value["deletedAt"].as_str().unwrap()).unwrap();
    assert_eq!(mutation.changed_fields, ["$tombstone"]);
}

#[test]
fn progress_two_to_five_captures_each_entity_and_repeated_updates_keep_identity() {
    let mut conn = database();
    let enabled = episode_history::enable(&mut conn, "r1", 2, 3, "device").unwrap();
    let advanced =
        episode_history::set_next(&mut conn, "r1", Some(5), enabled.record.rev, "device").unwrap();
    assert_eq!(advanced.completions.len(), 3);
    let rows = load_staged_descriptors(&conn)
        .unwrap()
        .into_iter()
        .filter(|row| row.entity_kind == "episode-completion")
        .collect::<Vec<_>>();
    assert_eq!(rows.len(), 3);
    let fourth = advanced
        .completions
        .iter()
        .find(|row| row.episode_number == 4)
        .unwrap();
    let initial = find(&conn, "episode-completion", &fourth.id);
    episode_history::uncomplete(&mut conn, "r1", 4, fourth.rev, "device").unwrap();
    let uncompleted = find(&conn, "episode-completion", &fourth.id);
    assert!(uncompleted.local.as_ref().unwrap()["completedAt"].is_null());
    assert!(uncompleted.delete_descriptor.is_none());
    let retreated =
        episode_history::set_next(&mut conn, "r1", Some(4), advanced.record.rev, "device").unwrap();
    episode_history::set_next(&mut conn, "r1", Some(5), retreated.record.rev, "device").unwrap();
    let updated = find(&conn, "episode-completion", &fourth.id);
    assert_eq!(initial.local_mutation_id, updated.local_mutation_id);
    assert_eq!(initial.first_generation, updated.first_generation);
    assert!(updated.last_generation > initial.last_generation);
    assert_eq!(initial.causal_anchor, updated.causal_anchor);
    assert!(sync_staging::get_staging(&conn)
        .unwrap()
        .entries
        .iter()
        .all(|row| row.entity_kind != "episode-completion"));
}

#[test]
fn episode_true_delete_keeps_composite_identity_and_survives_s1_cleanup_and_restart() {
    let path = std::env::temp_dir().join(format!("i61-capture-{}.sqlite", uuid::Uuid::new_v4()));
    let mut conn = Connection::open(&path).unwrap();
    db::setup_db(&conn).unwrap();
    db::insert_record(&conn, record("r1")).unwrap();
    let enabled = episode_history::enable(&mut conn, "r1", 2, 3, "device").unwrap();
    let advanced =
        episode_history::set_next(&mut conn, "r1", Some(5), enabled.record.rev, "device").unwrap();
    let episode = &advanced.completions[0];
    let initial = find(&conn, "episode-completion", &episode.id);
    episode_history::delete_completion(
        &mut conn,
        "r1",
        episode.episode_number,
        episode.rev,
        "delete-actor",
    )
    .unwrap();
    sync_staging::set_staging(&conn, &sync_staging::SyncStaging::default()).unwrap();
    let deleted = find(&conn, "episode-completion", &episode.id);
    assert_delete(&deleted, "delete-actor", episode.rev + 1);
    assert_eq!(deleted.local_mutation_id, initial.local_mutation_id);
    assert_eq!(deleted.first_generation, initial.first_generation);
    assert_eq!(
        deleted.frozen_delete_evidence().unwrap().entity_key(),
        json!(["episode-completion", "r1", episode.episode_number])
    );
    let id = episode.id.clone();
    drop(conn);
    let conn = Connection::open(&path).unwrap();
    db::setup_db(&conn).unwrap();
    assert_eq!(find(&conn, "episode-completion", &id), deleted);
    drop(conn);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn record_delete_captures_record_member_and_episode_before_cascade() {
    let mut conn = database();
    let member = seed_collection(&conn);
    baseline(&conn);
    let enabled = episode_history::enable(&mut conn, "r1", 2, 3, "device").unwrap();
    let advanced =
        episode_history::set_next(&mut conn, "r1", Some(3), enabled.record.rev, "device").unwrap();
    db_atomic_crud::delete_record_atomic(&mut conn, "r1", "delete-actor").unwrap();
    assert!(db::get_record(&conn, "r1").unwrap().is_none());
    assert!(episode_history::all_completions(&conn).unwrap().is_empty());
    assert_delete(
        &find(&conn, "record", "r1"),
        "delete-actor",
        advanced.record.rev + 1,
    );
    let member_row = find(&conn, "collection-member", &member);
    assert_delete(&member_row, "delete-actor", 4);
    assert_eq!(
        member_row.frozen_delete_evidence().unwrap().entity_key(),
        json!(["collection-member", "c1", "r1"])
    );
    assert_delete(
        &find(&conn, "episode-completion", &advanced.completions[0].id),
        "delete-actor",
        2,
    );
    let saved = load_staged_descriptors(&conn).unwrap();
    conn.execute("DELETE FROM settings WHERE key='sync_tombstones_v1'", [])
        .unwrap();
    sync_staging::set_staging(&conn, &sync_staging::SyncStaging::default()).unwrap();
    db::setup_db(&conn).unwrap();
    for row in saved
        .into_iter()
        .filter(|row| row.delete_descriptor.is_some())
    {
        assert_eq!(find(&conn, &row.entity_kind, &row.entity_id), row);
    }
}

#[test]
fn collection_delete_and_member_remove_keep_exact_deletion_fields() {
    for remove_member in [false, true] {
        let mut conn = database();
        let member = seed_collection(&conn);
        baseline(&conn);
        if remove_member {
            crate::collections::remove_member(&mut conn, "c1", "r1", 3, "delete-actor").unwrap();
        } else {
            crate::collections::delete(&mut conn, "c1", 3, "delete-actor").unwrap();
        }
        assert_delete(
            &find(&conn, "collection-member", &member),
            "delete-actor",
            4,
        );
        if !remove_member {
            assert_delete(&find(&conn, "collection", "c1"), "delete-actor", 4);
        }
        conn.execute("DELETE FROM collection_tombstones", [])
            .unwrap();
        conn.execute("DELETE FROM collection_member_tombstones", [])
            .unwrap();
        sync_staging::set_staging(&conn, &sync_staging::SyncStaging::default()).unwrap();
        db::setup_db(&conn).unwrap();
        assert_delete(
            &find(&conn, "collection-member", &member),
            "delete-actor",
            4,
        );
    }
}

#[test]
fn bulk_replacements_capture_removed_entities_and_episode_updates() {
    for mode in 0..3 {
        let mut conn = database();
        seed_collection(&conn);
        baseline(&conn);
        let enabled = episode_history::enable(&mut conn, "r1", 2, 3, "device").unwrap();
        let advanced =
            episode_history::set_next(&mut conn, "r1", Some(5), enabled.record.rev, "device")
                .unwrap();
        let first = find(&conn, "episode-completion", &advanced.completions[0].id);
        match mode {
            0 => db_atomic_crud::replace_all_records_atomic(&mut conn, vec![]).unwrap(),
            1 => episode_history::replace_library_atomic(
                &mut conn,
                vec![advanced.record.clone()],
                vec![],
            )
            .unwrap(),
            _ => crate::collections::replace_library_atomic(
                &mut conn,
                vec![advanced.record.clone()],
                vec![],
                vec![],
                vec![],
            )
            .unwrap(),
        }
        let deleted = find(&conn, "episode-completion", &advanced.completions[0].id);
        assert!(deleted.frozen_delete_evidence().is_ok());
        assert_eq!(deleted.local_mutation_id, first.local_mutation_id);
        assert_eq!(deleted.first_generation, first.first_generation);
        if mode == 0 {
            assert!(find(&conn, "record", "r1").frozen_delete_evidence().is_ok());
        }
        if mode == 2 {
            assert!(find(&conn, "collection", "c1")
                .frozen_delete_evidence()
                .is_ok());
        }
    }
}

#[test]
fn capture_failure_rolls_back_progress_and_delete_without_one_sided_state() {
    let mut conn = database();
    baseline(&conn);
    let enabled = episode_history::enable(&mut conn, "r1", 2, 3, "device").unwrap();
    let before_record = serde_json::to_value(db::get_record(&conn, "r1").unwrap()).unwrap();
    let before_staging = sync_staging::get_staging(&conn).unwrap();
    let before_authority = load_staged_descriptors(&conn).unwrap();
    let generation = crate::db_atomic_helpers::get_records_generation(&conn).unwrap();
    conn.execute_batch("CREATE TRIGGER fail_s2_capture BEFORE INSERT ON s2_lite_local_staging_descriptor_v1 WHEN NEW.entity_kind='episode-completion' BEGIN SELECT RAISE(ABORT,'injected capture failure'); END;").unwrap();
    assert!(
        episode_history::set_next(&mut conn, "r1", Some(5), enabled.record.rev, "device").is_err()
    );
    assert_eq!(
        serde_json::to_value(db::get_record(&conn, "r1").unwrap()).unwrap(),
        before_record
    );
    assert!(episode_history::all_completions(&conn).unwrap().is_empty());
    assert_eq!(sync_staging::get_staging(&conn).unwrap(), before_staging);
    assert_eq!(load_staged_descriptors(&conn).unwrap(), before_authority);
    assert_eq!(
        crate::db_atomic_helpers::get_records_generation(&conn).unwrap(),
        generation
    );
    conn.execute_batch("DROP TRIGGER fail_s2_capture; CREATE TRIGGER fail_record_delete BEFORE DELETE ON records BEGIN SELECT RAISE(ABORT,'injected business failure'); END;").unwrap();
    assert!(db_atomic_crud::delete_record_atomic(&mut conn, "r1", "device").is_err());
    assert_eq!(
        serde_json::to_value(db::get_record(&conn, "r1").unwrap()).unwrap(),
        before_record
    );
    assert_eq!(sync_staging::get_staging(&conn).unwrap(), before_staging);
    assert_eq!(load_staged_descriptors(&conn).unwrap(), before_authority);
    assert_eq!(
        crate::db_atomic_helpers::get_records_generation(&conn).unwrap(),
        generation
    );
    conn.execute_batch("DROP TRIGGER fail_record_delete; CREATE TRIGGER fail_s1_staging BEFORE INSERT ON settings WHEN NEW.key='sync_staging_v1' BEGIN SELECT RAISE(ABORT,'injected S1 staging failure'); END;").unwrap();
    assert!(
        episode_history::set_next(&mut conn, "r1", Some(5), enabled.record.rev, "device").is_err()
    );
    assert_eq!(
        serde_json::to_value(db::get_record(&conn, "r1").unwrap()).unwrap(),
        before_record
    );
    assert!(episode_history::all_completions(&conn).unwrap().is_empty());
    assert_eq!(sync_staging::get_staging(&conn).unwrap(), before_staging);
    assert_eq!(load_staged_descriptors(&conn).unwrap(), before_authority);
    assert_eq!(
        crate::db_atomic_helpers::get_records_generation(&conn).unwrap(),
        generation
    );
}

#[test]
fn all_delete_types_survive_real_restart_with_s1_evidence_erased() {
    let path = std::env::temp_dir().join(format!("i61-deletes-{}.sqlite", uuid::Uuid::new_v4()));
    let mut conn = Connection::open(&path).unwrap();
    db::setup_db(&conn).unwrap();
    db::insert_record(&conn, record("r1")).unwrap();
    let member = seed_collection(&conn);
    baseline(&conn);
    let enabled = episode_history::enable(&mut conn, "r1", 2, 3, "device").unwrap();
    episode_history::set_next(&mut conn, "r1", Some(3), enabled.record.rev, "device").unwrap();
    db_atomic_crud::delete_record_atomic(&mut conn, "r1", "delete-actor").unwrap();
    let collection_rev = crate::collections::all(&conn).unwrap()[0].rev;
    crate::collections::delete(&mut conn, "c1", collection_rev, "delete-actor").unwrap();
    conn.execute_batch("DELETE FROM collection_tombstones; DELETE FROM collection_member_tombstones; DELETE FROM settings WHERE key IN ('sync_tombstones_v1','sync_v3_baseline');").unwrap();
    sync_staging::set_staging(&conn, &sync_staging::SyncStaging::default()).unwrap();
    let saved = load_staged_descriptors(&conn).unwrap();
    assert_eq!(saved.len(), 4);
    assert!(saved.iter().all(|row| row.frozen_delete_evidence().is_ok()));
    drop(conn);
    let conn = Connection::open(&path).unwrap();
    db::setup_db(&conn).unwrap();
    assert_eq!(load_staged_descriptors(&conn).unwrap(), saved);
    assert_eq!(
        find(&conn, "collection-member", &member)
            .frozen_delete_evidence()
            .unwrap()
            .entity_key(),
        json!(["collection-member", "c1", "r1"])
    );
    drop(conn);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn completion_replacement_is_live_uncompletion_and_reuses_first_identity() {
    let mut conn = database();
    let enabled = episode_history::enable(&mut conn, "r1", 2, 3, "device").unwrap();
    let advanced =
        episode_history::set_next(&mut conn, "r1", Some(3), enabled.record.rev, "device").unwrap();
    let id = advanced.completions[0].id.clone();
    let initial = find(&conn, "episode-completion", &id);
    let mut items = advanced.completions;
    items[0].completed_at = None;
    items[0].rev += 1;
    episode_history::replace_library_atomic(&mut conn, vec![advanced.record], items).unwrap();
    let updated = find(&conn, "episode-completion", &id);
    assert!(updated.delete_descriptor.is_none());
    assert!(updated.local.as_ref().unwrap()["completedAt"].is_null());
    assert_eq!(updated.local_mutation_id, initial.local_mutation_id);
    assert_eq!(updated.first_generation, initial.first_generation);
    assert_eq!(updated.causal_anchor, initial.causal_anchor);
}

#[test]
fn local_create_delete_still_cancels_and_keeps_writer_identity() {
    let mut conn = database();
    db_atomic_crud::insert_record_atomic(&mut conn, record("new"), "device").unwrap();
    let writer = super::local_authority::load_writer(&conn).unwrap();
    db_atomic_crud::delete_record_atomic(&mut conn, "new", "device").unwrap();
    assert!(load_staged_descriptors(&conn)
        .unwrap()
        .iter()
        .all(|row| row.entity_id != "new"));
    assert!(sync_staging::get_staging(&conn)
        .unwrap()
        .entries
        .iter()
        .all(|row| row.id != "new"));
    assert_eq!(super::local_authority::load_writer(&conn).unwrap(), writer);
}

#[test]
fn malformed_delete_evidence_fails_closed_and_float_before_image_keeps_bits() {
    let mut conn = database();
    let mut r = record("r1");
    r.imdb_rating = Some(f64::from_bits(0x4002_a56c_6532_2d7e));
    db::insert_record(&conn, r).unwrap();
    baseline(&conn);
    db_atomic_crud::delete_record_atomic(&mut conn, "r1", "device").unwrap();
    let row = find(&conn, "record", "r1");
    assert_eq!(
        row.base.as_ref().unwrap()["imdbRating"]
            .as_f64()
            .unwrap()
            .to_bits(),
        0x4002_a56c_6532_2d7e
    );
    assert_eq!(row.state_version, 2);
    let original = serde_json::to_value(row).unwrap();
    let mut bad_revision = original.clone();
    bad_revision["deleteDescriptor"]["rev"] = json!(-1);
    let mut missing_evidence = original;
    missing_evidence
        .as_object_mut()
        .unwrap()
        .remove("deleteDescriptor");
    for bad in [bad_revision, missing_evidence] {
        conn.execute(
            "UPDATE s2_lite_local_staging_descriptor_v1 SET state_json=?1 WHERE entity_kind='record'",
            params![serde_json::to_vec(&bad).unwrap()],
        )
        .unwrap();
        assert!(load_staged_descriptors(&conn).is_err());
        assert!(db::setup_db(&conn).is_err());
    }
}
