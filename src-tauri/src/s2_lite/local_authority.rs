//! Local-only durable authority for S2 Lite ordinary mutations.
//!
//! This is deliberately not a publication queue: it cannot allocate a writer
//! sequence, build a commit, discover a remote, or perform network I/O.

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::canonical::{jcs_bytes, validate_canonical_uuid_v4, ProtocolError, Result};
use super::ordinary_mutation::{
    map_ordinary_mutation_v1, DeleteDescriptorV1, OrdinaryCausalBaseV1, OrdinaryMutationRequestV1,
};
use super::types::CommitMutationV1;

const FAILURE: ProtocolError = ProtocolError("S2_LOCAL_AUTHORITY_FAILURE");
const CORRUPTION: ProtocolError = ProtocolError("S2_LOCAL_AUTHORITY_CORRUPTION");
const SCHEMA_VERSION: &str = "1";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LocalWriterAuthorityV1 {
    pub writer_id: String,
}

#[derive(Clone, Debug)]
pub struct CapturedOrdinaryMutationV1 {
    pub mutation: CommitMutationV1,
    pub causal_base: OrdinaryCausalBaseV1,
    pub first_generation: i64,
    pub last_generation: i64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CapturedStagingDescriptorV1 {
    pub state_version: u8,
    pub entity_kind: String,
    pub entity_id: String,
    pub operation: String,
    pub local_mutation_id: String,
    pub causal_anchor: StagingAnchorStateV1,
    pub base: Option<Value>,
    pub local: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delete_descriptor: Option<DeleteDescriptorV1>,
    pub first_generation: i64,
    pub last_generation: i64,
}

impl CapturedStagingDescriptorV1 {
    /// An old S1 staging deletion without immutable evidence is unprepared;
    /// it must never be reconstructed from surviving mutable state.
    pub fn frozen_delete_evidence(&self) -> Result<&DeleteDescriptorV1> {
        if self.operation != "delete" {
            return Err(CORRUPTION);
        }
        self.delete_descriptor.as_ref().ok_or(CORRUPTION)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", tag = "state")]
pub enum StagingAnchorStateV1 {
    Unavailable,
    Absent,
    Live { value: Value },
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PersistedMutationV1 {
    state_version: u8,
    mutation: CommitMutationV1,
    causal_anchor: PersistedAnchorV1,
    first_generation: i64,
    last_generation: i64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", tag = "kind", content = "value")]
enum PersistedAnchorV1 {
    Absent,
    Tombstone,
    Live(Value),
}

fn database<T>(result: rusqlite::Result<T>) -> Result<T> {
    result.map_err(|_| FAILURE)
}

fn anchor_to_persisted(anchor: &OrdinaryCausalBaseV1) -> PersistedAnchorV1 {
    match anchor {
        OrdinaryCausalBaseV1::Absent => PersistedAnchorV1::Absent,
        OrdinaryCausalBaseV1::Tombstone => PersistedAnchorV1::Tombstone,
        OrdinaryCausalBaseV1::Live(value) => PersistedAnchorV1::Live(value.clone()),
    }
}

fn anchor_from_persisted(anchor: PersistedAnchorV1) -> OrdinaryCausalBaseV1 {
    match anchor {
        PersistedAnchorV1::Absent => OrdinaryCausalBaseV1::Absent,
        PersistedAnchorV1::Tombstone => OrdinaryCausalBaseV1::Tombstone,
        PersistedAnchorV1::Live(value) => OrdinaryCausalBaseV1::Live(value),
    }
}

fn encode(value: &PersistedMutationV1) -> Result<Vec<u8>> {
    serde_json::to_vec(value).map_err(|_| CORRUPTION)
}

fn decode(bytes: &[u8]) -> Result<PersistedMutationV1> {
    let value: Value = s2_serde_json::from_slice(bytes).map_err(|_| CORRUPTION)?;
    let decoded: PersistedMutationV1 =
        serde_json::from_value(value.clone()).map_err(|_| CORRUPTION)?;
    if decoded.state_version != 1
        || decoded.first_generation < 0
        || decoded.last_generation < decoded.first_generation
        || serde_json::to_value(&decoded).map_err(|_| CORRUPTION)? != value
    {
        return Err(CORRUPTION);
    }
    validate_canonical_uuid_v4(&decoded.mutation.local_mutation_id).map_err(|_| CORRUPTION)?;
    // Re-run the frozen envelope checks through the mapper-independent shape.
    if !matches!(decoded.mutation.operation.as_str(), "upsert" | "tombstone")
        || decoded.mutation.entity_type.is_empty()
    {
        return Err(CORRUPTION);
    }
    Ok(decoded)
}

pub(crate) fn migrate_schema(conn: &Connection) -> rusqlite::Result<()> {
    let version = conn
        .query_row(
            "SELECT value FROM settings WHERE key = 's2_lite_local_authority_schema_version'",
            [],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    if version
        .as_deref()
        .is_some_and(|value| value != SCHEMA_VERSION)
    {
        return Err(rusqlite::Error::InvalidQuery);
    }
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS s2_lite_local_writer_v1 (
             singleton INTEGER PRIMARY KEY CHECK(singleton = 1),
             writer_id TEXT NOT NULL UNIQUE
         );
         CREATE TABLE IF NOT EXISTS s2_lite_local_mutation_v1 (
             entity_key_jcs BLOB PRIMARY KEY NOT NULL,
             state_json BLOB NOT NULL CHECK(typeof(state_json) = 'blob')
         );
         CREATE TABLE IF NOT EXISTS s2_lite_local_staging_descriptor_v1 (
             entity_kind TEXT NOT NULL,
             entity_id TEXT NOT NULL,
             state_json BLOB NOT NULL CHECK(typeof(state_json) = 'blob'),
             PRIMARY KEY(entity_kind, entity_id)
         );
         INSERT INTO settings(key, value) VALUES('s2_lite_local_authority_schema_version', '1')
           ON CONFLICT(key) DO NOTHING;",
    )?;
    // A startup load is validation, never a repair path.
    let _ = load_writer(conn).map_err(|_| rusqlite::Error::InvalidQuery)?;
    let mut statement = conn.prepare("SELECT state_json FROM s2_lite_local_mutation_v1")?;
    let rows = statement.query_map([], |row| row.get::<_, Vec<u8>>(0))?;
    for row in rows {
        decode(&row?).map_err(|_| rusqlite::Error::InvalidQuery)?;
    }
    load_staged_descriptors(conn).map_err(|_| rusqlite::Error::InvalidQuery)?;
    Ok(())
}

fn decode_staging_descriptor(bytes: &[u8]) -> Result<CapturedStagingDescriptorV1> {
    let value: Value = s2_serde_json::from_slice(bytes).map_err(|_| CORRUPTION)?;
    let decoded: CapturedStagingDescriptorV1 =
        serde_json::from_value(value.clone()).map_err(|_| CORRUPTION)?;
    if !matches!(decoded.state_version, 1 | 2)
        || !matches!(
            decoded.entity_kind.as_str(),
            "record" | "collection" | "collection-member" | "episode-completion"
        )
        || decoded.entity_id.trim().is_empty()
        || !matches!(decoded.operation.as_str(), "upsert" | "delete")
        || decoded.first_generation < 0
        || decoded.last_generation < decoded.first_generation
        || serde_json::to_value(&decoded).map_err(|_| CORRUPTION)? != value
    {
        return Err(CORRUPTION);
    }
    validate_canonical_uuid_v4(&decoded.local_mutation_id).map_err(|_| CORRUPTION)?;
    if decoded.state_version == 2
        && (decoded.operation == "delete" && decoded.delete_descriptor.is_none()
            || decoded.operation == "upsert" && decoded.local.is_none())
    {
        return Err(CORRUPTION);
    }
    if let Some(delete) = &decoded.delete_descriptor {
        if decoded.operation != "delete"
            || decoded.local.is_some()
            || delete.entity_type() != decoded.entity_kind
            || delete.wire_value()["id"].as_str() != Some(decoded.entity_id.as_str())
        {
            return Err(CORRUPTION);
        }
        super::semantic::validate_tombstone(
            delete.entity_type(),
            &delete.entity_key(),
            &delete.wire_value(),
        )
        .map_err(|_| CORRUPTION)?;
    }
    Ok(decoded)
}

/// Captures the existing S1 atomic staging boundary without changing its S1
/// payload or network behavior. An unavailable first-edit anchor is retained;
/// subsequent discovery cannot retroactively supply its origin.
pub fn capture_staged_descriptor(
    conn: &Connection,
    entity_kind: &str,
    entity_id: &str,
    base: Option<Value>,
    local: Option<Value>,
    generation: i64,
) -> Result<CapturedStagingDescriptorV1> {
    if generation < 0 {
        return Err(CORRUPTION);
    }
    let _ = initialize_writer(conn)?;
    let previous = database(conn.query_row(
        "SELECT state_json FROM s2_lite_local_staging_descriptor_v1 WHERE entity_kind = ?1 AND entity_id = ?2",
        params![entity_kind, entity_id], |row| row.get::<_, Vec<u8>>(0),
    ).optional())?.map(|bytes| decode_staging_descriptor(&bytes)).transpose()?;
    let local_is_upsert = local.is_some();
    if previous
        .as_ref()
        .is_some_and(|row| row.entity_kind != entity_kind || row.entity_id != entity_id)
    {
        return Err(CORRUPTION);
    }
    let descriptor = if let Some(previous) = previous {
        CapturedStagingDescriptorV1 {
            operation: if local.is_some() {
                "upsert".into()
            } else {
                "delete".into()
            },
            local,
            delete_descriptor: if local_is_upsert {
                None
            } else {
                previous.delete_descriptor.clone()
            },
            last_generation: generation,
            ..previous
        }
    } else {
        CapturedStagingDescriptorV1 {
            state_version: 1,
            entity_kind: entity_kind.into(),
            entity_id: entity_id.into(),
            operation: if local.is_some() {
                "upsert".into()
            } else {
                "delete".into()
            },
            local_mutation_id: uuid::Uuid::new_v4().to_string(),
            causal_anchor: StagingAnchorStateV1::Unavailable,
            base,
            local,
            delete_descriptor: None,
            first_generation: generation,
            last_generation: generation,
        }
    };
    let validated =
        decode_staging_descriptor(&serde_json::to_vec(&descriptor).map_err(|_| CORRUPTION)?)?;
    database(conn.execute(
        "INSERT INTO s2_lite_local_staging_descriptor_v1(entity_kind, entity_id, state_json) VALUES(?1, ?2, ?3)
         ON CONFLICT(entity_kind, entity_id) DO UPDATE SET state_json=excluded.state_json",
        params![entity_kind, entity_id, serde_json::to_vec(&validated).map_err(|_| CORRUPTION)?],
    ))?;
    Ok(validated)
}

pub fn staged_descriptor_keys(conn: &Connection) -> Result<Vec<(String, String)>> {
    let mut statement = database(
        conn.prepare("SELECT entity_kind, entity_id FROM s2_lite_local_staging_descriptor_v1"),
    )?;
    let rows = database(statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?))))?;
    rows.map(database).collect()
}

pub fn load_staged_descriptors(conn: &Connection) -> Result<Vec<CapturedStagingDescriptorV1>> {
    let mut statement = database(conn.prepare(
        "SELECT entity_kind, entity_id, state_json FROM s2_lite_local_staging_descriptor_v1 ORDER BY entity_kind, entity_id",
    ))?;
    let rows = database(statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, Vec<u8>>(2)?,
        ))
    }))?;
    rows.map(|row| {
        let (kind, id, bytes) = database(row)?;
        let descriptor = decode_staging_descriptor(&bytes)?;
        if descriptor.entity_kind != kind || descriptor.entity_id != id {
            return Err(CORRUPTION);
        }
        Ok(descriptor)
    })
    .collect()
}

