use crate::db_atomic_helpers::{get_tombstones_tx, mark_local_records_mutated, set_tombstones_tx};
use crate::error::AppError;
use crate::models::WatchRecord;
use chrono::{SecondsFormat, Utc};
use rusqlite::{params, Connection};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CompleteMissingTmdbIdentityInput {
    pub record_id: String,
    pub expected_rev: i64,
    pub expected_imdb_id: String,
    pub tmdb_media_kind: String,
    pub tmdb_id: i64,
    pub tmdb_parent_id: Option<i64>,
    pub tmdb_season_number: Option<i32>,
    pub series_record_kind: String,
}

fn normalize_imdb(value: &str) -> Option<String> {
    let value = value.trim().to_ascii_lowercase();
    (value.len() > 2
        && value.starts_with("tt")
        && value[2..]
            .chars()
            .all(|character| character.is_ascii_digit()))
    .then_some(value)
}

fn validate_plan(input: &CompleteMissingTmdbIdentityInput) -> Result<(), AppError> {
    if input.tmdb_id <= 0 || normalize_imdb(&input.expected_imdb_id).is_none() {
        return Err(AppError::General("tmdb_identity_invalid".into()));
    }
    let valid = match input.tmdb_media_kind.as_str() {
        "movie" => {
            input.tmdb_parent_id.is_none()
                && input.tmdb_season_number.is_none()
                && input.series_record_kind == "single-work"
        }
        "tv" => {
            input.tmdb_parent_id.is_none()
                && input.tmdb_season_number.is_none()
                && input.series_record_kind == "whole-series"
        }
        "tv-season" => {
            input.tmdb_parent_id.is_some_and(|id| id > 0)
                && input.tmdb_season_number.is_some_and(|season| season > 0)
                && input.series_record_kind == "season"
        }
        _ => false,
    };
    if !valid {
        return Err(AppError::General("tmdb_identity_invalid".into()));
    }
    Ok(())
}

fn ensure_unique_identity(
    conn: &Connection,
    input: &CompleteMissingTmdbIdentityInput,
) -> Result<(), AppError> {
    let duplicate = match input.tmdb_media_kind.as_str() {
        "movie" => conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM records WHERE id<>?1 AND (tmdbMediaKind='movie' OR mediaType='电影') AND (tmdbId=?2 OR lower(trim(imdbId))=?3))",
            params![input.record_id, input.tmdb_id, normalize_imdb(&input.expected_imdb_id).unwrap()],
            |row| row.get::<_, bool>(0),
        )?,
        "tv-season" => conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM records WHERE id<>?1 AND ((tmdbMediaKind='tv-season' AND tmdbId=?2) OR (tmdbParentId=?3 AND tmdbSeasonNumber=?4)))",
            params![input.record_id, input.tmdb_id, input.tmdb_parent_id, input.tmdb_season_number],
            |row| row.get::<_, bool>(0),
        )?,
        "tv" => conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM records WHERE id<>?1 AND tmdbMediaKind='tv' AND tmdbId=?2)",
            params![input.record_id, input.tmdb_id],
            |row| row.get::<_, bool>(0),
        )?,
        _ => true,
    };
    if duplicate {
        return Err(AppError::General("tmdb_identity_duplicate".into()));
    }
    Ok(())
}

