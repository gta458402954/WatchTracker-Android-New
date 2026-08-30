use crate::app_paths::AppPaths;
use crate::db::{self, DatabaseCompatibilityIssue, DbState};
use crate::models::{UpdateWatchRecord, WatchRecord};
use crate::net;
use crate::recovery_points;
use crate::sync_state;
use serde_json::Value;
use std::fs;
use tauri::State;

fn lock_database(
    state: &DbState,
) -> Result<std::sync::MutexGuard<'_, rusqlite::Connection>, crate::error::AppError> {
    if let Some(issue) = &state.compatibility_issue {
        return Err(crate::error::AppError::General(format!(
            "database_compatibility:{}:V{}:V{}",
            issue.code, issue.detected_version, issue.supported_version,
        )));
    }
    state
        .conn
        .lock()
        .map_err(|error| crate::error::AppError::ConcurrencyError(error.to_string()))
}

fn validate_webdav_conditions(
    method: &str,
    if_match: Option<&str>,
    if_none_match: Option<&str>,
    if_dav_etag: Option<&str>,
) -> Result<(), crate::error::AppError> {
    if (if_match.is_some() || if_none_match.is_some() || if_dav_etag.is_some()) && method != "PUT" {
        return Err(crate::error::AppError::General(
            "Conditional headers are only allowed for WebDAV PUT".to_string(),
        ));
    }
    if [
        if_match.is_some(),
        if_none_match.is_some(),
        if_dav_etag.is_some(),
    ]
    .into_iter()
    .filter(|present| *present)
    .count()
        > 1
    {
        return Err(crate::error::AppError::General(
            "WebDAV write preconditions cannot be combined".to_string(),
        ));
    }
    if let Some(value) = if_match {
        let valid = value.starts_with('"')
            && value.ends_with('"')
            && value.len() >= 2
            && !value.starts_with("W/")
            && !value.contains(['\r', '\n']);
        if !valid {
            return Err(crate::error::AppError::General(
                "Invalid strong If-Match value".to_string(),
            ));
        }
    }
    if if_none_match.is_some_and(|value| value != "*") {
        return Err(crate::error::AppError::General(
            "Invalid If-None-Match value".to_string(),
        ));
    }
    if let Some(value) = if_dav_etag {
        let opaque = value
            .strip_prefix("W/\"")
            .or_else(|| value.strip_prefix('"'))
            .and_then(|rest| rest.strip_suffix('"'));
        let valid = opaque.is_some_and(|inner| {
            !inner.is_empty()
                && !inner.contains('"')
                && !inner.chars().any(|character| character.is_control())
        });
        if !valid {
            return Err(crate::error::AppError::General(
                "Invalid WebDAV entity tag".to_string(),
            ));
        }
    }
    Ok(())
}

#[tauri::command]
pub fn get_database_compatibility(state: State<DbState>) -> Option<DatabaseCompatibilityIssue> {
    state.compatibility_issue.clone()
}

#[tauri::command]
pub fn get_all_records(state: State<DbState>) -> Result<Vec<WatchRecord>, crate::error::AppError> {
    let conn = lock_database(state.inner())?;
    Ok(db::get_all_records(&conn)?)
}

/// Creates the deterministic M0 WebView poster fixture inside the app
/// sandbox. This is intentionally a tiny test seam; production poster
/// downloads still go through the normal cache command.
#[tauri::command]
pub fn m0_prepare_poster(state: State<AppPaths>) -> Result<String, crate::error::AppError> {
    if !cfg!(debug_assertions) {
        return Err(crate::error::AppError::General("m0_debug_only".to_string()));
    }
    let file_name = "m0-smoke.png";
    let path = state
        .poster_file(file_name)
        .map_err(crate::error::AppError::General)?;
    let bytes = base64::Engine::decode(
        &base64::engine::general_purpose::STANDARD,
        "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=",
    )
    .map_err(|error| crate::error::AppError::General(error.to_string()))?;
    fs::write(path, bytes).map_err(|error| crate::error::AppError::General(error.to_string()))?;
    Ok(file_name.to_string())
}