/// Captures immutable deletion evidence before the caller removes the source
/// row. The caller owns the business transaction and its generation.
pub(crate) fn capture_delete(
    conn: &Connection,
    delete: DeleteDescriptorV1,
    before: Value,
    generation: i64,
) -> Result<()> {
    let kind = delete.entity_type();
    let id = delete.wire_value()["id"]
        .as_str()
        .ok_or(CORRUPTION)?
        .to_owned();
    super::semantic::validate_tombstone(kind, &delete.entity_key(), &delete.wire_value())
        .map_err(|_| CORRUPTION)?;
    let staging = crate::sync_staging::get_staging(conn).map_err(|_| CORRUPTION)?;
    let staged = staging
        .entries
        .iter()
        .find(|entry| entry.entity_kind == kind && entry.id == id);
    // Preserve the existing S1 create/delete cancellation. Episode entities
    // have independent S2 capture and never infer absence from an S1 base.
    let previous = load_staged_descriptors(conn)?
        .into_iter()
        .find(|row| row.entity_kind == kind && row.entity_id == id);
    if staged.is_some_and(|entry| entry.base.is_none() && entry.local.is_some())
        && previous.as_ref().is_some_and(|row| {
            row.base.is_none()
                && row.delete_descriptor.is_none()
                && matches!(
                    row.causal_anchor,
                    StagingAnchorStateV1::Unavailable | StagingAnchorStateV1::Absent
                )
        })
    {
        return remove_staged_descriptor(conn, kind, &id);
    }
    let base = staged.and_then(|entry| entry.base.clone()).or(Some(before));
    initialize_writer(conn)?;
    let descriptor = if let Some(previous) = previous {
        CapturedStagingDescriptorV1 {
            state_version: 2,
            operation: "delete".into(),
            local: None,
            delete_descriptor: Some(delete),
            last_generation: generation,
            ..previous
        }
    } else {
        CapturedStagingDescriptorV1 {
            state_version: 2,
            entity_kind: kind.into(),
            entity_id: id.clone(),
            operation: "delete".into(),
            local_mutation_id: uuid::Uuid::new_v4().to_string(),
            causal_anchor: StagingAnchorStateV1::Unavailable,
            base,
            local: None,
            delete_descriptor: Some(delete),
            first_generation: generation,
            last_generation: generation,
        }
    };
    let bytes = serde_json::to_vec(&descriptor).map_err(|_| CORRUPTION)?;
    decode_staging_descriptor(&bytes)?;
    database(conn.execute(
        "INSERT INTO s2_lite_local_staging_descriptor_v1(entity_kind,entity_id,state_json) VALUES(?1,?2,?3)
         ON CONFLICT(entity_kind,entity_id) DO UPDATE SET state_json=excluded.state_json",
        params![kind, id, bytes],
    ))?;
    Ok(())
}

