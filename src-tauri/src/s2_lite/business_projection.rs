//! Explicit remote S2 business-table writes. None of these functions stage a
//! local mutation, advance `records_generation`, or create S1 publish state.

use std::collections::BTreeSet;

use rusqlite::{params, Connection};
use serde_json::Value;

use crate::collections::{Collection, CollectionMember};
use crate::episode_history::EpisodeCompletion;
use crate::error::AppError;
use crate::models::WatchRecord;

use super::canonical::{jcs_bytes, ProtocolError, Result as ProtocolResult};
use super::durable_persistence::{
    BusinessProjectionTransactionResultV1, DurableMaterializedProjectionV1, SqliteS2LiteStoreV1,
};
use super::materialized_projection::MaterializedProjectionEntityV1;

/// Local-only diagnostic for every canonical entity processed by the business
/// projector. These outcomes have no protocol or receipt authority.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BusinessProjectionOutcomeV1 {
    AppliedRemoteValue,
    AppliedRemoteTombstone,
    OverlayPreserved,
    ConflictPreserved,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BusinessProjectionReportV1 {
    pub generation: u64,
    pub outcomes: Vec<BusinessProjectionOutcomeV1>,
}

fn failure(_: crate::error::AppError) -> ProtocolError {
    ProtocolError("business_projection_failed")
}

fn key_id(key: &Value) -> ProtocolResult<Vec<u8>> {
    jcs_bytes(key)
}

/// Returns the active target only when it is bound to this projector's exact
/// physical root.  Legacy/no-target operation retains its pre-S2 behavior;
/// target-scoped overlay provenance is required only once target authority is
/// present.
fn active_target_for_root_v1(conn: &Connection, root_id: &str) -> ProtocolResult<Option<String>> {
    let Some(registry) = crate::sync_targets::registry(conn).map_err(failure)? else {
        return Ok(None);
    };
    let Some(target_id) = registry.active_target_id else {
        return Ok(None);
    };
    let target = registry
        .targets
        .iter()
        .find(|target| target.id == target_id)
        .ok_or(ProtocolError("projector_active_target_unavailable"))?;
    let root = super::webdav_adapter::webdav_root_v1(&target.normalized_url, &target.username)
        .map_err(|_| ProtocolError("projector_target_root_invalid"))?;
    if root.physical_root_id != root_id {
        return Err(ProtocolError("projector_target_root_mismatch"));
    }
    Ok(Some(target_id))
}

fn key_parts(key: &Value) -> ProtocolResult<(&str, &[Value])> {
    let values = key
        .as_array()
        .ok_or(ProtocolError("projector_invalid_entity_key"))?;
    let (kind, rest) = values
        .split_first()
        .ok_or(ProtocolError("projector_invalid_entity_key"))?;
    Ok((
        kind.as_str()
            .ok_or(ProtocolError("projector_invalid_entity_key"))?,
        rest,
    ))
}

fn key_string(value: &Value) -> ProtocolResult<&str> {
    value
        .as_str()
        .ok_or(ProtocolError("projector_invalid_entity_key"))
}

fn key_episode(value: &Value) -> ProtocolResult<i32> {
    value
        .as_i64()
        .and_then(|value| i32::try_from(value).ok())
        .filter(|value| *value > 0)
        .ok_or(ProtocolError("projector_invalid_entity_key"))
}

