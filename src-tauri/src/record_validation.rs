use crate::error::AppError;
use crate::models::{Patch, UpdateWatchRecord, WatchRecord, MEDIA_TYPE_VALUES};
use chrono::Utc;
use std::collections::HashSet;

#[derive(Clone, Copy)]
pub enum RecordWriteContext {
    Local,
    ImportOrSync,
}

fn invalid(field: &str, rule: &str) -> AppError {
    AppError::General(format!("Invalid {field}: {rule}"))
}

fn normalize_optional_text(value: &mut Option<String>) {
    *value = value.take().and_then(|text| {
        let trimmed = text.trim();
        (!trimmed.is_empty()).then(|| trimmed.to_string())
    });
}

fn normalize_patch_text(value: &mut Patch<String>) {
    if let Patch::Value(text) = value {
        let trimmed = text.trim();
        *value = if trimmed.is_empty() {
            Patch::Null
        } else {
            Patch::Value(trimmed.to_string())
        };
    }
}

fn valid_positive(value: Option<i32>) -> bool {
    value.map_or(true, |number| number > 0)
}

fn valid_non_negative(value: Option<i32>) -> bool {
    value.map_or(true, |number| number >= 0)
}

fn valid_range(value: Option<i32>, minimum: i32, maximum: i32) -> bool {
    value.map_or(true, |number| (minimum..=maximum).contains(&number))
}

fn valid_score(value: Option<f64>) -> bool {
    value.map_or(true, |number| {
        number.is_finite() && (0.0..=10.0).contains(&number)
    })
}

fn normalize_legacy_numbers(record: &mut WatchRecord) {
    if !valid_positive(record.total_episodes) {
        record.total_episodes = None;
    }
    if !valid_non_negative(record.movie_progress) {
        record.movie_progress = None;
    }
    if !valid_positive(record.movie_duration) {
        record.movie_duration = None;
    }
    if !valid_range(record.rating, 1, 10) {
        record.rating = None;
    }
    if !valid_range(record.interest_level, 1, 5) {
        record.interest_level = None;
    }
    if !valid_positive(record.episode_runtime) {
        record.episode_runtime = None;
    }
    if !valid_score(record.imdb_rating) {
        record.imdb_rating = None;
    }
    if record.rev < 0 {
        record.rev = 0;
    }
}

fn normalize_optional_zeroes(record: &mut WatchRecord) {
    if record.total_episodes == Some(0) {
        record.total_episodes = None;
    }
    if record.movie_duration == Some(0) {
        record.movie_duration = None;
    }
    if record.rating == Some(0) {
        record.rating = None;
    }
    if record.interest_level == Some(0) {
        record.interest_level = None;
    }
    if record.episode_runtime == Some(0) {
        record.episode_runtime = None;
    }
}

fn validate_record_numbers(record: &WatchRecord) -> Result<(), AppError> {
    if !valid_positive(record.total_episodes) {
        return Err(invalid("totalEpisodes", "must be greater than zero"));
    }
    if !valid_non_negative(record.movie_progress) {
        return Err(invalid("movieProgress", "must be non-negative"));
    }
    if !valid_positive(record.movie_duration) {
        return Err(invalid("movieDuration", "must be greater than zero"));
    }
    if !valid_range(record.rating, 1, 10) {
        return Err(invalid("rating", "must be between 1 and 10"));
    }
    if !valid_range(record.interest_level, 1, 5) {
        return Err(invalid("interestLevel", "must be between 1 and 5"));
    }
    if !valid_positive(record.episode_runtime) {
        return Err(invalid("episodeRuntime", "must be greater than zero"));
    }
    if !valid_score(record.imdb_rating) {
        return Err(invalid("imdbRating", "must be finite and between 0 and 10"));
    }
    if record.rev < 0 {
        return Err(invalid("rev", "must be non-negative"));
    }
    Ok(())
}