pub fn remove_staged_descriptor(
    conn: &Connection,
    entity_kind: &str,
    entity_id: &str,
) -> Result<()> {
    database(conn.execute(
        "DELETE FROM s2_lite_local_staging_descriptor_v1 WHERE entity_kind=?1 AND entity_id=?2",
        params![entity_kind, entity_id],
    ))?;
    Ok(())
}

pub fn load_writer(conn: &Connection) -> Result<Option<LocalWriterAuthorityV1>> {
    let value = database(
        conn.query_row(
            "SELECT writer_id FROM s2_lite_local_writer_v1 WHERE singleton = 1",
            [],
            |row| row.get::<_, String>(0),
        )
        .optional(),
    )?;
    value
        .map(|writer_id| {
            validate_canonical_uuid_v4(&writer_id).map_err(|_| CORRUPTION)?;
            Ok(LocalWriterAuthorityV1 { writer_id })
        })
        .transpose()
}

pub fn initialize_writer(conn: &Connection) -> Result<LocalWriterAuthorityV1> {
    if let Some(value) = load_writer(conn)? {
        return Ok(value);
    }
    let writer_id = uuid::Uuid::new_v4().to_string();
    database(conn.execute(
        "INSERT INTO s2_lite_local_writer_v1(singleton, writer_id) VALUES(1, ?1)",
        [&writer_id],
    ))?;
    Ok(LocalWriterAuthorityV1 { writer_id })
}