#[tauri::command]
pub fn insert_record(
    state: State<DbState>,
    r: WatchRecord,
) -> Result<WatchRecord, crate::error::AppError> {
    let mut conn = lock_database(state.inner())?;
    let actor_id = sync_state::device_id(&conn)?;
    crate::db_atomic_crud::insert_record_atomic(&mut conn, r, &actor_id)
}

#[tauri::command]
pub fn update_record(
    state: State<DbState>,
    id: String,
    updates: UpdateWatchRecord,
    actor_id: Option<String>,
) -> Result<WatchRecord, crate::error::AppError> {
    log::info!("[Commands] update_record called for id: {}", id);
    let mut conn = lock_database(state.inner())?;
    let actor_id = match actor_id {
        Some(value) => value,
        None => sync_state::device_id(&conn)?,
    };
    let result = crate::db_atomic_update::update_record_atomic(&mut conn, &id, &updates, &actor_id);
    if let Err(error) = &result {
        log::warn!("[Commands] update_record rejected for id {id}: {error}");
    }
    result
}

#[tauri::command]
pub fn complete_missing_tmdb_identity(
    state: State<DbState>,
    input: crate::metadata_identity::CompleteMissingTmdbIdentityInput,
) -> Result<WatchRecord, crate::error::AppError> {
    let mut conn = lock_database(state.inner())?;
    let actor_id = sync_state::device_id(&conn)?;
    crate::metadata_identity::complete_missing_tmdb_identity(&mut conn, input, &actor_id)
}

#[tauri::command]
pub fn delete_record(state: State<DbState>, id: String) -> Result<(), crate::error::AppError> {
    let mut conn = lock_database(state.inner())?;
    let actor_id = sync_state::device_id(&conn)?;
    crate::db_atomic_crud::delete_record_atomic(&mut conn, &id, &actor_id)
}

#[tauri::command]
pub fn get_collections(
    state: State<DbState>,
) -> Result<Vec<crate::collections::Collection>, crate::error::AppError> {
    let conn = lock_database(state.inner())?;
    crate::collections::all(&conn)
}

#[tauri::command]
pub fn get_collection_members(
    state: State<DbState>,
) -> Result<Vec<crate::collections::CollectionMember>, crate::error::AppError> {
    let conn = lock_database(state.inner())?;
    crate::collections::all_members(&conn)
}

#[tauri::command]
pub fn create_collection(
    state: State<DbState>,
    input: crate::collections::CreateCollectionInput,
) -> Result<crate::collections::Collection, crate::error::AppError> {
    let mut conn = lock_database(state.inner())?;
    let actor = sync_state::device_id(&conn)?;
    crate::collections::create(&mut conn, input, &actor)
}

#[tauri::command]
pub fn create_collection_for_record(
    state: State<DbState>,
    input: crate::collections::CreateCollectionInput,
    record_id: String,
) -> Result<crate::collections::Collection, crate::error::AppError> {
    let mut conn = lock_database(state.inner())?;
    let actor = sync_state::device_id(&conn)?;
    crate::collections::create_for_record(&mut conn, input, &record_id, &actor)
}

#[tauri::command]
pub fn update_collection(
    state: State<DbState>,
    id: String,
    input: crate::collections::UpdateCollectionInput,
) -> Result<crate::collections::Collection, crate::error::AppError> {
    let mut conn = lock_database(state.inner())?;
    let actor = sync_state::device_id(&conn)?;
    crate::collections::update(&mut conn, &id, input, &actor)
}

#[tauri::command]
pub fn delete_collection(
    state: State<DbState>,
    id: String,
    expected_rev: i64,
) -> Result<(), crate::error::AppError> {
    let mut conn = lock_database(state.inner())?;
    let actor = sync_state::device_id(&conn)?;
    crate::collections::delete(&mut conn, &id, expected_rev, &actor)
}

