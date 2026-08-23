use crate::error::AppError;
use chrono::Utc;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

pub const SYNC_OUTBOX_KEY: &str = "sync_outbox_v1";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SyncOutbox {
    pub version: u8,
    pub pending: bool,
    pub dirty_generation: i64,
    #[serde(default)]
    pub reasons: Vec<String>,
    pub first_queued_at: Option<String>,
    pub last_queued_at: Option<String>,
}

impl SyncOutbox {
    pub fn clean(generation: i64) -> Self {
        Self {
            version: 1,
            pending: false,
            dirty_generation: generation,
            reasons: Vec::new(),
            first_queued_at: None,
            last_queued_at: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Tombstone {
    pub id: String,
    pub deleted_at: String,
    #[serde(default)]
    pub rev: i64,
    #[serde(default)]
    pub rev_actor: String,
}

pub fn get_setting_tx(conn: &Connection, key: &str) -> Result<Option<String>, rusqlite::Error> {
    conn.query_row("SELECT value FROM settings WHERE key = ?1", [key], |row| {
        row.get(0)
    })
    .optional()
}

pub fn set_setting_tx(conn: &Connection, key: &str, value: &str) -> Result<(), rusqlite::Error> {
    conn.execute(
        "INSERT INTO settings (key, value) VALUES (?1, ?2) \
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![key, value],
    )?;
    Ok(())
}

pub fn get_records_generation(conn: &Connection) -> Result<i64, AppError> {
    let Some(value) = get_setting_tx(conn, "records_generation")? else {
        return Ok(0);
    };
    let generation = value
        .parse::<i64>()
        .map_err(|_| AppError::General(format!("Invalid records_generation value: {value}")))?;
    if generation < 0 {
        return Err(AppError::General(format!(
            "Invalid negative records_generation: {generation}"
        )));
    }
    Ok(generation)
}

pub fn mark_records_mutated(conn: &Connection) -> Result<i64, AppError> {
    let next = get_records_generation(conn)?
        .checked_add(1)
        .ok_or_else(|| AppError::General("records_generation overflow".to_string()))?;
    set_setting_tx(conn, "records_generation", &next.to_string())?;
    Ok(next)
}

pub fn get_sync_outbox(conn: &Connection) -> Result<Option<SyncOutbox>, AppError> {
    let key = crate::sync_targets::active_key(conn, SYNC_OUTBOX_KEY, "outbox_v1")?;
    let Some(raw) = get_setting_tx(conn, &key)? else {
        return Ok(None);
    };
    let outbox: SyncOutbox = serde_json::from_str(&raw)
        .map_err(|error| AppError::General(format!("Invalid {SYNC_OUTBOX_KEY}: {error}")))?;
    if outbox.version != 1 || outbox.dirty_generation < 0 {
        return Err(AppError::General(format!(
            "Invalid {SYNC_OUTBOX_KEY} version or generation"
        )));
    }
    Ok(Some(outbox))
}

pub fn set_sync_outbox(conn: &Connection, outbox: &SyncOutbox) -> Result<(), AppError> {
    let raw = serde_json::to_string(outbox)
        .map_err(|error| AppError::General(format!("Could not serialize outbox: {error}")))?;
    set_setting_tx(
        conn,
        &crate::sync_targets::active_key(conn, SYNC_OUTBOX_KEY, "outbox_v1")?,
        &raw,
    )?;
    Ok(())
}

pub fn mark_local_records_mutated(conn: &Connection, reason: &str) -> Result<i64, AppError> {
    let reason = reason.trim();
    if reason.is_empty() {
        return Err(AppError::General("Missing sync outbox reason".to_string()));
    }
    let generation = mark_records_mutated(conn)?;
    if crate::sync_targets::registry_exists_without_active_target(conn)? {
        return Ok(generation);
    }
    let now = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    let mut outbox = get_sync_outbox(conn)?.unwrap_or_else(|| SyncOutbox::clean(generation));
    if !outbox.pending {
        outbox.first_queued_at = Some(now.clone());
        outbox.reasons.clear();
    }
    outbox.pending = true;
    outbox.dirty_generation = generation;
    outbox.last_queued_at = Some(now);
    if !outbox.reasons.iter().any(|value| value == reason) {
        if outbox.reasons.len() == 8 {
            outbox.reasons.remove(0);
        }
        outbox.reasons.push(reason.to_string());
    }
    set_sync_outbox(conn, &outbox)?;
    Ok(generation)
}

pub fn acknowledge_sync_outbox(
    conn: &Connection,
    expected_generation: i64,
) -> Result<SyncOutbox, AppError> {
    let current_generation = get_records_generation(conn)?;
    let mut outbox =
        get_sync_outbox(conn)?.unwrap_or_else(|| SyncOutbox::clean(current_generation));
    if outbox.pending && outbox.dirty_generation <= expected_generation {
        outbox.pending = false;
        outbox.reasons.clear();
        outbox.first_queued_at = None;
        outbox.last_queued_at = None;
    }
    outbox.dirty_generation = current_generation;
    set_sync_outbox(conn, &outbox)?;
    Ok(outbox)
}

pub fn get_tombstones_tx(conn: &Connection) -> Result<Vec<Tombstone>, AppError> {
    match get_setting_tx(conn, "sync_tombstones")? {
        Some(value) => serde_json::from_str(&value)
            .map_err(|error| AppError::General(format!("Invalid sync_tombstones: {error}"))),
        None => Ok(Vec::new()),
    }
}

pub fn set_tombstones_tx(conn: &Connection, tombstones: &[Tombstone]) -> Result<(), AppError> {
    let value = serde_json::to_string(tombstones)
        .map_err(|error| AppError::General(format!("Could not serialize tombstones: {error}")))?;
    set_setting_tx(conn, "sync_tombstones", &value)?;
    Ok(())
}