pub fn capture_ordinary_mutation(
    conn: &Connection,
    request: &OrdinaryMutationRequestV1,
    generation: i64,
) -> Result<Option<CapturedOrdinaryMutationV1>> {
    if generation < 0 {
        return Err(CORRUPTION);
    }
    let key = jcs_bytes(&request.payload.entity_key())?;
    let existing = database(
        conn.query_row(
            "SELECT state_json FROM s2_lite_local_mutation_v1 WHERE entity_key_jcs = ?1",
            [&key],
            |row| row.get::<_, Vec<u8>>(0),
        )
        .optional(),
    )?;
    let (anchor, first_generation, local_mutation_id, base_frontier) = match existing {
        Some(bytes) => {
            let previous = decode(&bytes)?;
            // Coalescing is permitted only for the same frozen entity identity;
            // its original UUID, causal anchor and frontier remain immutable.
            (
                anchor_from_persisted(previous.causal_anchor),
                previous.first_generation,
                previous.mutation.local_mutation_id,
                previous.mutation.base_frontier,
            )
        }
        None => (
            request.causal_base.clone(),
            generation,
            request.local_mutation_id.clone(),
            request.base_frontier.clone(),
        ),
    };
    let effective = OrdinaryMutationRequestV1 {
        local_mutation_id,
        payload: request.payload.clone(),
        causal_base: anchor.clone(),
        base_frontier,
    };
    let Some(mutation) = map_ordinary_mutation_v1(&effective)? else {
        // A semantic revert cancels the entire coalesced logical mutation.
        // Retire its identity with the row; the next edit uses a new request's
        // UUID and first generation, while writer authority remains intact.
        database(conn.execute(
            "DELETE FROM s2_lite_local_mutation_v1 WHERE entity_key_jcs = ?1",
            [&key],
        ))?;
        return Ok(None);
    };
    let persisted = PersistedMutationV1 {
        state_version: 1,
        mutation: mutation.clone(),
        causal_anchor: anchor_to_persisted(&anchor),
        first_generation,
        last_generation: generation,
    };
    database(conn.execute(
        "INSERT INTO s2_lite_local_mutation_v1(entity_key_jcs, state_json) VALUES(?1, ?2)
         ON CONFLICT(entity_key_jcs) DO UPDATE SET state_json = excluded.state_json",
        params![key, encode(&persisted)?],
    ))?;
    Ok(Some(CapturedOrdinaryMutationV1 {
        mutation,
        causal_base: anchor,
        first_generation,
        last_generation: generation,
    }))
}