#[tauri::command]
pub fn add_collection_members(
    state: State<DbState>,
    collection_id: String,
    record_ids: Vec<String>,
    source_kind: String,
    expected_rev: i64,
) -> Result<Vec<crate::collections::CollectionMember>, crate::error::AppError> {
    let mut conn = lock_database(state.inner())?;
    let actor = sync_state::device_id(&conn)?;
    crate::collections::add_members(
        &mut conn,
        &collection_id,
        record_ids,
        &source_kind,
        expected_rev,
        &actor,
    )
}

#[tauri::command]
pub fn apply_collection_suggestion(
    state: State<DbState>,
    input: crate::collections::ApplyCollectionSuggestionInput,
) -> Result<crate::collections::Collection, crate::error::AppError> {
    let mut conn = lock_database(state.inner())?;
    let actor = sync_state::device_id(&conn)?;
    crate::collections::apply_suggestion(&mut conn, input, &actor)
}

#[tauri::command]
pub fn create_missing_seasons(
    state: State<DbState>,
    collection_id: String,
    records: Vec<WatchRecord>,
    expected_rev: i64,
) -> Result<Vec<WatchRecord>, crate::error::AppError> {
    let mut conn = lock_database(state.inner())?;
    let actor = sync_state::device_id(&conn)?;
    crate::collections::create_records_with_members(
        &mut conn,
        &collection_id,
        records,
        expected_rev,
        &actor,
    )
}

#[tauri::command]
pub fn complete_movie_collection(
    state: State<DbState>,
    input: crate::collections::CompleteMovieCollectionInput,
) -> Result<crate::collections::CompleteMovieCollectionResult, crate::error::AppError> {
    let mut conn = lock_database(state.inner())?;
    let actor = sync_state::device_id(&conn)?;
    crate::collections::complete_movie_collection(&mut conn, input, &actor)
}

#[tauri::command]
pub fn remove_collection_member(
    state: State<DbState>,
    collection_id: String,
    record_id: String,
    expected_rev: i64,
) -> Result<(), crate::error::AppError> {
    let mut conn = lock_database(state.inner())?;
    let actor = sync_state::device_id(&conn)?;
    crate::collections::remove_member(&mut conn, &collection_id, &record_id, expected_rev, &actor)
}

#[tauri::command]
pub fn reorder_collection_members(
    state: State<DbState>,
    collection_id: String,
    record_ids: Vec<String>,
    expected_rev: i64,
) -> Result<Vec<crate::collections::CollectionMember>, crate::error::AppError> {
    let mut conn = lock_database(state.inner())?;
    let actor = sync_state::device_id(&conn)?;
    crate::collections::reorder(&mut conn, &collection_id, record_ids, expected_rev, &actor)
}

#[tauri::command]
pub fn get_episode_tracking(
    state: State<DbState>,
    record_id: String,
) -> Result<crate::episode_history::EpisodeTracking, crate::error::AppError> {
    let conn = lock_database(state.inner())?;
    crate::episode_history::tracking(&conn, &record_id)
}

#[tauri::command]
pub fn get_all_episode_completions(
    state: State<DbState>,
) -> Result<Vec<crate::episode_history::EpisodeCompletion>, crate::error::AppError> {
    let conn = lock_database(state.inner())?;
    crate::episode_history::all_completions(&conn)
}

#[tauri::command]
pub fn get_local_export_snapshot(
    state: State<DbState>,
) -> Result<crate::local_export::LocalExportSnapshot, crate::error::AppError> {
    let conn = lock_database(state.inner())?;
    crate::local_export::snapshot(&conn)
}

#[tauri::command]
pub fn stage_local_export(
    paths: State<AppPaths>,
    json: String,
) -> Result<String, crate::error::AppError> {
    crate::local_export::stage(paths.inner(), &json)
}

#[tauri::command]
pub fn discard_local_export_stage(
    paths: State<AppPaths>,
    token: String,
) -> Result<(), crate::error::AppError> {
    crate::local_export::discard(paths.inner(), &token)
}