/// Rehydrate only row metadata from retained, verified replay. The reducer's
/// business value remains unchanged; no mutable local row is used as evidence.
fn local_live_value(
    conn: &Connection,
    root: &str,
    entity: &MaterializedProjectionEntityV1,
) -> ProtocolResult<Value> {
    let read = super::discovery_persistence::load_read_state_v1(conn, root)?
        .ok_or(ProtocolError("projector_replay_missing"))?;
    let entry = read.projection.replay["materialized"]
        .as_array()
        .and_then(|rows| {
            rows.iter()
                .find(|row| row["entityKey"] == entity.entity_key)
        })
        .ok_or(ProtocolError("projector_replay_missing"))?;
    let resolved = &entry["value"];
    if resolved["state"] != "Resolved"
        || resolved["businessValue"] != entity.business_value.clone().unwrap_or(Value::Null)
    {
        return Err(ProtocolError("projector_replay_mismatch"));
    }
    // Frozen replay sorts variants by commit ref. Retain all alternatives in
    // authority and select the last variant deterministically for SQLite metadata.
    let metadata = resolved["metadataVariants"]
        .as_array()
        .and_then(|rows| rows.last())
        .and_then(|row| row["metadata"].as_object())
        .ok_or(ProtocolError("projector_metadata_missing"))?;
    let mut value = entity
        .business_value
        .clone()
        .ok_or(ProtocolError("projector_live_value_missing"))?;
    let map = value
        .as_object_mut()
        .ok_or(ProtocolError("projector_live_value_invalid"))?;
    for (key, value) in metadata {
        map.insert(key.clone(), value.clone());
    }
    super::semantic::validate_native_entity(&entity.entity_key, &value)?;
    for field in ["rev", "position", "tmdbId", "tmdbParentId"] {
        if let Some(item) = value.get_mut(field) {
            if !item.is_null() {
                *item = Value::from(super::canonical::parse_int64_decimal(
                    item.as_str()
                        .ok_or(ProtocolError("projector_scalar_invalid"))?,
                    i64::MIN,
                    i64::MAX,
                )?);
            }
        }
    }
    Ok(value)
}

fn apply_live(
    conn: &Connection,
    root: &str,
    entity: &MaterializedProjectionEntityV1,
) -> ProtocolResult<()> {
    let value = local_live_value(conn, root, entity)?;
    let (kind, key) = key_parts(&entity.entity_key)?;
    match kind {
        "record" if key.len() == 1 => remote_upsert_record_no_stage_tx(
            conn,
            serde_json::from_value(value)
                .map_err(|_| ProtocolError("projector_record_value_invalid"))?,
        )
        .map_err(failure),
        "collection" if key.len() == 1 => remote_upsert_collection_no_stage_tx(
            conn,
            &serde_json::from_value(value)
                .map_err(|_| ProtocolError("projector_collection_value_invalid"))?,
        )
        .map_err(failure),
        "collection-member" if key.len() == 2 => remote_upsert_member_no_stage_tx(
            conn,
            &serde_json::from_value(value)
                .map_err(|_| ProtocolError("projector_member_value_invalid"))?,
        )
        .map_err(failure),
        "episode-completion" if key.len() == 2 => remote_upsert_episode_completion_no_stage_tx(
            conn,
            &serde_json::from_value(value)
                .map_err(|_| ProtocolError("projector_completion_value_invalid"))?,
        )
        .map_err(failure),
        _ => Err(ProtocolError("projector_invalid_entity_key")),
    }
}

fn apply_tombstone(
    conn: &Connection,
    entity: &MaterializedProjectionEntityV1,
) -> ProtocolResult<()> {
    apply_absent_entity_key(conn, &entity.entity_key)
}

fn apply_absent_entity_key(conn: &Connection, entity_key: &Value) -> ProtocolResult<()> {
    let (kind, key) = key_parts(entity_key)?;
    match kind {
        "record" if key.len() == 1 => {
            remote_delete_record_no_stage_tx(conn, key_string(&key[0])?).map_err(failure)
        }
        "collection" if key.len() == 1 => {
            remote_delete_collection_no_stage_tx(conn, key_string(&key[0])?).map_err(failure)
        }
        "collection-member" if key.len() == 2 => {
            remote_delete_member_no_stage_tx(conn, key_string(&key[0])?, key_string(&key[1])?)
                .map_err(failure)
        }
        "episode-completion" if key.len() == 2 => remote_delete_episode_completion_no_stage_tx(
            conn,
            key_string(&key[0])?,
            key_episode(&key[1])?,
        )
        .map_err(failure),
        _ => Err(ProtocolError("projector_invalid_entity_key")),
    }
}

fn entity_from_current_projection<'a>(
    projection: &'a DurableMaterializedProjectionV1,
    entity_key: &Value,
) -> ProtocolResult<Option<&'a MaterializedProjectionEntityV1>> {
    let id = key_id(entity_key)?;
    if projection
        .state
        .relation_blocked_entity_keys
        .iter()
        .any(|key| key_id(key).is_ok_and(|candidate| candidate == id))
    {
        return Err(ProtocolError("projector_resolution_conflict"));
    }
    let entity = projection
        .state
        .entities
        .iter()
        .find(|entity| key_id(&entity.entity_key).is_ok_and(|candidate| candidate == id));
    if entity.is_some_and(|entity| entity.conflict) {
        return Err(ProtocolError("projector_resolution_conflict"));
    }
    Ok(entity)
}