pub fn load_captured_mutations(conn: &Connection) -> Result<Vec<CapturedOrdinaryMutationV1>> {
    let mut statement = database(
        conn.prepare("SELECT state_json FROM s2_lite_local_mutation_v1 ORDER BY entity_key_jcs"),
    )?;
    let rows = database(statement.query_map([], |row| row.get::<_, Vec<u8>>(0)))?;
    rows.map(|row| {
        let item = decode(&database(row)?)?;
        Ok(CapturedOrdinaryMutationV1 {
            mutation: item.mutation,
            causal_base: anchor_from_persisted(item.causal_anchor),
            first_generation: item.first_generation,
            last_generation: item.last_generation,
        })
    })
    .collect()
}

#[cfg(test)]
mod tests {
    use super::super::canonical::sha256_hex;
    use super::super::ordinary_mutation::{
        DeleteDescriptorV1, LocalCollectionV1, LocalEntityValueV1, OrdinaryPayloadV1,
    };
    use super::*;

    fn collection(id: &str, name: &str) -> LocalCollectionV1 {
        LocalCollectionV1 {
            id: id.into(),
            name: name.into(),
            normalized_name: name.to_lowercase(),
            description: None,
            source_kind: "manual".into(),
            source_key: None,
            collection_kind: "manual".into(),
            order_mode: "manual".into(),
            created_at: "2026-01-01T00:00:00.000Z".into(),
            updated_at: "2026-01-01T00:00:00.000Z".into(),
            rev: 1,
            rev_actor: "test".into(),
        }
    }

    fn request(id: &str, mutation_id: &str, name: &str) -> OrdinaryMutationRequestV1 {
        OrdinaryMutationRequestV1 {
            local_mutation_id: mutation_id.into(),
            payload: OrdinaryPayloadV1::Upsert(LocalEntityValueV1::Collection(collection(
                id, name,
            ))),
            causal_base: OrdinaryCausalBaseV1::Absent,
            base_frontier: Vec::new(),
        }
    }

    #[test]
    fn capture_coalesces_without_replacing_writer_anchor_or_mutation_id() {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::setup_db(&conn).unwrap();
        let writer = initialize_writer(&conn).unwrap();
        let first = capture_ordinary_mutation(
            &conn,
            &request("c1", "10000000-0000-4000-8000-000000000001", "First"),
            4,
        )
        .unwrap()
        .unwrap();
        let second = capture_ordinary_mutation(
            &conn,
            &request("c1", "20000000-0000-4000-8000-000000000002", "Second"),
            9,
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            first.mutation.local_mutation_id,
            second.mutation.local_mutation_id
        );
        assert_eq!(second.first_generation, 4);
        assert_eq!(second.last_generation, 9);
        assert_eq!(load_writer(&conn).unwrap().unwrap(), writer);
        let restarted = load_captured_mutations(&conn).unwrap();
        assert_eq!(restarted.len(), 1);
        assert_eq!(restarted[0].mutation.value["name"], "Second");
    }

