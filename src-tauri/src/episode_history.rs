use crate::db;
use crate::db_atomic_helpers::mark_local_records_mutated;
use crate::error::AppError;
use crate::models::{RecordStatus, WatchRecord};
use chrono::{Local, SecondsFormat, Utc};
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EpisodeCompletion {
    pub id: String,
    pub record_id: String,
    pub episode_number: i32,
    pub completed_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub rev: i64,
    pub rev_actor: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EpisodeTracking {
    pub record: WatchRecord,
    pub completions: Vec<EpisodeCompletion>,
}

fn invalid(code: &str) -> AppError {
    AppError::General(code.to_string())
}

fn completion_id(record_id: &str, episode: i32) -> String {
    let mut digest = Sha256::new();
    digest.update(b"episode-completion:v1\0");
    digest.update(record_id.as_bytes());
    digest.update(b"\0");
    digest.update(episode.to_string().as_bytes());
    format!("{:x}", digest.finalize())
}

fn row_to_completion(row: &rusqlite::Row<'_>) -> rusqlite::Result<EpisodeCompletion> {
    Ok(EpisodeCompletion {
        id: row.get("id")?,
        record_id: row.get("recordId")?,
        episode_number: row.get("episodeNumber")?,
        completed_at: row.get("completedAt")?,
        created_at: row.get("createdAt")?,
        updated_at: row.get("updatedAt")?,
        rev: row.get("rev")?,
        rev_actor: row.get("revActor")?,
    })
}

pub fn completions(conn: &Connection, record_id: &str) -> Result<Vec<EpisodeCompletion>, AppError> {
    let mut statement =
        conn.prepare("SELECT * FROM episode_completions WHERE recordId=?1 ORDER BY episodeNumber")?;
    let result = statement
        .query_map([record_id], row_to_completion)?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(result)
}

pub fn all_completions(conn: &Connection) -> Result<Vec<EpisodeCompletion>, AppError> {
    let mut statement =
        conn.prepare("SELECT * FROM episode_completions ORDER BY recordId, episodeNumber")?;
    let result = statement
        .query_map([], row_to_completion)?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(result)
}

pub fn replace_completions_tx(
    conn: &Connection,
    items: &[EpisodeCompletion],
    locked_record_ids: &std::collections::HashSet<String>,
) -> Result<(), AppError> {
    let mut seen = std::collections::HashSet::new();
    for item in items {
        if item.id != completion_id(&item.record_id, item.episode_number)
            || item.episode_number <= 0
            || item.created_at.is_empty()
            || item.updated_at.is_empty()
            || item.rev < 0
            || !seen.insert((item.record_id.clone(), item.episode_number))
        {
            return Err(invalid("episode_history_corrupt"));
        }
        let total = conn
            .query_row(
                "SELECT totalEpisodes FROM records WHERE id=?1",
                [&item.record_id],
                |row| row.get::<_, Option<i32>>(0),
            )
            .optional()?
            .flatten()
            .ok_or_else(|| invalid("episode_history_corrupt"))?;
        if item.episode_number > total {
            return Err(invalid("episode_history_corrupt"));
        }
        if let Some(value) = &item.completed_at {
            chrono::DateTime::parse_from_rfc3339(value)
                .map_err(|_| invalid("episode_history_corrupt"))?;
        }
    }
    conn.execute(
        "DELETE FROM episode_completions
         WHERE recordId NOT IN (SELECT id FROM records WHERE isLocked=1)",
        [],
    )?;
    for item in items {
        if locked_record_ids.contains(&item.record_id) {
            continue;
        }
        conn.execute(
            "INSERT INTO episode_completions
              (id,recordId,episodeNumber,completedAt,createdAt,updatedAt,rev,revActor)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
            params![
                item.id,
                item.record_id,
                item.episode_number,
                item.completed_at,
                item.created_at,
                item.updated_at,
                item.rev,
                item.rev_actor
            ],
        )?;
    }
    Ok(())
}

pub fn replace_library_atomic(
    conn: &mut Connection,
    records: Vec<WatchRecord>,
    completions: Vec<EpisodeCompletion>,
) -> Result<(), AppError> {
    let transaction = conn.transaction()?;
    let previous_collection_members = crate::collections::all_members(&transaction)?;
    let actor = crate::sync_state::device_id(&transaction)?;
    let locked_ids = transaction
        .prepare("SELECT id FROM records WHERE isLocked=1")?
        .query_map([], |row| row.get(0))?
        .collect::<Result<std::collections::HashSet<String>, _>>()?;
    let records = crate::record_validation::prepare_import_batch(
        records
            .into_iter()
            .filter(|record| !locked_ids.contains(&record.id))
            .collect(),
    )?;
    db::replace_all_records_tx(&transaction, records)?;
    crate::collections::reconcile_after_record_replace_tx(
        &transaction,
        &previous_collection_members,
        &actor,
    )?;
    replace_completions_tx(&transaction, &completions, &locked_ids)?;
    let generation = mark_local_records_mutated(&transaction, "library-import")?;
    crate::sync_staging::rebuild_from_current(&transaction, generation)?;
    transaction.commit()?;
    Ok(())
}

pub fn tracking(conn: &Connection, record_id: &str) -> Result<EpisodeTracking, AppError> {
    let record =
        db::get_record(conn, record_id)?.ok_or_else(|| invalid("episode_record_missing"))?;
    Ok(EpisodeTracking {
        completions: completions(conn, record_id)?,
        record,
    })
}

fn validate_record(record: &WatchRecord, expected_rev: i64) -> Result<i32, AppError> {
    if record.rev != expected_rev {
        return Err(invalid("stale_episode_progress"));
    }
    if record.is_locked.unwrap_or(false) {
        return Err(invalid("episode_record_locked"));
    }
    if record.media_type == "电影" {
        return Err(invalid("episode_tracking_unsupported_media"));
    }
    record
        .total_episodes
        .filter(|value| *value > 0)
        .ok_or_else(|| invalid("episode_total_missing"))
}

fn validate_history_bounds(
    conn: &Connection,
    record: &WatchRecord,
    total: i32,
) -> Result<(), AppError> {
    let max_completed = conn.query_row(
        "SELECT MAX(episodeNumber) FROM episode_completions WHERE recordId=?1",
        [&record.id],
        |row| row.get::<_, Option<i32>>(0),
    )?;
    if record.next_episode.is_some_and(|episode| episode > total)
        || max_completed.is_some_and(|episode| episode > total)
    {
        return Err(invalid("episode_total_mismatch"));
    }
    Ok(())
}

fn persist_record_change(
    transaction: &Transaction<'_>,
    record_id: &str,
    actor_id: &str,
    now: &str,
    next_episode: Option<i32>,
    complete: bool,
) -> Result<WatchRecord, AppError> {
    let local_date = Local::now().format("%Y-%m-%d").to_string();
    if complete {
        transaction.execute(
            "UPDATE records SET episodeTrackingEnabled=1, nextEpisode=NULL, status='已看',
               endDate=COALESCE(NULLIF(endDate, ''), ?1), updatedAt=?2,
               rev=COALESCE(rev,0)+1, revActor=?3 WHERE id=?4",
            params![local_date, now, actor_id, record_id],
        )?;
    } else {
        transaction.execute(
            "UPDATE records SET episodeTrackingEnabled=1, nextEpisode=?1, status='在看',
               startDate=COALESCE(NULLIF(startDate, ''), ?2), updatedAt=?3,
               rev=COALESCE(rev,0)+1, revActor=?4 WHERE id=?5",
            params![next_episode, local_date, now, actor_id, record_id],
        )?;
    }
    db::get_record(transaction, record_id)?.ok_or_else(|| invalid("episode_record_missing"))
}

fn insert_unknown(
    transaction: &Transaction<'_>,
    record_id: &str,
    episode: i32,
    now: &str,
    actor_id: &str,
) -> Result<(), AppError> {
    transaction.execute(
        "INSERT INTO episode_completions
          (id,recordId,episodeNumber,completedAt,createdAt,updatedAt,rev,revActor)
         VALUES(?1,?2,?3,NULL,?4,?4,1,?5)
         ON CONFLICT(recordId,episodeNumber) DO NOTHING",
        params![
            completion_id(record_id, episode),
            record_id,
            episode,
            now,
            actor_id
        ],
    )?;
    Ok(())
}

fn insert_known(
    transaction: &Transaction<'_>,
    record_id: &str,
    episode: i32,
    now: &str,
    actor_id: &str,
) -> Result<(), AppError> {
    transaction.execute(
        "INSERT INTO episode_completions
          (id,recordId,episodeNumber,completedAt,createdAt,updatedAt,rev,revActor)
         VALUES(?1,?2,?3,?4,?4,?4,1,?5)
         ON CONFLICT(recordId,episodeNumber) DO UPDATE SET
           completedAt=excluded.completedAt, updatedAt=excluded.updatedAt,
           rev=episode_completions.rev+1, revActor=excluded.revActor
         WHERE episode_completions.completedAt IS NULL",
        params![
            completion_id(record_id, episode),
            record_id,
            episode,
            now,
            actor_id
        ],
    )?;
    Ok(())
}

pub fn enable(
    conn: &mut Connection,
    record_id: &str,
    initial_next_episode: i32,
    expected_rev: i64,
    actor_id: &str,
) -> Result<EpisodeTracking, AppError> {
    let record =
        db::get_record(conn, record_id)?.ok_or_else(|| invalid("episode_record_missing"))?;
    let total = validate_record(&record, expected_rev)?;
    if record.status == RecordStatus::Watched {
        return Err(invalid("episode_record_already_completed"));
    }
    validate_history_bounds(conn, &record, total)?;
    if record.episode_tracking_enabled {
        return Err(invalid("episode_tracking_already_enabled"));
    }
    if !(1..=total).contains(&initial_next_episode) {
        return Err(invalid("episode_out_of_range"));
    }
    let transaction = conn.transaction()?;
    let now = Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true);
    let record = persist_record_change(
        &transaction,
        record_id,
        actor_id,
        &now,
        Some(initial_next_episode),
        false,
    )?;
    let generation = mark_local_records_mutated(&transaction, "episode-tracking-enable")?;
    crate::sync_staging::stage_upsert(&transaction, &record, generation)?;
    transaction.commit()?;
    tracking(conn, record_id)
}