pub fn prepare_record(
    mut record: WatchRecord,
    context: RecordWriteContext,
) -> Result<WatchRecord, AppError> {
    record.id = record.id.trim().to_string();
    record.original_name = record.original_name.trim().to_string();
    record.chinese_name = record.chinese_name.trim().to_string();
    record.progress = record.progress.trim().to_string();
    record.platform = record.platform.trim().to_string();
    record.created_at = record.created_at.trim().to_string();
    record.media_type = record.media_type.trim().to_string();
    record.rev_actor = record.rev_actor.trim().to_string();
    normalize_optional_text(&mut record.release_year);
    normalize_optional_text(&mut record.poster_path);
    normalize_optional_text(&mut record.start_date);
    normalize_optional_text(&mut record.end_date);
    normalize_optional_text(&mut record.updated_at);
    normalize_optional_text(&mut record.imdb_id);
    normalize_optional_text(&mut record.genres);
    normalize_optional_text(&mut record.origin_country);
    normalize_optional_text(&mut record.tmdb_status);
    normalize_optional_text(&mut record.content_tags);
    normalize_optional_text(&mut record.tmdb_media_kind);
    normalize_optional_text(&mut record.series_record_kind);
    if record
        .tmdb_media_kind
        .as_deref()
        .is_some_and(|value| !matches!(value, "movie" | "tv" | "tv-season"))
    {
        return Err(invalid("tmdbMediaKind", "unsupported value"));
    }
    if record
        .series_record_kind
        .as_deref()
        .is_some_and(|value| !matches!(value, "season" | "whole-series" | "single-work"))
    {
        return Err(invalid("seriesRecordKind", "unsupported value"));
    }
    if record.tmdb_id.is_some_and(|value| value <= 0)
        || record.tmdb_parent_id.is_some_and(|value| value <= 0)
        || record.tmdb_season_number.is_some_and(|value| value < 0)
    {
        return Err(invalid("tmdbIdentity", "invalid numeric value"));
    }

    if record.id.is_empty() {
        return Err(invalid("id", "must not be empty"));
    }
    if record.created_at.trim().is_empty() {
        record.created_at = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    }

    match context {
        RecordWriteContext::Local => {
            normalize_optional_zeroes(&mut record);
            if record.original_name.is_empty() && record.chinese_name.is_empty() {
                return Err(invalid("name", "at least one title is required"));
            }
            if !MEDIA_TYPE_VALUES.contains(&record.media_type.as_str()) {
                return Err(invalid("mediaType", "unsupported value"));
            }
            validate_record_numbers(&record)?;
        }
        RecordWriteContext::ImportOrSync => {
            if !MEDIA_TYPE_VALUES.contains(&record.media_type.as_str()) {
                record.media_type = "电影".to_string();
            }
            normalize_legacy_numbers(&mut record);
        }
    }
    Ok(record)
}

fn validate_patch_i32(
    field: &str,
    patch: &Patch<i32>,
    predicate: impl Fn(i32) -> bool,
    rule: &str,
) -> Result<(), AppError> {
    if let Patch::Value(value) = patch {
        if !predicate(*value) {
            return Err(invalid(field, rule));
        }
    }
    Ok(())
}

pub fn prepare_update(mut updates: UpdateWatchRecord) -> Result<UpdateWatchRecord, AppError> {
    if let Some(value) = &mut updates.original_name {
        *value = value.trim().to_string();
    }
    if let Some(value) = &mut updates.chinese_name {
        *value = value.trim().to_string();
    }
    if let Some(value) = &mut updates.platform {
        *value = value.trim().to_string();
    }
    if let Some(value) = &mut updates.progress {
        *value = value.trim().to_string();
    }
    if let Some(value) = &mut updates.media_type {
        *value = value.trim().to_string();
        if !MEDIA_TYPE_VALUES.contains(&value.as_str()) {
            return Err(invalid("mediaType", "unsupported value"));
        }
    }
    for value in [
        &mut updates.release_year,
        &mut updates.poster_path,
        &mut updates.start_date,
        &mut updates.end_date,
        &mut updates.imdb_id,
        &mut updates.genres,
        &mut updates.origin_country,
        &mut updates.tmdb_status,
        &mut updates.content_tags,
        &mut updates.tmdb_media_kind,
        &mut updates.series_record_kind,
    ] {
        normalize_patch_text(value);
    }

    validate_patch_i32(
        "totalEpisodes",
        &updates.total_episodes,
        |value| value > 0,
        "must be greater than zero",
    )?;
    if let Patch::Value(value) = updates.tmdb_season_number {
        if value < 0 {
            return Err(invalid("tmdbSeasonNumber", "must be non-negative"));
        }
    }
    if let Patch::Value(value) = &updates.tmdb_media_kind {
        if !matches!(value.as_str(), "movie" | "tv" | "tv-season") {
            return Err(invalid("tmdbMediaKind", "unsupported value"));
        }
    }
    if let Patch::Value(value) = &updates.series_record_kind {
        if !matches!(value.as_str(), "season" | "whole-series" | "single-work") {
            return Err(invalid("seriesRecordKind", "unsupported value"));
        }
    }
    validate_patch_i32(
        "movieProgress",
        &updates.movie_progress,
        |value| value >= 0,
        "must be non-negative",
    )?;
    validate_patch_i32(
        "movieDuration",
        &updates.movie_duration,
        |value| value > 0,
        "must be greater than zero",
    )?;
    validate_patch_i32(
        "rating",
        &updates.rating,
        |value| (1..=10).contains(&value),
        "must be between 1 and 10",
    )?;
    validate_patch_i32(
        "interestLevel",
        &updates.interest_level,
        |value| (1..=5).contains(&value),
        "must be between 1 and 5",
    )?;
    validate_patch_i32(
        "episodeRuntime",
        &updates.episode_runtime,
        |value| value > 0,
        "must be greater than zero",
    )?;
    if let Patch::Value(value) = updates.imdb_rating {
        if !value.is_finite() || !(0.0..=10.0).contains(&value) {
            return Err(invalid("imdbRating", "must be finite and between 0 and 10"));
        }
    }
    Ok(updates)
}

pub fn prepare_import_batch(records: Vec<WatchRecord>) -> Result<Vec<WatchRecord>, AppError> {
    let mut ids = HashSet::new();
    records
        .into_iter()
        .map(|record| {
            let record = prepare_record(record, RecordWriteContext::ImportOrSync)?;
            if !ids.insert(record.id.clone()) {
                return Err(invalid("id", "duplicate record in replacement batch"));
            }
            Ok(record)
        })
        .collect()
}