    #[test]
    fn capture_keeps_distinct_entities_and_complete_tombstone_descriptor() {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::setup_db(&conn).unwrap();
        capture_ordinary_mutation(
            &conn,
            &request("c1", "10000000-0000-4000-8000-000000000001", "One"),
            1,
        )
        .unwrap();
        let deleted = OrdinaryMutationRequestV1 {
            local_mutation_id: "20000000-0000-4000-8000-000000000002".into(),
            payload: OrdinaryPayloadV1::Tombstone(DeleteDescriptorV1::CollectionMember {
                id: sha256_hex(b"collection-member:v1\0c1\0r1"),
                collection_id: "c1".into(),
                record_id: "r1".into(),
                deleted_at: "2026-01-02T00:00:00.000Z".into(),
                rev: 2,
                rev_actor: "test".into(),
            }),
            causal_base: OrdinaryCausalBaseV1::Absent,
            base_frontier: Vec::new(),
        };
        let captured = capture_ordinary_mutation(&conn, &deleted, 2)
            .unwrap()
            .unwrap();
        assert_eq!(captured.mutation.operation, "tombstone");
        assert_eq!(captured.mutation.value["collectionId"], "c1");
        assert_eq!(captured.mutation.value["recordId"], "r1");
        assert_eq!(load_captured_mutations(&conn).unwrap().len(), 2);
    }

    #[test]
    fn malformed_writer_is_fail_closed() {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::setup_db(&conn).unwrap();
        initialize_writer(&conn).unwrap();
        conn.execute("UPDATE s2_lite_local_writer_v1 SET writer_id = 'bad'", [])
            .unwrap();
        assert!(load_writer(&conn).is_err());
    }