pub fn set_next(
    conn: &mut Connection,
    record_id: &str,
    next_episode: Option<i32>,
    expected_rev: i64,
    actor_id: &str,
) -> Result<EpisodeTracking, AppError> {
    let record =
        db::get_record(conn, record_id)?.ok_or_else(|| invalid("episode_record_missing"))?;
    let total = validate_record(&record, expected_rev)?;
    validate_history_bounds(conn, &record, total)?;
    if !record.episode_tracking_enabled {
        return Err(invalid("episode_tracking_not_enabled"));
    }
    if next_episode.is_some_and(|episode| !(1..=total).contains(&episode)) {
        return Err(invalid("episode_out_of_range"));
    }
    if record.next_episode == next_episode {
        return tracking(conn, record_id);
    }

    let transaction = conn.transaction()?;
    let now = Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true);
    if let Some(current) = record.next_episode {
        let boundary = next_episode.map_or(total, |target| target - 1);
        if boundary >= current {
            for episode in current..boundary {
                insert_unknown(&transaction, record_id, episode, &now, actor_id)?;
            }
            insert_known(&transaction, record_id, boundary, &now, actor_id)?;
        }
    }
    let record = persist_record_change(
        &transaction,
        record_id,
        actor_id,
        &now,
        next_episode,
        next_episode.is_none(),
    )?;
    let generation = mark_local_records_mutated(&transaction, "episode-progress")?;
    crate::sync_staging::stage_upsert(&transaction, &record, generation)?;
    transaction.commit()?;
    tracking(conn, record_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{RecordStatus, WatchRecord};

    fn record(id: &str) -> WatchRecord {
        WatchRecord {
            id: id.into(),
            original_name: "Series".into(),
            chinese_name: "剧".into(),
            progress: "旧进度".into(),
            total_episodes: Some(6),
            episode_tracking_enabled: false,
            next_episode: None,
            movie_progress: None,
            movie_duration: None,
            release_year: None,
            poster_path: None,
            status: RecordStatus::Unwatched,
            platform: String::new(),
            rating: None,
            start_date: None,
            end_date: None,
            notes: String::new(),
            created_at: "2026-01-01".into(),
            updated_at: None,
            imdb_id: None,
            is_locked: Some(false),
            genres: None,
            origin_country: None,
            imdb_rating: None,
            tmdb_status: None,
            interest_level: None,
            episode_runtime: None,
            media_type: "剧集".into(),
            content_tags: None,
            tmdb_media_kind: None,
            tmdb_id: None,
            tmdb_parent_id: None,
            tmdb_season_number: None,
            series_record_kind: None,
            rev: 1,
            rev_actor: "seed".into(),
        }
    }

    fn database() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::setup_db(&conn).unwrap();
        crate::db::migrate_episode_history_schema(&conn).unwrap();
        crate::db::insert_record(&conn, record("series")).unwrap();
        conn
    }

    #[test]
    fn enable_and_skip_preserve_legacy_progress_and_create_three_state_history() {
        let mut conn = database();
        let enabled = enable(&mut conn, "series", 2, 1, "device").unwrap();
        assert_eq!(enabled.record.progress, "旧进度");
        assert!(enabled.completions.is_empty());
        let advanced =
            set_next(&mut conn, "series", Some(5), enabled.record.rev, "device").unwrap();
        assert_eq!(advanced.record.next_episode, Some(5));
        assert_eq!(advanced.completions.len(), 3);
        assert_eq!(advanced.completions[0].completed_at, None);
        assert_eq!(advanced.completions[1].completed_at, None);
        assert!(advanced.completions[2].completed_at.is_some());
    }

    #[test]
    fn completion_is_idempotent_and_retreat_keeps_history() {
        let mut conn = database();
        let enabled = enable(&mut conn, "series", 6, 1, "device").unwrap();
        let done = set_next(&mut conn, "series", None, enabled.record.rev, "device").unwrap();
        let completed_at = done.completions[0].completed_at.clone();
        let repeated = set_next(&mut conn, "series", None, done.record.rev, "device").unwrap();
        assert_eq!(repeated.completions[0].completed_at, completed_at);
        let retreated =
            set_next(&mut conn, "series", Some(4), repeated.record.rev, "device").unwrap();
        assert_eq!(retreated.record.status, RecordStatus::Watching);
        assert_eq!(retreated.completions.len(), 1);
        assert_eq!(retreated.record.progress, "旧进度");
    }

    #[test]
    fn completed_record_cannot_enable_tracking() {
        let mut conn = database();
        conn.execute("UPDATE records SET status='已看' WHERE id='series'", [])
            .unwrap();
        let error = enable(&mut conn, "series", 1, 1, "device").unwrap_err();
        assert!(error
            .to_string()
            .contains("episode_record_already_completed"));
        let unchanged = tracking(&conn, "series").unwrap();
        assert!(!unchanged.record.episode_tracking_enabled);
        assert!(unchanged.completions.is_empty());
    }

    #[test]
    fn completed_tracking_can_resume_when_total_increases() {
        let mut conn = database();
        let enabled = enable(&mut conn, "series", 6, 1, "device").unwrap();
        let done = set_next(&mut conn, "series", None, enabled.record.rev, "device").unwrap();
        let end_date = done.record.end_date.clone();
        conn.execute("UPDATE records SET totalEpisodes=8 WHERE id='series'", [])
            .unwrap();

        let resumed = set_next(&mut conn, "series", Some(7), done.record.rev, "device").unwrap();
        assert_eq!(resumed.record.status, RecordStatus::Watching);
        assert_eq!(resumed.record.next_episode, Some(7));
        assert_eq!(resumed.record.end_date, end_date);
        assert_eq!(resumed.completions, done.completions);
    }

    #[test]
    fn shrinking_total_below_existing_history_blocks_without_changes() {
        let mut conn = database();
        let enabled = enable(&mut conn, "series", 6, 1, "device").unwrap();
        let done = set_next(&mut conn, "series", None, enabled.record.rev, "device").unwrap();
        conn.execute("UPDATE records SET totalEpisodes=5 WHERE id='series'", [])
            .unwrap();
        let before = tracking(&conn, "series").unwrap();
        let error = set_next(&mut conn, "series", Some(4), done.record.rev, "device").unwrap_err();
        assert!(error.to_string().contains("episode_total_mismatch"));
        assert_eq!(
            tracking(&conn, "series").unwrap().completions,
            before.completions
        );
    }
}
