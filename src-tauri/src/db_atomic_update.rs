use crate::db;
use crate::db_atomic_helpers::{get_tombstones_tx, mark_local_records_mutated, set_tombstones_tx};
use crate::error::AppError;
use crate::models::{Patch, RecordStatus, UpdateWatchRecord, WatchRecord};
use crate::record_validation::prepare_update;
use chrono::Utc;
use rusqlite::{types::Value, Connection};

pub fn update_record_atomic(
    conn: &mut Connection,
    id: &str,
    updates: &UpdateWatchRecord,
    actor_id: &str,
) -> Result<WatchRecord, AppError> {
    if actor_id.trim().is_empty() {
        return Err(AppError::General("Missing revision actor ID".to_string()));
    }
    let updates = prepare_update(updates.clone())?;
    let names_changed = updates.original_name.is_some() || updates.chinese_name.is_some();

    let mut clauses = Vec::new();
    let mut values = Vec::new();

    macro_rules! optional {
        ($field:expr, $column:literal, $convert:expr) => {
            if let Some(value) = &$field {
                clauses.push(concat!($column, " = ?").to_string());
                values.push($convert(value.clone()));
            }
        };
    }
    macro_rules! patch {
        ($field:expr, $column:literal, $convert:expr) => {
            match &$field {
                Patch::Missing => {}
                Patch::Null => {
                    clauses.push(concat!($column, " = ?").to_string());
                    values.push(Value::Null);
                }
                Patch::Value(value) => {
                    clauses.push(concat!($column, " = ?").to_string());
                    values.push($convert(value.clone()));
                }
            }
        };
    }

    optional!(updates.original_name, "originalName", Value::Text);
    optional!(updates.chinese_name, "chineseName", Value::Text);
    optional!(updates.progress, "progress", Value::Text);
    patch!(updates.total_episodes, "totalEpisodes", |value: i32| {
        Value::Integer(value as i64)
    });
    patch!(updates.movie_progress, "movieProgress", |value: i32| {
        Value::Integer(value as i64)
    });
    patch!(updates.movie_duration, "movieDuration", |value: i32| {
        Value::Integer(value as i64)
    });
    patch!(updates.release_year, "releaseYear", Value::Text);
    patch!(updates.poster_path, "posterPath", Value::Text);
    if let Some(status) = &updates.status {
        clauses.push("status = ?".to_string());
        let value = match status {
            RecordStatus::Watched => "已看",
            RecordStatus::Watching => "在看",
            RecordStatus::Unwatched => "未看",
        };
        values.push(Value::Text(value.to_string()));
    }
    optional!(updates.platform, "platform", Value::Text);
    patch!(updates.rating, "rating", |value: i32| Value::Integer(
        value as i64
    ));
    patch!(updates.start_date, "startDate", Value::Text);
    patch!(updates.end_date, "endDate", Value::Text);
    optional!(updates.notes, "notes", Value::Text);
    patch!(updates.imdb_id, "imdbId", Value::Text);
    patch!(updates.is_locked, "isLocked", |value: bool| Value::Integer(
        i64::from(value)
    ));
    patch!(updates.genres, "genres", Value::Text);
    patch!(updates.origin_country, "originCountry", Value::Text);
    patch!(updates.imdb_rating, "imdbRating", Value::Real);
    patch!(updates.tmdb_status, "tmdbStatus", Value::Text);
    patch!(updates.interest_level, "interestLevel", |value: i32| {
        Value::Integer(value as i64)
    });
    patch!(updates.episode_runtime, "episodeRuntime", |value: i32| {
        Value::Integer(value as i64)
    });
    optional!(updates.media_type, "mediaType", Value::Text);
    patch!(updates.content_tags, "contentTags", Value::Text);
    patch!(updates.tmdb_media_kind, "tmdbMediaKind", Value::Text);
    patch!(updates.tmdb_id, "tmdbId", Value::Integer);
    patch!(updates.tmdb_parent_id, "tmdbParentId", Value::Integer);
    patch!(
        updates.tmdb_season_number,
        "tmdbSeasonNumber",
        |value: i32| Value::Integer(value as i64)
    );
    patch!(updates.series_record_kind, "seriesRecordKind", Value::Text);

    if clauses.is_empty() {
        return Err(AppError::General("Empty update payload".to_string()));
    }

    let transaction = conn.transaction()?;
    let updated_at = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    clauses.push("updatedAt = ?".to_string());
    values.push(Value::Text(updated_at));
    clauses.push("rev = COALESCE(rev, 0) + 1".to_string());
    clauses.push("revActor = ?".to_string());
    values.push(Value::Text(actor_id.to_string()));
    values.push(Value::Text(id.to_string()));

    let sql = format!("UPDATE records SET {} WHERE id = ?", clauses.join(", "));
    if transaction.execute(&sql, rusqlite::params_from_iter(values.iter()))? == 0 {
        return Err(AppError::General(format!("Record not found: {id}")));
    }

    let mut tombstones = get_tombstones_tx(&transaction)?;
    if tombstones.iter().any(|tombstone| tombstone.id == id) {
        tombstones.retain(|tombstone| tombstone.id != id);
        set_tombstones_tx(&transaction, &tombstones)?;
    }
    let generation = mark_local_records_mutated(&transaction, "record-update")?;
    let record = db::get_record(&transaction, id)?
        .ok_or_else(|| AppError::General(format!("Record not found after update: {id}")))?;
    if names_changed && record.original_name.is_empty() && record.chinese_name.is_empty() {
        return Err(AppError::General(
            "Invalid name: at least one title is required".to_string(),
        ));
    }
    crate::sync_staging::stage_upsert(&transaction, &record, generation)?;
    transaction.commit()?;
    Ok(record)
}
