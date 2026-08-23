use crate::db;
use crate::db_atomic_helpers::{
    get_tombstones_tx, mark_local_records_mutated, set_tombstones_tx, Tombstone,
};
use crate::error::AppError;
use crate::models::WatchRecord;
use crate::record_validation::{prepare_import_batch, prepare_record, RecordWriteContext};
use chrono::Utc;
use rusqlite::{Connection, OptionalExtension};
use std::collections::{HashMap, HashSet};

pub fn insert_record_atomic(
    conn: &mut Connection,
    record: WatchRecord,
    actor_id: &str,
) -> Result<WatchRecord, AppError> {
    if actor_id.trim().is_empty() {
        return Err(AppError::General("Missing revision actor ID".to_string()));
    }
    let mut record = prepare_record(record, RecordWriteContext::Local)?;
    let transaction = conn.transaction()?;
    let id = record.id.clone();
    let previous_revision = transaction
        .query_row("SELECT rev FROM records WHERE id = ?1", [&id], |row| {
            row.get::<_, i64>(0)
        })
        .optional()?
        .unwrap_or(0);
    record.updated_at = Some(Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true));
    record.rev = previous_revision
        .checked_add(1)
        .ok_or_else(|| AppError::General("Record revision overflow".to_string()))?;
    record.rev_actor = actor_id.to_string();
    db::insert_record(&transaction, record)?;

    let mut tombstones = get_tombstones_tx(&transaction)?;
    if tombstones.iter().any(|tombstone| tombstone.id == id) {
        tombstones.retain(|tombstone| tombstone.id != id);
        set_tombstones_tx(&transaction, &tombstones)?;
    }
    let generation = mark_local_records_mutated(&transaction, "record-insert")?;
    let persisted = db::get_record(&transaction, &id)?
        .ok_or_else(|| AppError::General(format!("Record not found after upsert: {id}")))?;
    crate::sync_staging::stage_upsert(&transaction, &persisted, generation)?;
    transaction.commit()?;
    Ok(persisted)
}

pub fn delete_record_atomic(
    conn: &mut Connection,
    id: &str,
    actor_id: &str,
) -> Result<(), AppError> {
    if actor_id.trim().is_empty() {
        return Err(AppError::General("Missing revision actor ID".to_string()));
    }
    let transaction = conn.transaction()?;
    let previous_revision = transaction.query_row(
        "SELECT COALESCE(rev, 0) FROM records WHERE id = ?1",
        [id],
        |row| row.get::<_, i64>(0),
    )?;
    let (removed_collection_members, changed_collections) =
        crate::collections::detach_record_tx(&transaction, id, actor_id)?;
    transaction.execute("DELETE FROM episode_completions WHERE recordId = ?1", [id])?;
    if transaction.execute("DELETE FROM records WHERE id = ?1", [id])? == 0 {
        return Err(AppError::General(format!("Record not found: {id}")));
    }

    let mut tombstones = get_tombstones_tx(&transaction)?;
    tombstones.retain(|tombstone| tombstone.id != id);
    tombstones.push(Tombstone {
        id: id.to_string(),
        deleted_at: Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        rev: previous_revision
            .checked_add(1)
            .ok_or_else(|| AppError::General("Record revision overflow".to_string()))?,
        rev_actor: actor_id.to_string(),
    });
    set_tombstones_tx(&transaction, &tombstones)?;
    let generation = mark_local_records_mutated(&transaction, "record-delete")?;
    crate::sync_staging::stage_delete(&transaction, id, generation)?;
    for member_id in removed_collection_members {
        crate::sync_staging::stage_entity_delete(
            &transaction,
            "collection-member",
            &member_id,
            generation,
        )?;
    }
    for collection in changed_collections {
        let collection_id = collection.id.clone();
        crate::sync_staging::stage_entity_upsert(
            &transaction,
            "collection",
            &collection_id,
            serde_json::to_value(collection)
                .map_err(|error| AppError::General(error.to_string()))?,
            generation,
        )?;
    }
    transaction.commit()?;
    Ok(())
}

pub fn replace_all_records_atomic(
    conn: &mut Connection,
    records: Vec<WatchRecord>,
) -> Result<(), AppError> {
    let transaction = conn.transaction()?;
    let previous_collection_members = crate::collections::all_members(&transaction)?;
    let actor = crate::sync_state::device_id(&transaction)?;
    let previous_completions = crate::episode_history::all_completions(&transaction)?;
    let existing_tracking = db::get_all_records(&transaction)?
        .into_iter()
        .map(|record| {
            (
                record.id.clone(),
                (record.episode_tracking_enabled, record.next_episode),
            )
        })
        .collect::<HashMap<_, _>>();
    let locked_ids = {
        let mut statement = transaction.prepare("SELECT id FROM records WHERE isLocked = 1")?;
        let ids = statement
            .query_map([], |row| row.get(0))?
            .collect::<Result<HashSet<String>, _>>()?;
        ids
    };
    let mut records = prepare_import_batch(
        records
            .into_iter()
            .filter(|record| !locked_ids.contains(record.id.trim()))
            .collect(),
    )?;
    for record in &mut records {
        if let Some((enabled, next_episode)) = existing_tracking.get(&record.id) {
            record.episode_tracking_enabled = *enabled;
            record.next_episode = *next_episode;
        }
    }
    db::replace_all_records_tx(&transaction, records)?;
    crate::collections::reconcile_after_record_replace_tx(
        &transaction,
        &previous_collection_members,
        &actor,
    )?;
    let retained_ids = db::get_all_records(&transaction)?
        .into_iter()
        .map(|record| record.id)
        .collect::<HashSet<_>>();
    let retained_completions = previous_completions
        .into_iter()
        .filter(|item| retained_ids.contains(&item.record_id))
        .collect::<Vec<_>>();
    crate::episode_history::replace_completions_tx(
        &transaction,
        &retained_completions,
        &locked_ids,
    )?;
    let generation = mark_local_records_mutated(&transaction, "records-replace")?;
    crate::sync_staging::rebuild_from_current(&transaction, generation)?;
    transaction.commit()?;
    Ok(())
}