#[tauri::command]
pub fn preview_local_import(
    state: State<DbState>,
    paths: State<AppPaths>,
    token: String,
    file_name: String,
) -> Result<crate::local_import::LocalImportPreview, crate::error::AppError> {
    let conn = lock_database(state.inner())?;
    crate::local_import::preview(&conn, paths.inner(), &token, &file_name)
}

#[tauri::command]
pub fn commit_local_import(
    state: State<DbState>,
    paths: State<AppPaths>,
    token: String,
    expected_stage_sha256: String,
    expected_library_fingerprint: String,
) -> Result<crate::local_import::LocalImportResult, crate::error::AppError> {
    // The application database mutex remains held across re-read, validation,
    // freshness check, recovery snapshot and atomic replacement. Local edits
    // and sync therefore cannot enter between the stale guard and replacement.
    let mut conn = lock_database(state.inner())?;
    crate::local_import::commit(
        &mut conn,
        paths.inner(),
        &token,
        &expected_stage_sha256,
        &expected_library_fingerprint,
    )
}

#[tauri::command]
pub fn discard_local_import_stage(
    paths: State<AppPaths>,
    token: String,
) -> Result<(), crate::error::AppError> {
    crate::local_import::discard(paths.inner(), &token)
}

#[tauri::command]
pub fn replace_library(
    state: State<DbState>,
    paths: State<AppPaths>,
    records: Vec<WatchRecord>,
    episode_completions: Vec<crate::episode_history::EpisodeCompletion>,
) -> Result<(), crate::error::AppError> {
    let mut conn = lock_database(state.inner())?;
    recovery_points::create(&conn, paths.inner(), "import")?;
    crate::episode_history::replace_library_atomic(&mut conn, records, episode_completions)
}

#[tauri::command]
pub fn replace_library_v3(
    state: State<DbState>,
    paths: State<AppPaths>,
    records: Vec<WatchRecord>,
    episode_completions: Vec<crate::episode_history::EpisodeCompletion>,
    collections: Vec<crate::collections::Collection>,
    collection_members: Vec<crate::collections::CollectionMember>,
) -> Result<(), crate::error::AppError> {
    let mut conn = lock_database(state.inner())?;
    recovery_points::create(&conn, paths.inner(), "import")?;
    crate::collections::replace_library_atomic(
        &mut conn,
        records,
        episode_completions,
        collections,
        collection_members,
    )
}

#[tauri::command]
pub fn enable_episode_tracking(
    state: State<DbState>,
    record_id: String,
    initial_next_episode: i32,
    expected_rev: i64,
) -> Result<crate::episode_history::EpisodeTracking, crate::error::AppError> {
    let mut conn = lock_database(state.inner())?;
    let actor_id = sync_state::device_id(&conn)?;
    crate::episode_history::enable(
        &mut conn,
        &record_id,
        initial_next_episode,
        expected_rev,
        &actor_id,
    )
}

#[tauri::command]
pub fn set_next_episode(
    state: State<DbState>,
    record_id: String,
    next_episode: Option<i32>,
    expected_rev: i64,
) -> Result<crate::episode_history::EpisodeTracking, crate::error::AppError> {
    let mut conn = lock_database(state.inner())?;
    let actor_id = sync_state::device_id(&conn)?;
    crate::episode_history::set_next(&mut conn, &record_id, next_episode, expected_rev, &actor_id)
}

#[tauri::command]
pub fn replace_all_records(
    state: State<DbState>,
    paths: State<AppPaths>,
    records: Vec<WatchRecord>,
    reason: String,
) -> Result<(), crate::error::AppError> {
    let mut conn = lock_database(state.inner())?;
    recovery_points::create(&conn, paths.inner(), &reason)?;
    crate::db_atomic_crud::replace_all_records_atomic(&mut conn, records)
}

#[tauri::command]
pub fn get_sync_snapshot(
    state: State<DbState>,
    paths: State<AppPaths>,
) -> Result<sync_state::SyncSnapshot, crate::error::AppError> {
    let mut conn = lock_database(state.inner())?;
    crate::sync_targets::ensure_migrated(&mut conn, paths.inner())?;
    sync_state::snapshot(&conn)
}

