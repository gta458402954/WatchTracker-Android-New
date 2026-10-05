//! Local-only admission for a frozen legacy bootstrap migration.
//!
//! This module translates the four production business entities through the
//! existing ordinary frozen scalar mapping. It deliberately has no remote,
//! activation, or ordinary-writer collaborator.

use std::sync::Mutex;

use rusqlite::Connection;
use serde_json::Value;

use super::canonical::{ProtocolError, Result};
use super::durable_persistence::{
    MigrationAdmissionInputV1, MigrationAdmissionResultV1, SqliteS2LiteStoreV1,
};
use super::migration_orchestration::{
    capture_legacy_snapshot_v1, CapturedLegacySnapshotV1, LegacySnapshotEntryV1,
};
use super::ordinary_mutation::{
    LocalCollectionMemberV1, LocalCollectionV1, LocalEntityValueV1, LocalEpisodeCompletionV1,
    LocalRecordV1,
};
use super::semantic::validate_native_entity;
use super::target_root_binding::{
    load_historical_target_root_binding_v1, resolve_active_target_root_binding_v1,
};
use super::types::{BootstrapEntity, LegacySemanticAdapterV1};

const ADMISSION_FAILURE: ProtocolError = ProtocolError("S2_MIGRATION_ADMISSION_FAILURE");

/// The production adapter is intentionally typed: source rows are converted
/// to their established local entity representation before the same frozen
/// wire/value validation used by ordinary S2 mutations.
#[derive(Clone, Copy, Debug, Default)]
pub struct ProductionLegacySemanticAdapterV1;

impl LegacySemanticAdapterV1 for ProductionLegacySemanticAdapterV1 {
    fn adapt_live_entity(
        &self,
        entity_type: &str,
        legacy_value: &Value,
    ) -> std::result::Result<BootstrapEntity, String> {
        let local = match entity_type {
            "record" => LocalEntityValueV1::Record(Box::new(
                serde_json::from_value::<LocalRecordV1>(legacy_value.clone())
                    .map_err(|_| "invalid_record".to_string())?,
            )),
            "episode-completion" => LocalEntityValueV1::EpisodeCompletion(
                serde_json::from_value::<LocalEpisodeCompletionV1>(legacy_value.clone())
                    .map_err(|_| "invalid_episode_completion".to_string())?,
            ),
            "collection" => LocalEntityValueV1::Collection(
                serde_json::from_value::<LocalCollectionV1>(legacy_value.clone())
                    .map_err(|_| "invalid_collection".to_string())?,
            ),
            "collection-member" => LocalEntityValueV1::CollectionMember(
                serde_json::from_value::<LocalCollectionMemberV1>(legacy_value.clone())
                    .map_err(|_| "invalid_collection_member".to_string())?,
            ),
            _ => return Err("unsupported_entity_type".to_string()),
        };
        let entity = BootstrapEntity {
            entity_type: local.entity_type().to_string(),
            entity_key: local.entity_key(),
            value: local
                .wire_value()
                .map_err(|_| "invalid_entity".to_string())?,
        };
        validate_native_entity(&entity.entity_key, &entity.value)
            .map_err(|_| "invalid_entity".to_string())?;
        Ok(entity)
    }
}

fn record_status(value: &crate::models::RecordStatus) -> &'static str {
    match value {
        crate::models::RecordStatus::Watched => "已看",
        crate::models::RecordStatus::Watching => "在看",
        crate::models::RecordStatus::Unwatched => "未看",
    }
}

fn record(value: crate::models::WatchRecord) -> LocalRecordV1 {
    LocalRecordV1 {
        id: value.id,
        original_name: value.original_name,
        chinese_name: value.chinese_name,
        progress: value.progress,
        total_episodes: value.total_episodes,
        episode_tracking_enabled: value.episode_tracking_enabled,
        next_episode: value.next_episode,
        movie_progress: value.movie_progress,
        movie_duration: value.movie_duration,
        release_year: value.release_year,
        poster_path: value.poster_path,
        status: record_status(&value.status).to_string(),
        platform: value.platform,
        rating: value.rating,
        start_date: value.start_date,
        end_date: value.end_date,
        notes: value.notes,
        created_at: value.created_at,
        updated_at: value.updated_at,
        imdb_id: value.imdb_id,
        is_locked: value.is_locked,
        genres: value.genres,
        origin_country: value.origin_country,
        imdb_rating: value.imdb_rating,
        tmdb_status: value.tmdb_status,
        interest_level: value.interest_level,
        episode_runtime: value.episode_runtime,
        media_type: value.media_type,
        content_tags: value.content_tags,
        tmdb_media_kind: value.tmdb_media_kind,
        tmdb_id: value.tmdb_id,
        tmdb_parent_id: value.tmdb_parent_id,
        tmdb_season_number: value.tmdb_season_number,
        series_record_kind: value.series_record_kind,
        rev: value.rev,
        rev_actor: value.rev_actor,
    }
}

