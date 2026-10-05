//! Root-bound read authority only. No writer head, publication, or cutover state.
//! Persistence envelopes follow the frozen desktop strict roundtrip model;
//! projections and conflict results are revalidated against retained exact bytes.
use std::collections::BTreeSet;

use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::Value;

use super::canonical::{validate_entity_key, ProtocolError, Result};
use super::causal::{decode_frozen_wire_commit_v1, replay_verified_history_v1};
use super::materialized_projection::{
    rebuild_materialized_projection_v1, resolve_ordinary_causal_base_v1,
    MaterializedProjectionStateV1, MaterializedProjectionStatusV1, OrdinaryCausalBaseResolutionV1,
};
use super::remote_discovery::*;

const CORRUPTION: ProtocolError = ProtocolError("S2_DISCOVERY_STATE_CORRUPTION");
const FAILURE: ProtocolError = ProtocolError("S2_DISCOVERY_PERSISTENCE_FAILURE");

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DurableDiscoveryV1 {
    pub persistence_version: u8,
    pub physical_root_id: String,
    pub storage_generation: u64,
    pub state: DiscoveryStateV1,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DurableReadProjectionV1 {
    pub persistence_version: u8,
    pub physical_root_id: String,
    pub source_discovery_generation: u64,
    pub state: MaterializedProjectionStateV1,
    /// Full frozen replay: validity/pending refs, provenance, forensic versions,
    /// entity conflicts, relation conflicts, alternatives, and diagnostics.
    pub replay: Value,
}

#[derive(Clone, Debug)]
pub struct DurableReadStateV1 {
    pub discovery: DurableDiscoveryV1,
    pub projection: DurableReadProjectionV1,
    pub fatal_codes: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum VerifiedAnchorResolutionV1 {
    Unavailable,
    Verified(OrdinaryCausalBaseResolutionV1),
}

fn db<T>(result: rusqlite::Result<T>) -> Result<T> {
    result.map_err(|_| FAILURE)
}
fn encode<T: Serialize>(value: &T) -> Result<Vec<u8>> {
    serde_json::to_vec(value).map_err(|_| CORRUPTION)
}
fn decode<T: DeserializeOwned + Serialize>(bytes: &[u8]) -> Result<T> {
    let value: Value = s2_serde_json::from_slice(bytes).map_err(|_| CORRUPTION)?;
    let decoded: T = serde_json::from_value(value.clone()).map_err(|_| CORRUPTION)?;
    if serde_json::to_value(&decoded).map_err(|_| CORRUPTION)? != value {
        return Err(CORRUPTION);
    }
    Ok(decoded)
}
fn valid_root(root: &str) -> Result<()> {
    let hash = root.strip_prefix("s2-root-v1:").ok_or(CORRUPTION)?;
    if hash.len() != 64
        || !hash
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(CORRUPTION);
    }
    Ok(())
}
fn generation(text: &str) -> Result<u64> {
    let number: u64 = text.parse().map_err(|_| CORRUPTION)?;
    if number.to_string() != text {
        return Err(CORRUPTION);
    }
    Ok(number)
}
fn unhex(text: &str) -> Result<Vec<u8>> {
    if !text.is_ascii()
        || text.len() % 2 != 0
        || !text
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(CORRUPTION);
    }
    (0..text.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&text[index..index + 2], 16).map_err(|_| CORRUPTION))
        .collect()
}