    #[test]
    fn staging_boundary_capture_keeps_first_anchor_and_identity_across_delete() {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::setup_db(&conn).unwrap();
        let first = capture_staged_descriptor(
            &conn,
            "collection",
            "c1",
            None,
            Some(serde_json::json!({"id":"c1", "name":"First"})),
            3,
        )
        .unwrap();
        let updated = capture_staged_descriptor(
            &conn,
            "collection",
            "c1",
            None,
            Some(serde_json::json!({"id":"c1", "name":"Second"})),
            7,
        )
        .unwrap();
        let deleted = capture_staged_descriptor(&conn, "collection", "c1", None, None, 8).unwrap();
        assert_eq!(first.local_mutation_id, updated.local_mutation_id);
        assert_eq!(updated.local_mutation_id, deleted.local_mutation_id);
        assert_eq!(deleted.causal_anchor, StagingAnchorStateV1::Unavailable);
        assert_eq!(deleted.first_generation, 3);
        assert_eq!(deleted.last_generation, 8);
        assert_eq!(deleted.operation, "delete");
    }
    #[test]
    fn semantic_revert_removes_mutation_across_restart_and_reedit_has_new_identity() {
        let path = std::env::temp_dir().join(format!("i61-cancel-{}.sqlite", uuid::Uuid::new_v4()));
        let mut conn = Connection::open(&path).unwrap();
        crate::db::setup_db(&conn).unwrap();
        let writer = initialize_writer(&conn).unwrap();
        let original = collection("c1", "Original");
        let original_wire = LocalEntityValueV1::Collection(original.clone())
            .wire_value()
            .unwrap();
        let mut changed = request("c1", &uuid::Uuid::new_v4().to_string(), "Changed");
        changed.causal_base = OrdinaryCausalBaseV1::Live(
            super::super::semantic::canonical_semantic_value(&original_wire).unwrap(),
        );
        let cancelled_id = capture_ordinary_mutation(&conn, &changed, 1)
            .unwrap()
            .unwrap()
            .mutation
            .local_mutation_id;
        let mut revert = request("c1", &uuid::Uuid::new_v4().to_string(), "Original");
        // Caller-supplied subsequent basis cannot replace the first live basis.
        revert.causal_base = OrdinaryCausalBaseV1::Absent;
        let tx = conn.transaction().unwrap();
        assert!(capture_ordinary_mutation(&tx, &revert, 2)
            .unwrap()
            .is_none());
        assert!(load_captured_mutations(&tx).unwrap().is_empty());
        tx.commit().unwrap();
        assert!(load_staged_descriptors(&conn).unwrap().is_empty());
        assert_eq!(load_writer(&conn).unwrap().unwrap(), writer);
        drop(conn);
        let conn = Connection::open(&path).unwrap();
        crate::db::setup_db(&conn).unwrap();
        assert!(load_captured_mutations(&conn).unwrap().is_empty());
        assert_eq!(load_writer(&conn).unwrap().unwrap(), writer);
        let new_id = uuid::Uuid::new_v4().to_string();
        let mut again = request("c1", &new_id, "ChangedAgain");
        again.causal_base = OrdinaryCausalBaseV1::Live(
            super::super::semantic::canonical_semantic_value(&original_wire).unwrap(),
        );
        let captured = capture_ordinary_mutation(&conn, &again, 3)
            .unwrap()
            .unwrap();
        assert_eq!(captured.mutation.local_mutation_id, new_id);
        assert_ne!(captured.mutation.local_mutation_id, cancelled_id);
        assert_eq!(captured.first_generation, 3);
        assert_eq!(load_captured_mutations(&conn).unwrap().len(), 1);
        drop(conn);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn delete_then_recreate_original_cancels_against_immutable_live_basis() {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::setup_db(&conn).unwrap();
        let original_wire = LocalEntityValueV1::Collection(collection("c1", "Original"))
            .wire_value()
            .unwrap();
        let delete = OrdinaryMutationRequestV1 {
            local_mutation_id: uuid::Uuid::new_v4().to_string(),
            payload: OrdinaryPayloadV1::Tombstone(DeleteDescriptorV1::Collection {
                id: "c1".into(),
                deleted_at: "2026-01-02T00:00:00.000Z".into(),
                rev: 2,
                rev_actor: "test".into(),
            }),
            causal_base: OrdinaryCausalBaseV1::Live(
                super::super::semantic::canonical_semantic_value(&original_wire).unwrap(),
            ),
            base_frontier: vec![],
        };
        capture_ordinary_mutation(&conn, &delete, 1)
            .unwrap()
            .unwrap();
        let recreate = request("c1", &uuid::Uuid::new_v4().to_string(), "Original");
        assert!(capture_ordinary_mutation(&conn, &recreate, 2)
            .unwrap()
            .is_none());
        assert!(load_captured_mutations(&conn).unwrap().is_empty());
    }

    #[test]
    fn semantic_cancellation_obeys_transaction_rollback_and_delete_failure() {
        let mut conn = Connection::open_in_memory().unwrap();
        crate::db::setup_db(&conn).unwrap();
        let mut changed = request("c1", &uuid::Uuid::new_v4().to_string(), "Changed");
        changed.causal_base = OrdinaryCausalBaseV1::Live(
            super::super::semantic::canonical_semantic_value(
                &LocalEntityValueV1::Collection(collection("c1", "Original"))
                    .wire_value()
                    .unwrap(),
            )
            .unwrap(),
        );
        let first = capture_ordinary_mutation(&conn, &changed, 1)
            .unwrap()
            .unwrap();
        let revert = request("c1", &uuid::Uuid::new_v4().to_string(), "Original");
        {
            let tx = conn.transaction().unwrap();
            assert!(capture_ordinary_mutation(&tx, &revert, 2)
                .unwrap()
                .is_none());
            assert!(load_captured_mutations(&tx).unwrap().is_empty());
            tx.rollback().unwrap();
        }
        assert_eq!(
            serde_json::to_value(&load_captured_mutations(&conn).unwrap()[0].mutation).unwrap(),
            serde_json::to_value(&first.mutation).unwrap()
        );
        conn.execute_batch("CREATE TRIGGER reject_cancel BEFORE DELETE ON s2_lite_local_mutation_v1 BEGIN SELECT RAISE(ABORT, 'injected'); END;").unwrap();
        assert!(capture_ordinary_mutation(&conn, &revert, 2).is_err());
        assert_eq!(
            serde_json::to_value(&load_captured_mutations(&conn).unwrap()[0].mutation).unwrap(),
            serde_json::to_value(&first.mutation).unwrap()
        );
    }
}