#[tauri::command]
pub fn get_sync_runtime_state(
    state: State<DbState>,
    paths: State<AppPaths>,
) -> Result<sync_state::SyncRuntimeState, crate::error::AppError> {
    let mut conn = lock_database(state.inner())?;
    if let Err(error) = crate::sync_targets::ensure_migrated(&mut conn, paths.inner()) {
        if error.to_string().contains("target_migration_required") {
            return sync_state::runtime_state(&conn);
        }
        return Err(error);
    }
    sync_state::runtime_state(&conn)
}

#[tauri::command]
pub fn get_sync_targets(
    state: State<DbState>,
    paths: State<AppPaths>,
) -> Result<crate::sync_targets::SyncTargetRegistry, crate::error::AppError> {
    let mut conn = lock_database(state.inner())?;
    crate::sync_targets::ensure_migrated(&mut conn, paths.inner())
}

#[tauri::command]
pub fn get_active_sync_connection(
    state: State<DbState>,
    paths: State<AppPaths>,
) -> Result<Option<crate::sync_targets::ActiveSyncConnection>, crate::error::AppError> {
    let mut conn = lock_database(state.inner())?;
    crate::sync_targets::credentials(&mut conn, paths.inner())
}

#[tauri::command]
pub fn activate_sync_target(
    state: State<DbState>,
    paths: State<AppPaths>,
    input: crate::sync_targets::ActivateTargetInput,
) -> Result<crate::sync_targets::SyncTargetRegistry, crate::error::AppError> {
    let mut conn = lock_database(state.inner())?;
    crate::sync_targets::activate(&mut conn, paths.inner(), input)
}

#[tauri::command]
pub fn disconnect_sync_target(
    state: State<DbState>,
    paths: State<AppPaths>,
) -> Result<crate::sync_targets::SyncTargetRegistry, crate::error::AppError> {
    let mut conn = lock_database(state.inner())?;
    crate::sync_targets::disconnect(&mut conn, paths.inner())
}

#[tauri::command]
pub fn set_auto_sync_paused(
    state: State<DbState>,
    paused: bool,
    target_id: Option<String>,
    target_epoch: Option<u64>,
) -> Result<sync_state::SyncRuntimeState, crate::error::AppError> {
    let conn = lock_database(state.inner())?;
    sync_state::set_paused(&conn, paused, target_id.as_deref(), target_epoch)
}

#[tauri::command]
pub fn record_sync_failure(
    state: State<DbState>,
    code: String,
    next_attempt_at: Option<String>,
    target_id: Option<String>,
    target_epoch: Option<u64>,
) -> Result<sync_state::SyncRuntimeState, crate::error::AppError> {
    let conn = lock_database(state.inner())?;
    sync_state::record_failure(
        &conn,
        &code,
        next_attempt_at,
        target_id.as_deref(),
        target_epoch,
    )
}

#[tauri::command]
pub fn commit_sync_result(
    state: State<DbState>,
    paths: State<AppPaths>,
    input: sync_state::SyncCommitInput,
) -> Result<sync_state::SyncCommitResult, crate::error::AppError> {
    let mut conn = lock_database(state.inner())?;
    sync_state::commit(&mut conn, paths.inner(), input)
}

#[tauri::command]
pub fn prepare_sync_publish_intent(
    state: State<DbState>,
    input: crate::sync_staging::PreparePublishIntentInput,
) -> Result<crate::sync_staging::SyncPublishIntent, crate::error::AppError> {
    let conn = lock_database(state.inner())?;
    sync_state::prepare_publish_intent(&conn, input)
}

#[tauri::command]
pub fn resolve_sync_conflict(
    state: State<DbState>,
    id: String,
    resolution: sync_state::SyncConflictResolution,
    target_id: Option<String>,
    target_epoch: Option<u64>,
) -> Result<(), crate::error::AppError> {
    let mut conn = lock_database(state.inner())?;
    sync_state::resolve_conflict(
        &mut conn,
        &id,
        resolution,
        target_id.as_deref(),
        target_epoch,
    )
}