fn segment_name(value: &str) -> bool {
    value.len() == super::immutable_publish::SEGMENT_NAME_WIDTH_V1
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn writer_id(value: &str) -> bool {
    super::canonical::validate_canonical_uuid(value).is_ok()
}
fn segment_key(value: &str) -> bool {
    value
        .split_once('/')
        .is_some_and(|(writer, segment)| writer_id(writer) && segment_name(segment))
}
fn unique_strings(values: &[String]) -> bool {
    values.iter().collect::<BTreeSet<_>>().len() == values.len()
}
fn validate_progress(state: &DiscoveryStateV1) -> Result<()> {
    let cursors = &state.historical_audit_cursor;
    let valid_cursor = |(writer, segment): (&String, &Option<String>)| {
        writer_id(writer) && segment.as_deref().map_or(true, segment_name)
    };
    let exact_path = |path: &String| {
        parse_writer_candidate_path_v1(path).is_some()
            || parse_activation_candidate_path_v1(path).is_some()
    };
    if !unique_strings(&state.observed_writers)
        || !state.observed_writers.iter().all(|value| writer_id(value))
        || !unique_strings(&state.observed_segments)
        || !state
            .observed_segments
            .iter()
            .all(|value| segment_key(value))
        || !unique_strings(&state.historical_closed_segments)
        || !state
            .historical_closed_segments
            .iter()
            .all(|value| segment_key(value))
        || !unique_strings(&state.observed_activations)
        || !state
            .observed_activations
            .iter()
            .all(|value| parse_activation_candidate_path_v1(value).is_some())
        || !unique_strings(&state.known_gaps)
        || !state.known_gaps.iter().all(|value| {
            value.split_once('/').is_some_and(|(writer, seq)| {
                writer_id(writer) && super::canonical::parse_writer_seq(seq).is_ok()
            })
        })
        || !cursors.last_writer_id.as_deref().map_or(true, writer_id)
        || !cursors.last_segment_by_writer.iter().all(valid_cursor)
        || !state
            .gap_segment_cursor_by_writer
            .iter()
            .all(|(writer, key)| {
                writer_id(writer)
                    && key.as_ref().map_or(true, |key| {
                        segment_key(key) && key.starts_with(&format!("{writer}/"))
                    })
            })
        || state.exact_work_scheduler.after_by_class.len() != 3
        || !state
            .exact_work_scheduler
            .after_by_class
            .values()
            .all(|value| value.as_ref().map_or(true, exact_path))
        || !unique_strings(&state.reverification_queue)
        || !state.reverification_queue.iter().all(|path| {
            state
                .verified_objects
                .iter()
                .any(|object| object.path == *path)
        })
        || !unique_strings(&state.terminal_candidate_paths)
        || !state.terminal_candidate_paths.iter().all(|path| {
            super::remote_discovery::classify_candidate_path_v1(path)
                != CandidatePathClassificationV1::UnrelatedJunk
        })
    {
        return Err(CORRUPTION);
    }
    Ok(())
}

fn validate_discovery(state: &DiscoveryStateV1) -> Result<()> {
    if state.state_version != 1 {
        return Err(CORRUPTION);
    }
    validate_progress(state)?;
    let mut paths = BTreeSet::new();
    for candidate in &state.observed_candidates {
        let parsed = parse_writer_candidate_path_v1(candidate.path())
            .or_else(|| parse_activation_candidate_path_v1(candidate.path()));
        let retained_observation = match candidate {
            ObservedCandidateV1::Activation { path, .. } => {
                state.observed_activations.contains(path)
            }
            ObservedCandidateV1::Commit {
                writer_id,
                segment_name,
                ..
            } => {
                state.observed_writers.contains(writer_id)
                    && state
                        .observed_segments
                        .contains(&format!("{writer_id}/{segment_name}"))
            }
        };
        if !retained_observation
            || parsed.as_ref() != Some(candidate)
            || !paths.insert(candidate.path())
        {
            return Err(CORRUPTION);
        }
    }
    let mut verified_paths = BTreeSet::new();
    let mut aggregate = create_discovery_state_v1();
    for object in &state.verified_objects {
        if !verified_paths.insert(&object.path) {
            return Err(CORRUPTION);
        }
        let candidate = state
            .observed_candidates
            .iter()
            .find(|candidate| candidate.path() == object.path)
            .ok_or(CORRUPTION)?;
        let bytes = unhex(&object.exact_bytes_hex)?;
        let mut checked = create_discovery_state_v1();
        match candidate {
            ObservedCandidateV1::Commit { .. } => {
                verify_commit_candidate_v1(&mut checked, candidate, &bytes)
                    .map_err(|_| CORRUPTION)?;
            }
            ObservedCandidateV1::Activation { .. } => {
                let activation =
                    validate_production_activation_body_v1(&bytes).map_err(|_| CORRUPTION)?;
                verify_activation_candidate_v1(&mut checked, candidate, &bytes, &activation)
                    .map_err(|_| CORRUPTION)?;
            }
        }
        match candidate {
            ObservedCandidateV1::Commit { .. } => {
                verify_commit_candidate_v1(&mut aggregate, candidate, &bytes)
                    .map_err(|_| CORRUPTION)?;
            }
            ObservedCandidateV1::Activation { .. } => {
                let activation =
                    validate_production_activation_body_v1(&bytes).map_err(|_| CORRUPTION)?;
                verify_activation_candidate_v1(&mut aggregate, candidate, &bytes, &activation)
                    .map_err(|_| CORRUPTION)?;
            }
        }
        if checked.verified_objects.as_slice() != [object.clone()] {
            return Err(CORRUPTION);
        }
    }
    // `terminal_candidate_paths` is scheduler suppression authority, not a
    // cache.  Frozen discovery adds an entry only after an exact GET returned
    // bytes for an already retained candidate and verification produced an
    // exact-path terminal fatal.  A persisted marker without that durable
    // evidence could otherwise suppress an unresolved retained observation
    // forever after restart.
    const TERMINAL_CANDIDATE_FATALS: &[&str] = &[
        "REMOTE_IMMUTABLE_PATH_CONTENT_MISMATCH",
        "REMOTE_S2_OBJECT_INVALID",
        "REMOTE_S2_PATH_BODY_IDENTITY_MISMATCH",
        "REMOTE_S2_UNSUPPORTED_FEATURE",
    ];
    for path in &state.terminal_candidate_paths {
        if !state
            .observed_candidates
            .iter()
            .any(|candidate| candidate.path() == path)
            || !state.root_fatal_signals.iter().any(|signal| {
                signal.path == *path && TERMINAL_CANDIDATE_FATALS.contains(&signal.code.as_str())
            })
        {
            return Err(CORRUPTION);
        }
    }
    // Missing dependency targets are derived from verified bytes, not a
    // mutable progress shortcut. Corrupt queue removal must not starve replay.
    if aggregate
        .targeted_queue
        .iter()
        .any(|reference| !state.targeted_queue.contains(reference))
    {
        return Err(CORRUPTION);
    }
    if aggregate.root_fatal_signals.iter().any(|signal| {
        !state.root_fatal_signals.iter().any(|stored| {
            stored.code == signal.code
                && stored.writer_id == signal.writer_id
                && stored.writer_seq == signal.writer_seq
        })
    }) {
        return Err(CORRUPTION);
    }
    if state
        .root_fatal_signals
        .iter()
        .any(|signal| signal.code.is_empty() || signal.path.is_empty())
    {
        return Err(CORRUPTION);
    }
    for reference in &state.targeted_queue {
        super::canonical::validate_commit_ref(reference).map_err(|_| CORRUPTION)?;
    }
    Ok(())
}

fn derive(root: &str, gen: u64, state: &DiscoveryStateV1) -> Result<DurableReadProjectionV1> {
    let bytes = retained_verified_commit_bytes_v1(state)?;
    let commits = bytes
        .iter()
        .map(|bytes| decode_frozen_wire_commit_v1(bytes))
        .collect::<Result<Vec<_>>>()?;
    Ok(DurableReadProjectionV1 {
        persistence_version: 1,
        physical_root_id: root.into(),
        source_discovery_generation: gen,
        state: rebuild_materialized_projection_v1(state)?,
        replay: serde_json::to_value(replay_verified_history_v1(&commits)?)
            .map_err(|_| CORRUPTION)?,
    })
}

pub(crate) fn migrate_schema(conn: &Connection) -> rusqlite::Result<()> {
    let transaction = conn.unchecked_transaction()?;
    let conn = &transaction;
    let version: Option<String> = conn
        .query_row(
            "SELECT value FROM settings WHERE key='s2_lite_read_authority_schema_version'",
            [],
            |row| row.get(0),
        )
        .optional()?;
    if version.as_deref().is_some_and(|value| value != "1") {
        return Err(rusqlite::Error::InvalidQuery);
    }
    let table_count: u32 = conn.query_row("SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name IN ('s2_lite_read_root_v1','s2_lite_discovery_v1','s2_lite_read_projection_v1')",[],|row|row.get(0))?;
    if (version.is_none() && table_count != 0) || (version.is_some() && table_count != 3) {
        return Err(rusqlite::Error::InvalidQuery);
    }
    conn.execute_batch("CREATE TABLE IF NOT EXISTS s2_lite_read_root_v1 (
        root_id TEXT PRIMARY KEY NOT NULL, fatal_json BLOB NOT NULL CHECK(typeof(fatal_json)='blob'));
        CREATE TABLE IF NOT EXISTS s2_lite_discovery_v1 (
        root_id TEXT PRIMARY KEY NOT NULL REFERENCES s2_lite_read_root_v1(root_id) ON DELETE RESTRICT,
        storage_generation TEXT NOT NULL, state_json BLOB NOT NULL CHECK(typeof(state_json)='blob'));
        CREATE TABLE IF NOT EXISTS s2_lite_read_projection_v1 (
        root_id TEXT PRIMARY KEY NOT NULL REFERENCES s2_lite_discovery_v1(root_id) ON DELETE RESTRICT,
        source_generation TEXT NOT NULL, state_json BLOB NOT NULL CHECK(typeof(state_json)='blob'));
        INSERT INTO settings(key,value) VALUES('s2_lite_read_authority_schema_version','1') ON CONFLICT(key) DO NOTHING;")?;
    let mut statement = conn.prepare("SELECT root_id FROM s2_lite_read_root_v1 UNION SELECT root_id FROM s2_lite_discovery_v1 UNION SELECT root_id FROM s2_lite_read_projection_v1")?;
    let roots = statement
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    drop(statement);
    for root in roots {
        load_read_state_v1(conn, &root).map_err(|_| rusqlite::Error::InvalidQuery)?;
    }
    transaction.commit()?;
    Ok(())
}

pub fn load_read_state_v1(conn: &Connection, root: &str) -> Result<Option<DurableReadStateV1>> {
    valid_root(root)?;
    // One SELECT snapshot, including all three bound rows. Partial state is
    // corruption even when foreign key enforcement was disabled externally.
    let row = db(conn
        .query_row(
            "SELECT r.fatal_json,d.storage_generation,d.state_json,p.source_generation,p.state_json
        FROM s2_lite_read_root_v1 r LEFT JOIN s2_lite_discovery_v1 d ON d.root_id=r.root_id
        LEFT JOIN s2_lite_read_projection_v1 p ON p.root_id=r.root_id WHERE r.root_id=?1",
            [root],
            |row| {
                Ok((
                    row.get::<_, Vec<u8>>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<Vec<u8>>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, Option<Vec<u8>>>(4)?,
                ))
            },
        )
        .optional())?;
    let Some((fatal, gen, discovery, source, projection)) = row else {
        let orphan: bool = db(conn.query_row("SELECT EXISTS(SELECT 1 FROM s2_lite_discovery_v1 WHERE root_id=?1 UNION ALL SELECT 1 FROM s2_lite_read_projection_v1 WHERE root_id=?1)",[root],|row| row.get(0)))?;
        return if orphan { Err(CORRUPTION) } else { Ok(None) };
    };
    let gen = generation(&gen.ok_or(CORRUPTION)?)?;
    let discovery: DurableDiscoveryV1 = decode(&discovery.ok_or(CORRUPTION)?)?;
    let projection: DurableReadProjectionV1 = decode(&projection.ok_or(CORRUPTION)?)?;
    let fatal_codes: Vec<String> = decode(&fatal)?;
    if discovery.persistence_version != 1
        || discovery.physical_root_id != root
        || discovery.storage_generation != gen
        || generation(&source.ok_or(CORRUPTION)?)? != gen
        || fatal_codes.iter().any(|code| code.is_empty())
        || fatal_codes.windows(2).any(|pair| pair[0] >= pair[1])
    {
        return Err(CORRUPTION);
    }
    validate_discovery(&discovery.state)?;
    if projection != derive(root, gen, &discovery.state)? {
        return Err(CORRUPTION);
    }
    if discovery
        .state
        .root_fatal_signals
        .iter()
        .any(|signal| !fatal_codes.contains(&signal.code))
        || matches!(&projection.state.status, MaterializedProjectionStatusV1::Fatal {code} if !fatal_codes.contains(code))
    {
        return Err(CORRUPTION);
    }
    Ok(Some(DurableReadStateV1 {
        discovery,
        projection,
        fatal_codes,
    }))
}

/// Atomic discovery + replay + conflicts + fatal latch, bound to a physical
/// root and compared against the generation loaded before remote I/O.
pub(crate) fn save_round_v1(
    conn: &mut Connection,
    root: &str,
    expected: Option<u64>,
    state: DiscoveryStateV1,
) -> Result<DurableReadStateV1> {
    valid_root(root)?;
    validate_discovery(&state)?;
    let transaction = db(conn.transaction_with_behavior(TransactionBehavior::Immediate))?;
    let previous = load_read_state_v1(&transaction, root)?;
    if previous
        .as_ref()
        .map(|value| value.discovery.storage_generation)
        != expected
    {
        return Err(ProtocolError("S2_DISCOVERY_STALE_GENERATION"));
    }
    if let Some(prior) = &previous {
        // Evidence only grows. Queues/cursors are mutable progress, while
        // observations, verified bytes and root-fatal facts cannot disappear.
        let before = &prior.discovery.state;
        if before
            .observed_writers
            .iter()
            .any(|value| !state.observed_writers.contains(value))
            || before
                .observed_segments
                .iter()
                .any(|value| !state.observed_segments.contains(value))
            || before
                .observed_activations
                .iter()
                .any(|value| !state.observed_activations.contains(value))
            || before
                .historical_closed_segments
                .iter()
                .any(|value| !state.historical_closed_segments.contains(value))
            || prior
                .discovery
                .state
                .verified_objects
                .iter()
                .any(|object| !state.verified_objects.contains(object))
            || prior
                .discovery
                .state
                .observed_candidates
                .iter()
                .any(|candidate| !state.observed_candidates.contains(candidate))
            || prior
                .discovery
                .state
                .root_fatal_signals
                .iter()
                .any(|signal| {
                    !state.root_fatal_signals.iter().any(|current| {
                        current.code == signal.code
                            && current.writer_id == signal.writer_id
                            && current.writer_seq == signal.writer_seq
                    })
                })
        {
            return Err(CORRUPTION);
        }
    }
    let gen = expected.unwrap_or(0).checked_add(1).ok_or(CORRUPTION)?;
    let projection = derive(root, gen, &state)?;
    let mut fatal_codes = previous.map(|value| value.fatal_codes).unwrap_or_default();
    fatal_codes.extend(
        state
            .root_fatal_signals
            .iter()
            .map(|signal| signal.code.clone()),
    );
    if let MaterializedProjectionStatusV1::Fatal { code } = &projection.state.status {
        fatal_codes.push(code.clone());
    }
    fatal_codes.sort();
    fatal_codes.dedup();
    let discovery = DurableDiscoveryV1 {
        persistence_version: 1,
        physical_root_id: root.into(),
        storage_generation: gen,
        state,
    };
    db(transaction.execute("INSERT INTO s2_lite_read_root_v1(root_id,fatal_json) VALUES(?1,?2) ON CONFLICT(root_id) DO UPDATE SET fatal_json=excluded.fatal_json",params![root,encode(&fatal_codes)?]))?;
    db(transaction.execute("INSERT INTO s2_lite_discovery_v1(root_id,storage_generation,state_json) VALUES(?1,?2,?3) ON CONFLICT(root_id) DO UPDATE SET storage_generation=excluded.storage_generation,state_json=excluded.state_json",params![root,gen.to_string(),encode(&discovery)?]))?;
    db(transaction.execute("INSERT INTO s2_lite_read_projection_v1(root_id,source_generation,state_json) VALUES(?1,?2,?3) ON CONFLICT(root_id) DO UPDATE SET source_generation=excluded.source_generation,state_json=excluded.state_json",params![root,gen.to_string(),encode(&projection)?]))?;
    db(transaction.commit())?;
    Ok(DurableReadStateV1 {
        discovery,
        projection,
        fatal_codes,
    })
}

/// Queries verified causal evidence without touching local captured mutations.
/// Unknown entity keys do not become absent merely because replay is complete.
pub fn resolve_verified_anchor_v1(
    conn: &Connection,
    root: &str,
    key: &Value,
) -> Result<VerifiedAnchorResolutionV1> {
    validate_entity_key(key)?;
    let Some(snapshot) = load_read_state_v1(conn, root)? else {
        return Ok(VerifiedAnchorResolutionV1::Unavailable);
    };
    if !snapshot.fatal_codes.is_empty() {
        return Ok(VerifiedAnchorResolutionV1::Verified(
            OrdinaryCausalBaseResolutionV1::Fatal,
        ));
    }
    if !snapshot
        .projection
        .state
        .entities
        .iter()
        .any(|entity| entity.entity_key == *key)
    {
        return Ok(VerifiedAnchorResolutionV1::Unavailable);
    }
    Ok(VerifiedAnchorResolutionV1::Verified(
        resolve_ordinary_causal_base_v1(&snapshot.projection.state, key)?,
    ))
}

/// Execution readiness over retained discovery knowledge. A clean projection
/// alone cannot establish that unresolved remote observations are safe to ignore.
pub(crate) fn publication_discovery_ready_v1(
    state: &super::remote_discovery::DiscoveryStateV1,
) -> bool {
    !state.last_round_indeterminate
        && state.known_gaps.is_empty()
        && state.targeted_queue.is_empty()
        && state.observed_activations.iter().all(|path| {
            state
                .verified_objects
                .iter()
                .any(|object| object.path == *path)
        })
        && state.observed_candidates.iter().all(|candidate| {
            state
                .verified_objects
                .iter()
                .any(|object| object.path == candidate.path())
                || state
                    .terminal_candidate_paths
                    .iter()
                    .any(|path| path == candidate.path())
        })
}