/// Applies exactly one complete durable projection. Local staging is an
/// overlay: it blocks the matching canonical entity but is never modified or
/// acknowledged here.
pub fn apply_complete_projection_v1(
    store: &mut SqliteS2LiteStoreV1<'_>,
    expected_projection_generation: u64,
) -> ProtocolResult<(
    BusinessProjectionTransactionResultV1,
    Option<BusinessProjectionReportV1>,
)> {
    store.run_business_projection_transaction(
        expected_projection_generation,
        |conn, projection: &DurableMaterializedProjectionV1| {
            let staging = crate::sync_staging::get_staging(conn).map_err(failure)?;
            let target_id = active_target_for_root_v1(conn, &projection.physical_root_id)?;
            let mut overlays = staging
                .entries
                .iter()
                .map(|entry| {
                    crate::sync_staging::staged_entry_entity_key(entry)
                        .map_err(failure)
                        .and_then(|key| key_id(&key))
                })
                .collect::<ProtocolResult<BTreeSet<_>>>()?;
            // Android keeps episode capture independently of S1 staging.
            // All four local mutation classes are overlays on remote business writes.
            for row in super::local_authority::load_staged_descriptors(conn)? {
                overlays.insert(key_id(&super::local_authority::descriptor_entity_key(
                    &row,
                )?)?);
            }
            let relation_conflicts = projection
                .state
                .relation_blocked_entity_keys
                .iter()
                .map(key_id)
                .collect::<ProtocolResult<BTreeSet<_>>>()?;
            let mut cascade_blocked = BTreeSet::new();
            for entity in &projection.state.entities {
                let id = key_id(&entity.entity_key)?;
                if overlays.contains(&id) || entity.conflict || relation_conflicts.contains(&id) {
                    let (kind, key) = key_parts(&entity.entity_key)?;
                    if kind == "collection-member" {
                        cascade_blocked.insert(key_id(&serde_json::json!(["collection", key[0]]))?);
                        cascade_blocked.insert(key_id(&serde_json::json!(["record", key[1]]))?);
                    } else if kind == "episode-completion" {
                        cascade_blocked.insert(key_id(&serde_json::json!(["record", key[0]]))?);
                    }
                }
            }
            // A staged local child may not exist in canonical replay at all.
            for row in super::local_authority::load_staged_descriptors(conn)? {
                let key = super::local_authority::descriptor_entity_key(&row)?;
                if key[0] == "collection-member" {
                    cascade_blocked.insert(key_id(&serde_json::json!(["collection", key[1]]))?);
                    cascade_blocked.insert(key_id(&serde_json::json!(["record", key[2]]))?);
                } else if key[0] == "episode-completion" {
                    cascade_blocked.insert(key_id(&serde_json::json!(["record", key[1]]))?);
                }
            }
            let mut outcomes = Vec::with_capacity(projection.state.entities.len());
            // SQLite foreign keys require parent upserts before child upserts
            // and child tombstones before parent tombstones. Canonical authority
            // and replay order are unchanged; this is a local SQL dependency order.
            let mut ordered = projection.state.entities.iter().collect::<Vec<_>>();
            ordered.sort_by_key(|entity| {
                let child = matches!(
                    entity.entity_key[0].as_str(),
                    Some("collection-member" | "episode-completion")
                );
                let tombstone = entity
                    .semantic_state
                    .as_ref()
                    .is_some_and(|state| state["state"] == "tombstone");
                match (tombstone, child) {
                    (true, true) => 0,
                    (true, false) => 1,
                    (false, false) => 2,
                    (false, true) => 3,
                }
            });
            for entity in ordered {
                let id = key_id(&entity.entity_key)?;
                let outcome = if entity.conflict || relation_conflicts.contains(&id) {
                    BusinessProjectionOutcomeV1::ConflictPreserved
                } else if overlays.contains(&id)
                    || (cascade_blocked.contains(&id)
                        && entity
                            .semantic_state
                            .as_ref()
                            .is_some_and(|state| state["state"] == "tombstone"))
                {
                    if let Some(target_id) = target_id.as_deref() {
                        super::durable_persistence::upsert_entity_projection_overlay_blocker_v1(
                            conn,
                            &projection.physical_root_id,
                            target_id,
                            expected_projection_generation,
                            entity,
                        )?;
                    }
                    BusinessProjectionOutcomeV1::OverlayPreserved
                } else if entity
                    .semantic_state
                    .as_ref()
                    .is_some_and(|state| state["state"] == "live")
                {
                    apply_live(conn, &projection.physical_root_id, entity)?;
                    if let Some(target_id) = target_id.as_deref() {
                        super::durable_persistence::clear_entity_projection_overlay_blocker_v1(
                            conn,
                            &projection.physical_root_id,
                            target_id,
                            &entity.entity_key,
                        )?;
                    }
                    BusinessProjectionOutcomeV1::AppliedRemoteValue
                } else if entity
                    .semantic_state
                    .as_ref()
                    .is_some_and(|state| state["state"] == "tombstone")
                {
                    apply_tombstone(conn, entity)?;
                    if let Some(target_id) = target_id.as_deref() {
                        super::durable_persistence::clear_entity_projection_overlay_blocker_v1(
                            conn,
                            &projection.physical_root_id,
                            target_id,
                            &entity.entity_key,
                        )?;
                    }
                    BusinessProjectionOutcomeV1::AppliedRemoteTombstone
                } else {
                    return Err(ProtocolError("projector_unresolved_entity"));
                };
                outcomes.push(outcome);
            }
            Ok(BusinessProjectionReportV1 {
                generation: expected_projection_generation,
                outcomes,
            })
        },
    )
}