#[tauri::command]
pub fn create_recovery_point(
    state: State<DbState>,
    paths: State<AppPaths>,
    reason: String,
) -> Result<recovery_points::RecoveryPoint, crate::error::AppError> {
    let conn = lock_database(state.inner())?;
    recovery_points::create(&conn, paths.inner(), &reason)
}

#[tauri::command]
pub fn list_recovery_points(
    paths: State<AppPaths>,
) -> Result<recovery_points::RecoveryPointList, crate::error::AppError> {
    recovery_points::list(paths.inner())
}

#[tauri::command]
pub fn set_recovery_point_retained(
    paths: State<AppPaths>,
    id: String,
    retained: bool,
) -> Result<(), crate::error::AppError> {
    recovery_points::set_retained(paths.inner(), &id, retained)
}

#[tauri::command]
pub fn delete_recovery_point(
    paths: State<AppPaths>,
    id: String,
) -> Result<(), crate::error::AppError> {
    recovery_points::delete(paths.inner(), &id)
}

#[tauri::command]
pub fn restore_recovery_point(
    state: State<DbState>,
    paths: State<AppPaths>,
    id: String,
) -> Result<recovery_points::RecoveryResult, crate::error::AppError> {
    let mut conn = lock_database(state.inner())?;
    recovery_points::restore(&mut conn, paths.inner(), &id)
}

#[tauri::command]
pub fn open_backup_directory(paths: State<AppPaths>) -> Result<(), crate::error::AppError> {
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("explorer")
            .arg(paths.backups())
            .spawn()?;
        Ok(())
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = paths;
        Err(crate::error::AppError::General(
            "Opening the backup directory is only supported on Windows".to_string(),
        ))
    }
}

#[tauri::command]
pub fn vacuum_db(state: State<DbState>) -> Result<(), crate::error::AppError> {
    let conn = lock_database(state.inner())?;
    Ok(db::vacuum_db(&conn)?)
}

#[tauri::command]
pub fn get_setting(
    state: State<DbState>,
    key: String,
) -> Result<Option<String>, crate::error::AppError> {
    let conn = lock_database(state.inner())?;
    Ok(db::get_setting(&conn, key)?)
}

#[tauri::command]
pub fn set_setting(
    state: State<DbState>,
    key: String,
    value: String,
) -> Result<(), crate::error::AppError> {
    let conn = lock_database(state.inner())?;
    Ok(db::set_setting(&conn, key, value)?)
}

