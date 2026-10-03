//! Local business transaction capture; no S1 wire fields or remote authority.
use std::collections::{BTreeMap, HashSet};

use chrono::{SecondsFormat, Utc};
use rusqlite::Connection;
use serde::Serialize;
use serde_json::Value;

use super::local_authority::{capture_delete, capture_staged_descriptor};
use super::ordinary_mutation::DeleteDescriptorV1;
use crate::error::AppError;

type EntityId = (String, String);

pub(crate) struct LocalCaptureSnapshot {
    rows: BTreeMap<EntityId, Value>,
    retained: HashSet<EntityId>,
}

fn failure() -> AppError {
    AppError::General("S2_LOCAL_CAPTURE_INVALID".into())
}
fn text(value: &Value, key: &str) -> Result<String, AppError> {
    value[key].as_str().map(str::to_owned).ok_or_else(failure)
}

impl LocalCaptureSnapshot {
    pub(crate) fn read(conn: &Connection) -> Result<Self, AppError> {
        let mut rows = BTreeMap::new();
        fn append<T: Serialize>(
            rows: &mut BTreeMap<EntityId, Value>,
            kind: &str,
            values: Vec<T>,
        ) -> Result<(), AppError> {
            for item in values {
                let value = serde_json::to_value(item).map_err(|_| failure())?;
                rows.insert((kind.to_owned(), text(&value, "id")?), value);
            }
            Ok(())
        }
        append(&mut rows, "record", crate::db::get_all_records(conn)?)?;
        append(&mut rows, "collection", crate::collections::all(conn)?)?;
        append(
            &mut rows,
            "collection-member",
            crate::collections::all_members(conn)?,
        )?;
        append(
            &mut rows,
            "episode-completion",
            crate::episode_history::all_completions(conn)?,
        )?;
        let retained = rows.keys().cloned().collect();
        Ok(Self { rows, retained })
    }

    pub(crate) fn retain_records(&mut self, ids: &HashSet<String>) {
        self.retained.retain(|(kind, id)| match kind.as_str() {
            "record" => ids.contains(id),
            "episode-completion" | "collection-member" => self.rows[&(kind.clone(), id.clone())]
                ["recordId"]
                .as_str()
                .is_some_and(|parent| ids.contains(parent)),
            _ => true,
        });
    }

    pub(crate) fn retain_kind(&mut self, kind: &str, ids: &HashSet<String>) {
        self.retained
            .retain(|(item_kind, id)| item_kind != kind || ids.contains(id));
    }

    pub(crate) fn remove_record(&mut self, id: &str) {
        let ids = self
            .rows
            .keys()
            .filter(|(kind, candidate)| kind == "record" && candidate != id)
            .map(|(_, id)| id.clone())
            .collect();
        self.retain_records(&ids);
    }

    pub(crate) fn remove_collection(&mut self, id: &str) {
        self.retained.retain(|(kind, candidate)| {
            !(kind == "collection" && candidate == id
                || kind == "collection-member"
                    && self.rows[&(kind.clone(), candidate.clone())]["collectionId"].as_str()
                        == Some(id))
        });
    }

    /// Writes complete evidence before any physical deletion, including the
    /// old rows which a bulk replace will remove temporarily and reinsert.
    /// Only logical removals receive tombstones.
    pub(crate) fn prepare_deletions(
        &self,
        conn: &Connection,
        generation: i64,
        actor: &str,
        timestamp: &str,
    ) -> Result<(), AppError> {
        for (key, before) in &self.rows {
            if !self.retained.contains(key) {
                capture_delete_value(conn, &key.0, before.clone(), generation, actor, timestamp)?;
            }
        }
        Ok(())
    }

    pub(crate) fn capture_episode_changes(
        &self,
        conn: &Connection,
        generation: i64,
    ) -> Result<(), AppError> {
        for item in crate::episode_history::all_completions(conn)? {
            let key = ("episode-completion".to_owned(), item.id.clone());
            let value = serde_json::to_value(&item).map_err(|_| failure())?;
            let before = self.rows.get(&key);
            if before != Some(&value) {
                capture_staged_descriptor(
                    conn,
                    &key.0,
                    &key.1,
                    before.cloned(),
                    Some(value),
                    generation,
                )
                .map_err(|error| AppError::General(error.0.into()))?;
            }
        }
        Ok(())
    }

    pub(crate) fn locked_episode_ids(&self, locked_records: &HashSet<String>) -> HashSet<String> {
        self.rows
            .iter()
            .filter(|((kind, _), value)| {
                kind == "episode-completion"
                    && value["recordId"]
                        .as_str()
                        .is_some_and(|id| locked_records.contains(id))
            })
            .map(|((_, id), _)| id.clone())
            .collect()
    }
}

pub(crate) fn timestamp() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}

pub(crate) fn capture_delete_value(
    conn: &Connection,
    kind: &str,
    before: Value,
    generation: i64,
    actor: &str,
    deleted_at: &str,
) -> Result<(), AppError> {
    let id = text(&before, "id")?;
    let rev = before["rev"]
        .as_i64()
        .ok_or_else(failure)?
        .checked_add(1)
        .ok_or_else(failure)?;
    let descriptor = match kind {
        "record" => DeleteDescriptorV1::Record {
            id,
            deleted_at: deleted_at.into(),
            rev,
            rev_actor: actor.into(),
        },
        "collection" => DeleteDescriptorV1::Collection {
            id,
            deleted_at: deleted_at.into(),
            rev,
            rev_actor: actor.into(),
        },
        "collection-member" => DeleteDescriptorV1::CollectionMember {
            id,
            collection_id: text(&before, "collectionId")?,
            record_id: text(&before, "recordId")?,
            deleted_at: deleted_at.into(),
            rev,
            rev_actor: actor.into(),
        },
        "episode-completion" => DeleteDescriptorV1::EpisodeCompletion {
            id,
            record_id: text(&before, "recordId")?,
            episode_number: before["episodeNumber"]
                .as_i64()
                .and_then(|value| i32::try_from(value).ok())
                .ok_or_else(failure)?,
            deleted_at: deleted_at.into(),
            rev,
            rev_actor: actor.into(),
        },
        _ => return Err(failure()),
    };
    capture_delete(conn, descriptor, before, generation)
        .map_err(|error| AppError::General(error.0.into()))
}