fn resolve_staged_entity_from_current_projection_inner_v1(
    store: &mut SqliteS2LiteStoreV1<'_>,
    expected_projection_generation: u64,
    entity_key: &Value,
    fail_after_business_write: bool,
) -> ProtocolResult<()> {
    let root_id = store.root_id().to_string();
    store.run_entity_projection_resolution_transaction(
        expected_projection_generation,
        |conn, projection| {
            let target_id = active_target_for_root_v1(conn, &root_id)?
                .ok_or(ProtocolError("projector_active_target_unavailable"))?;
            let mut staging = crate::sync_staging::get_staging(conn).map_err(failure)?;
            let wanted = key_id(entity_key)?;
            let mut position = None;
            for (index, entry) in staging.entries.iter().enumerate() {
                let candidate = crate::sync_staging::staged_entry_entity_key(entry)
                    .map_err(failure)
                    .and_then(|key| key_id(&key))?;
                if candidate == wanted {
                    position = Some(index);
                    break;
                }
            }
            let Some(position) = position else {
                return Err(ProtocolError("projector_resolution_staging_missing"));
            };
            match entity_from_current_projection(projection, entity_key)? {
                Some(entity)
                    if entity
                        .semantic_state
                        .as_ref()
                        .is_some_and(|state| state["state"] == "live") =>
                {
                    apply_live(conn, &projection.physical_root_id, entity)?;
                }
                Some(entity)
                    if entity
                        .semantic_state
                        .as_ref()
                        .is_some_and(|state| state["state"] == "tombstone") =>
                {
                    apply_tombstone(conn, entity)?;
                }
                Some(_) => return Err(ProtocolError("projector_resolution_unresolved_entity")),
                None => apply_absent_entity_key(conn, entity_key)?,
            }
            if fail_after_business_write {
                return Err(ProtocolError("projector_resolution_injected_failure"));
            }
            staging.entries.remove(position);
            crate::sync_staging::set_staging(conn, &staging).map_err(failure)?;
            super::durable_persistence::clear_entity_projection_overlay_blocker_v1(
                conn, &root_id, &target_id, entity_key,
            )?;
            Ok(())
        },
    )
}

/// Explicitly resolves one old staging overlay against the exact current
/// projection.  A plain staging deletion intentionally cannot use this path:
/// the projected entity write, staging removal, and blocker clear commit as
/// one durable transaction.
pub fn resolve_staged_entity_from_current_projection_v1(
    store: &mut SqliteS2LiteStoreV1<'_>,
    expected_projection_generation: u64,
    entity_key: &Value,
) -> ProtocolResult<()> {
    resolve_staged_entity_from_current_projection_inner_v1(
        store,
        expected_projection_generation,
        entity_key,
        false,
    )
}