fn collection(value: crate::collections::Collection) -> LocalCollectionV1 {
    LocalCollectionV1 {
        id: value.id,
        name: value.name,
        normalized_name: value.normalized_name,
        description: value.description,
        source_kind: value.source_kind,
        source_key: value.source_key,
        collection_kind: value.collection_kind,
        order_mode: value.order_mode,
        created_at: value.created_at,
        updated_at: value.updated_at,
        rev: value.rev,
        rev_actor: value.rev_actor,
    }
}

fn member(value: crate::collections::CollectionMember) -> LocalCollectionMemberV1 {
    LocalCollectionMemberV1 {
        id: value.id,
        collection_id: value.collection_id,
        record_id: value.record_id,
        position: value.position,
        source_kind: value.source_kind,
        created_at: value.created_at,
        updated_at: value.updated_at,
        rev: value.rev,
        rev_actor: value.rev_actor,
    }
}

fn episode(value: crate::episode_history::EpisodeCompletion) -> LocalEpisodeCompletionV1 {
    LocalEpisodeCompletionV1 {
        id: value.id,
        record_id: value.record_id,
        episode_number: value.episode_number,
        completed_at: value.completed_at,
        created_at: value.created_at,
        updated_at: value.updated_at,
        rev: value.rev,
        rev_actor: value.rev_actor,
    }
}

/// Reads only the four frozen bootstrap entity classes from one SQLite view.
pub fn capture_production_legacy_snapshot_v1(
    conn: &Connection,
) -> Result<(i64, CapturedLegacySnapshotV1)> {
    let mut entries = Vec::new();
    for value in crate::db::get_all_records(conn).map_err(|_| ADMISSION_FAILURE)? {
        entries.push(LegacySnapshotEntryV1 {
            entity_type: "record".to_string(),
            value: serde_json::to_value(record(value)).map_err(|_| ADMISSION_FAILURE)?,
        });
    }
    for value in crate::episode_history::all_completions(conn).map_err(|_| ADMISSION_FAILURE)? {
        entries.push(LegacySnapshotEntryV1 {
            entity_type: "episode-completion".to_string(),
            value: serde_json::to_value(episode(value)).map_err(|_| ADMISSION_FAILURE)?,
        });
    }
    for value in crate::collections::all(conn).map_err(|_| ADMISSION_FAILURE)? {
        entries.push(LegacySnapshotEntryV1 {
            entity_type: "collection".to_string(),
            value: serde_json::to_value(collection(value)).map_err(|_| ADMISSION_FAILURE)?,
        });
    }
    for value in crate::collections::all_members(conn).map_err(|_| ADMISSION_FAILURE)? {
        entries.push(LegacySnapshotEntryV1 {
            entity_type: "collection-member".to_string(),
            value: serde_json::to_value(member(value)).map_err(|_| ADMISSION_FAILURE)?,
        });
    }
    let generation =
        crate::db_atomic_helpers::get_records_generation(conn).map_err(|_| ADMISSION_FAILURE)?;
    let snapshot = capture_legacy_snapshot_v1(&entries, &ProductionLegacySemanticAdapterV1)?;
    Ok((generation, snapshot))
}

/// Admits a migration against the current active target and captures the
/// immutable production snapshot. The store revalidates the active/bound
/// identity in its single `BEGIN IMMEDIATE` transaction before capture.
pub fn admit_and_capture_migration_v1(
    conn: &Mutex<Connection>,
    target_id: &str,
    target_epoch: u64,
    migration_id: &str,
    migration_writer_id: &str,
    created_at: &str,
) -> Result<MigrationAdmissionResultV1> {
    // A durable historical binding is sufficient only for re-attaching to an
    // already-owned guard; the store still requires a current active binding
    // before it can perform a fresh capture.
    let binding = load_historical_target_root_binding_v1(conn, target_id, target_epoch)?
        .map_or_else(
            || {
                resolve_active_target_root_binding_v1(conn, target_id, target_epoch)
                    .map(|bound| bound.binding)
            },
            Ok,
        )?;
    let root_id = binding.physical_root_id.clone();
    let mut store = SqliteS2LiteStoreV1::open(conn, &root_id)?;
    store.admit_and_capture_migration_v1(
        &MigrationAdmissionInputV1 {
            target_binding: binding,
            migration_id: migration_id.to_string(),
            migration_writer_id: migration_writer_id.to_string(),
            created_at: created_at.to_string(),
        },
        capture_production_legacy_snapshot_v1,
    )
}