fn tmdb_secret(
    conn: &rusqlite::Connection,
) -> Result<Option<zeroize::Zeroizing<String>>, crate::error::AppError> {
    crate::secret_store::resolve_or_migrate(
        conn,
        "tmdb_api_key",
        &crate::secret_store::LogicalSecret::Tmdb,
        "api-key",
        |legacy| Ok(legacy.to_string()),
    )
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CredentialStatus {
    available: bool,
    state: String,
}

#[tauri::command]
pub fn get_tmdb_credential_status(
    state: State<DbState>,
) -> Result<CredentialStatus, crate::error::AppError> {
    let conn = lock_database(state.inner())?;
    match tmdb_secret(&conn) {
        Ok(secret) => Ok(CredentialStatus {
            available: secret.is_some(),
            state: if secret.is_some() {
                "protected".into()
            } else {
                "missing".into()
            },
        }),
        Err(error) => Ok(CredentialStatus {
            available: false,
            state: if error.to_string().contains("credential_reentry_required") {
                "reentry-required".into()
            } else {
                "unavailable".into()
            },
        }),
    }
}

#[tauri::command]
pub fn save_tmdb_credential(
    state: State<DbState>,
    secret: String,
) -> Result<CredentialStatus, crate::error::AppError> {
    let conn = lock_database(state.inner())?;
    crate::secret_store::save_setting_secret(
        &conn,
        "tmdb_api_key",
        &crate::secret_store::LogicalSecret::Tmdb,
        "api-key",
        &secret,
    )?;
    Ok(CredentialStatus {
        available: true,
        state: "protected".into(),
    })
}

#[tauri::command]
pub fn clear_tmdb_credential(
    state: State<DbState>,
) -> Result<CredentialStatus, crate::error::AppError> {
    let conn = lock_database(state.inner())?;
    crate::secret_store::clear_setting_secret(
        &conn,
        "tmdb_api_key",
        &crate::secret_store::LogicalSecret::Tmdb,
    )?;
    Ok(CredentialStatus {
        available: false,
        state: "missing".into(),
    })
}

#[tauri::command]
pub async fn search_tmdb(
    state: State<'_, DbState>,
    query: String,
    language: Option<String>,
    proxy: Option<String>,
) -> Result<Value, crate::error::AppError> {
    let api_key = {
        let conn = lock_database(state.inner())?;
        tmdb_secret(&conn)?
            .ok_or_else(|| crate::error::AppError::General("credential_missing".into()))?
    };
    net::search_tmdb(
        api_key.to_string(),
        query,
        language.unwrap_or("zh-CN".to_string()),
        proxy,
    )
    .await
    .map_err(crate::error::AppError::General)
}

#[tauri::command]
pub async fn get_tmdb_detail(
    state: State<'_, DbState>,
    id: i32,
    media_type: String,
    language: Option<String>,
    proxy: Option<String>,
) -> Result<Value, crate::error::AppError> {
    let api_key = {
        let conn = lock_database(state.inner())?;
        tmdb_secret(&conn)?
            .ok_or_else(|| crate::error::AppError::General("credential_missing".into()))?
    };
    net::get_tmdb_detail(
        api_key.to_string(),
        id,
        media_type,
        language.unwrap_or("zh-CN".to_string()),
        proxy,
    )
    .await
    .map_err(crate::error::AppError::General)
}

#[tauri::command]
pub async fn get_tmdb_season_detail(
    state: State<'_, DbState>,
    series_id: i32,
    season_number: i32,
    language: Option<String>,
    proxy: Option<String>,
) -> Result<Value, crate::error::AppError> {
    let api_key = {
        let conn = lock_database(state.inner())?;
        tmdb_secret(&conn)?
            .ok_or_else(|| crate::error::AppError::General("credential_missing".into()))?
    };
    net::get_tmdb_season_detail(
        api_key.to_string(),
        series_id,
        season_number,
        language.unwrap_or("en-US".to_string()),
        proxy,
    )
    .await
    .map_err(crate::error::AppError::General)
}

#[tauri::command]
pub async fn download_poster(
    state: State<'_, DbState>,
    paths: State<'_, AppPaths>,
    poster_state: State<'_, crate::poster_cache::PosterDownloadState>,
    path: String,
    size: Option<String>,
    proxy: Option<String>,
) -> Result<crate::poster_cache::PosterDownloadResult, crate::error::AppError> {
    let result = net::download_poster(&paths, &poster_state, path, size, proxy)
        .await
        .map_err(crate::error::AppError::General)?;
    match lock_database(&state).and_then(|conn| {
        let records = db::get_all_records(&conn)?;
        crate::poster_cache::enforce_capacity(&paths, &records)
            .map_err(crate::error::AppError::General)
    }) {
        Ok(_) => {}
        Err(error) => log::warn!("Could not enforce poster cache capacity after download: {error}"),
    }
    Ok(result)
}

#[tauri::command]
pub fn get_poster_cache_stats(
    state: State<'_, DbState>,
    paths: State<'_, AppPaths>,
) -> Result<crate::poster_cache::PosterCacheStats, crate::error::AppError> {
    let conn = lock_database(&state)?;
    let records = db::get_all_records(&conn)?;
    crate::poster_cache::stats(&paths, &records).map_err(crate::error::AppError::General)
}

#[tauri::command]
pub fn clean_poster_cache(
    state: State<'_, DbState>,
    paths: State<'_, AppPaths>,
    mode: String,
) -> Result<crate::poster_cache::PosterCacheStats, crate::error::AppError> {
    let conn = lock_database(&state)?;
    let records = db::get_all_records(&conn)?;
    crate::poster_cache::clean(&paths, &records, &mode).map_err(crate::error::AppError::General)
}

#[tauri::command]
pub async fn webdav_request(
    state: State<'_, DbState>,
    paths: State<'_, AppPaths>,
    request: net::StoredWebDavRequest,
) -> Result<net::WebDavResponse, crate::error::AppError> {
    if !matches!(
        request.method.as_str(),
        "GET" | "PUT" | "MKCOL" | "PROPFIND"
    ) {
        return Err(crate::error::AppError::General(
            "Unsupported WebDAV method".to_string(),
        ));
    }
    let (base_url, username, password) = {
        let mut conn = lock_database(state.inner())?;
        crate::sync_targets::active_request_credentials(
            &mut conn,
            paths.inner(),
            &request.target_id,
            request.target_epoch,
        )?
    };
    let allowed_url = request.url == base_url
        || request.url == format!("{base_url}records-v3.json")
        || request.url == format!("{base_url}records.json");
    if !allowed_url {
        return Err(crate::error::AppError::General(
            "Invalid WebDAV URL".to_string(),
        ));
    }
    validate_webdav_conditions(
        &request.method,
        request.if_match.as_deref(),
        request.if_none_match.as_deref(),
        request.if_dav_etag.as_deref(),
    )?;
    net::webdav_request(net::WebDavRequest {
        method: request.method,
        url: request.url,
        username,
        password: password.to_string(),
        body: request.body,
        proxy: request.proxy,
        if_match: request.if_match,
        if_none_match: request.if_none_match,
        if_dav_etag: request.if_dav_etag,
    })
    .await
    .map_err(crate::error::AppError::General)
}

#[tauri::command]
pub async fn probe_webdav_request(
    request: net::WebDavRequest,
) -> Result<net::WebDavResponse, crate::error::AppError> {
    if !matches!(request.method.as_str(), "GET" | "PROPFIND") {
        return Err(crate::error::AppError::General(
            "Probe must be read-only".into(),
        ));
    }
    if !request.url.starts_with("http://") && !request.url.starts_with("https://") {
        return Err(crate::error::AppError::General("Invalid WebDAV URL".into()));
    }
    validate_webdav_conditions(&request.method, None, None, None)?;
    net::webdav_request(request)
        .await
        .map_err(crate::error::AppError::General)
}

#[cfg(test)]
mod command_tests {
    use super::validate_webdav_conditions;

    #[test]
    fn webdav_condition_headers_accept_only_safe_put_preconditions() {
        assert!(validate_webdav_conditions("PUT", Some("\"etag\""), None, None).is_ok());
        assert!(validate_webdav_conditions("PUT", None, Some("*"), None).is_ok());
        assert!(validate_webdav_conditions("PUT", None, None, Some("\"strong\"")).is_ok());
        assert!(validate_webdav_conditions("PUT", None, None, Some("W/\"weak\"")).is_ok());
        assert!(validate_webdav_conditions("PUT", Some("W/\"weak\""), None, None).is_err());
        assert!(validate_webdav_conditions("PUT", Some("\"bad\r\nheader\""), None, None).is_err());
        assert!(validate_webdav_conditions("GET", Some("\"etag\""), None, None).is_err());
        assert!(validate_webdav_conditions("PROPFIND", None, None, None).is_ok());
        assert!(
            validate_webdav_conditions("PUT", Some("\"etag\""), None, Some("W/\"weak\"")).is_err()
        );
        assert!(
            validate_webdav_conditions("PUT", None, None, Some("W/\"bad\r\nheader\"")).is_err()
        );
        assert!(validate_webdav_conditions("PUT", None, None, Some("unquoted")).is_err());
        assert!(validate_webdav_conditions("PUT", None, None, Some("\"bad\r\nheader\"")).is_err());
    }
}