pub fn remote_upsert_record_no_stage_tx(
    conn: &Connection,
    value: WatchRecord,
) -> Result<(), AppError> {
    crate::db::insert_record(conn, value)?;
    Ok(())
}

pub fn remote_delete_record_no_stage_tx(conn: &Connection, id: &str) -> Result<(), AppError> {
    conn.execute("DELETE FROM records WHERE id=?1", [id])?;
    Ok(())
}

pub fn remote_upsert_collection_no_stage_tx(
    conn: &Connection,
    value: &Collection,
) -> Result<(), AppError> {
    conn.execute(
        "INSERT INTO collections(id,name,normalizedName,description,sourceKind,sourceKey,collectionKind,orderMode,createdAt,updatedAt,rev,revActor)
         VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)
         ON CONFLICT(id) DO UPDATE SET name=excluded.name,normalizedName=excluded.normalizedName,description=excluded.description,sourceKind=excluded.sourceKind,sourceKey=excluded.sourceKey,collectionKind=excluded.collectionKind,orderMode=excluded.orderMode,updatedAt=excluded.updatedAt,rev=excluded.rev,revActor=excluded.revActor",
        params![value.id,value.name,value.normalized_name,value.description,value.source_kind,value.source_key,value.collection_kind,value.order_mode,value.created_at,value.updated_at,value.rev,value.rev_actor],
    )?;
    Ok(())
}

pub fn remote_delete_collection_no_stage_tx(conn: &Connection, id: &str) -> Result<(), AppError> {
    conn.execute("DELETE FROM collection_members WHERE collectionId=?1", [id])?;
    conn.execute("DELETE FROM collections WHERE id=?1", [id])?;
    Ok(())
}

pub fn remote_upsert_member_no_stage_tx(
    conn: &Connection,
    value: &CollectionMember,
) -> Result<(), AppError> {
    conn.execute(
        "INSERT INTO collection_members(id,collectionId,recordId,position,sourceKind,createdAt,updatedAt,rev,revActor)
         VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)
         ON CONFLICT(id) DO UPDATE SET collectionId=excluded.collectionId,recordId=excluded.recordId,position=excluded.position,sourceKind=excluded.sourceKind,updatedAt=excluded.updatedAt,rev=excluded.rev,revActor=excluded.revActor",
        params![value.id,value.collection_id,value.record_id,value.position,value.source_kind,value.created_at,value.updated_at,value.rev,value.rev_actor],
    )?;
    Ok(())
}

pub fn remote_delete_member_no_stage_tx(
    conn: &Connection,
    collection_id: &str,
    record_id: &str,
) -> Result<(), AppError> {
    conn.execute(
        "DELETE FROM collection_members WHERE collectionId=?1 AND recordId=?2",
        params![collection_id, record_id],
    )?;
    Ok(())
}

pub fn remote_upsert_episode_completion_no_stage_tx(
    conn: &Connection,
    value: &EpisodeCompletion,
) -> Result<(), AppError> {
    conn.execute(
        "INSERT INTO episode_completions(id,recordId,episodeNumber,completedAt,createdAt,updatedAt,rev,revActor)
         VALUES(?1,?2,?3,?4,?5,?6,?7,?8)
         ON CONFLICT(recordId,episodeNumber) DO UPDATE SET id=excluded.id,completedAt=excluded.completedAt,updatedAt=excluded.updatedAt,rev=excluded.rev,revActor=excluded.revActor",
        params![value.id,value.record_id,value.episode_number,value.completed_at,value.created_at,value.updated_at,value.rev,value.rev_actor],
    )?;
    Ok(())
}

pub fn remote_delete_episode_completion_no_stage_tx(
    conn: &Connection,
    record_id: &str,
    episode_number: i32,
) -> Result<(), AppError> {
    conn.execute(
        "DELETE FROM episode_completions WHERE recordId=?1 AND episodeNumber=?2",
        params![record_id, episode_number],
    )?;
    Ok(())
}