pub fn complete_missing_tmdb_identity(
    conn: &mut Connection,
    input: CompleteMissingTmdbIdentityInput,
    actor: &str,
) -> Result<WatchRecord, AppError> {
    validate_plan(&input)?;
    let tx = conn.transaction()?;
    let mut record = crate::db::get_record(&tx, &input.record_id)?
        .ok_or_else(|| AppError::General("record_not_found".into()))?;
    if record.rev != input.expected_rev {
        return Err(AppError::General("stale_record".into()));
    }
    if record.is_locked.unwrap_or(false) {
        return Err(AppError::General("record_locked".into()));
    }
    if normalize_imdb(record.imdb_id.as_deref().unwrap_or(""))
        != normalize_imdb(&input.expected_imdb_id)
    {
        return Err(AppError::General("tmdb_identity_imdb_changed".into()));
    }
    let conflicts = record
        .tmdb_media_kind
        .as_ref()
        .is_some_and(|value| value != &input.tmdb_media_kind)
        || record.tmdb_id.is_some_and(|value| value != input.tmdb_id)
        || record.tmdb_parent_id != input.tmdb_parent_id && record.tmdb_parent_id.is_some()
        || record.tmdb_season_number != input.tmdb_season_number
            && record.tmdb_season_number.is_some()
        || record
            .series_record_kind
            .as_ref()
            .is_some_and(|value| value != &input.series_record_kind);
    if conflicts {
        return Err(AppError::General("tmdb_identity_conflict".into()));
    }
    ensure_unique_identity(&tx, &input)?;

    let mut changed = false;
    if record.tmdb_media_kind.is_none() {
        record.tmdb_media_kind = Some(input.tmdb_media_kind);
        changed = true;
    }
    if record.tmdb_id.is_none() {
        record.tmdb_id = Some(input.tmdb_id);
        changed = true;
    }
    if record.tmdb_parent_id.is_none() && input.tmdb_parent_id.is_some() {
        record.tmdb_parent_id = input.tmdb_parent_id;
        changed = true;
    }
    if record.tmdb_season_number.is_none() && input.tmdb_season_number.is_some() {
        record.tmdb_season_number = input.tmdb_season_number;
        changed = true;
    }
    if record.series_record_kind.is_none() {
        record.series_record_kind = Some(input.series_record_kind);
        changed = true;
    }
    if !changed {
        tx.commit()?;
        return Ok(record);
    }

    record.rev += 1;
    record.rev_actor = actor.to_string();
    record.updated_at = Some(Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true));
    crate::db::insert_record(&tx, record.clone())?;
    let mut tombstones = get_tombstones_tx(&tx)?;
    if tombstones.iter().any(|item| item.id == record.id) {
        tombstones.retain(|item| item.id != record.id);
        set_tombstones_tx(&tx, &tombstones)?;
    }
    let generation = mark_local_records_mutated(&tx, "tmdb-identity-completion")?;
    crate::sync_staging::stage_upsert(&tx, &record, generation)?;
    tx.commit()?;
    Ok(record)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(id: &str, imdb: &str) -> WatchRecord {
        serde_json::from_value(serde_json::json!({
            "id": id, "originalName": "Series Season 2", "chineseName": "剧集 第 2 季", "progress": "",
            "status": "未看", "platform": "", "startDate": "", "endDate": "", "notes": "",
            "createdAt": "2026-01-01T00:00:00Z", "mediaType": "剧集", "imdbId": imdb,
            "rev": 1, "revActor": "fixture"
        })).unwrap()
    }

    fn database() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::setup_db(&conn).unwrap();
        crate::db::insert_record(&conn, record("season-2", "tt1740299")).unwrap();
        conn
    }

    fn plan() -> CompleteMissingTmdbIdentityInput {
        CompleteMissingTmdbIdentityInput {
            record_id: "season-2".into(),
            expected_rev: 1,
            expected_imdb_id: "tt1740299".into(),
            tmdb_media_kind: "tv-season".into(),
            tmdb_id: 62090,
            tmdb_parent_id: Some(46511),
            tmdb_season_number: Some(2),
            series_record_kind: "season".into(),
        }
    }

    #[test]
    fn fills_only_missing_season_identity_atomically() {
        let mut conn = database();
        let result = complete_missing_tmdb_identity(&mut conn, plan(), "device").unwrap();
        assert_eq!(result.tmdb_parent_id, Some(46511));
        assert_eq!(result.tmdb_season_number, Some(2));
        assert_eq!(result.tmdb_id, Some(62090));
        assert_eq!(result.rev, 2);
    }

    #[test]
    fn conflicting_identity_and_changed_imdb_write_nothing() {
        let mut conn = database();
        conn.execute(
            "UPDATE records SET tmdbParentId=999 WHERE id='season-2'",
            [],
        )
        .unwrap();
        assert!(complete_missing_tmdb_identity(&mut conn, plan(), "device")
            .unwrap_err()
            .to_string()
            .contains("tmdb_identity_conflict"));
        conn.execute(
            "UPDATE records SET tmdbParentId=NULL, imdbId='tt0000001' WHERE id='season-2'",
            [],
        )
        .unwrap();
        assert!(complete_missing_tmdb_identity(&mut conn, plan(), "device")
            .unwrap_err()
            .to_string()
            .contains("tmdb_identity_imdb_changed"));
        assert_eq!(
            crate::db_atomic_helpers::get_records_generation(&conn).unwrap(),
            0
        );
    }

    #[test]
    fn season_identity_uses_its_namespace_and_rejects_same_parent_season() {
        let mut conn = database();
        crate::db::insert_record(&conn, record("whole-series", "tt9999999")).unwrap();
        conn.execute(
            "UPDATE records SET tmdbMediaKind='tv', tmdbId=62090, seriesRecordKind='whole-series' WHERE id='whole-series'",
            [],
        )
        .unwrap();
        assert!(complete_missing_tmdb_identity(&mut conn, plan(), "device").is_ok());

        let mut conn = database();
        crate::db::insert_record(&conn, record("duplicate-season", "tt1740299")).unwrap();
        conn.execute(
            "UPDATE records SET tmdbMediaKind='tv-season', tmdbParentId=46511, tmdbSeasonNumber=2, seriesRecordKind='season' WHERE id='duplicate-season'",
            [],
        )
        .unwrap();
        assert!(complete_missing_tmdb_identity(&mut conn, plan(), "device")
            .unwrap_err()
            .to_string()
            .contains("tmdb_identity_duplicate"));
        assert_eq!(
            crate::db_atomic_helpers::get_records_generation(&conn).unwrap(),
            0
        );
    }
}
