//! Local-only durable authority for S2 Lite ordinary mutations.
//!
//! This is deliberately not a publication queue: it cannot allocate a writer
//! sequence, build a commit, discover a remote, or perform network I/O.

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::canonical::{jcs_bytes, validate_canonical_uuid_v4, ProtocolError, Result};
use super::ordinary_mutation::{
    map_ordinary_mutation_v1, OrdinaryCausalBaseV1, OrdinaryMutationRequestV1,
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
    pub causal_anchor: String,
    pub base: Option<Value>,
    pub local: Option<Value>,
    pub first_generation: i64,
    pub last_generation: i64,
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
    let value: Value = serde_json::from_slice(bytes).map_err(|_| CORRUPTION)?;
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
    let mut statement =
        conn.prepare("SELECT state_json FROM s2_lite_local_staging_descriptor_v1")?;
    for row in statement.query_map([], |row| row.get::<_, Vec<u8>>(0))? {
        decode_staging_descriptor(&row?).map_err(|_| rusqlite::Error::InvalidQuery)?;
    }
    Ok(())
}

fn decode_staging_descriptor(bytes: &[u8]) -> Result<CapturedStagingDescriptorV1> {
    let value: Value = serde_json::from_slice(bytes).map_err(|_| CORRUPTION)?;
    let decoded: CapturedStagingDescriptorV1 =
        serde_json::from_value(value.clone()).map_err(|_| CORRUPTION)?;
    if decoded.state_version != 1
        || !matches!(
            decoded.entity_kind.as_str(),
            "record" | "collection" | "collection-member"
        )
        || decoded.entity_id.trim().is_empty()
        || !matches!(decoded.operation.as_str(), "upsert" | "delete")
        || decoded.causal_anchor != "absent"
        || decoded.first_generation < 0
        || decoded.last_generation < decoded.first_generation
        || serde_json::to_value(&decoded).map_err(|_| CORRUPTION)? != value
    {
        return Err(CORRUPTION);
    }
    validate_canonical_uuid_v4(&decoded.local_mutation_id).map_err(|_| CORRUPTION)?;
    Ok(decoded)
}

/// Captures the existing S1 atomic staging boundary without changing its S1
/// payload or network behavior. The initially absent anchor is immutable until
/// a later projection phase can supply a verified remote frontier.
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
    let descriptor = if let Some(previous) = previous {
        CapturedStagingDescriptorV1 {
            operation: if local.is_some() {
                "upsert".into()
            } else {
                "delete".into()
            },
            local,
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
            causal_anchor: "absent".into(),
            base,
            local,
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
        assert_eq!(deleted.causal_anchor, "absent");
        assert_eq!(deleted.first_generation, 3);
        assert_eq!(deleted.last_generation, 8);
        assert_eq!(deleted.operation, "delete");
    }
}
