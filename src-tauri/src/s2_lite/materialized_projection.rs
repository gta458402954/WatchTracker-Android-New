//! Rebuildable local cache over retained verified exact commits.
//!
//! This module never treats its result as authority. The authority remains the
//! durable discovery facts and frozen causal replay from which it is rebuilt.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::canonical::{compare_commit_ref_v1, jcs_bytes, sha256_hex, Result};
use super::causal::{decode_frozen_wire_commit_v1, replay_verified_history_v1};
use super::ordinary_mutation::OrdinaryCausalBaseV1;
use super::remote_discovery::{retained_verified_commit_bytes_v1, DiscoveryStateV1};
use super::types::{CommitRef, HistoricalValidityState, MaterializedEntityV1};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", tag = "status")]
pub enum MaterializedProjectionStatusV1 {
    Complete,
    PendingDependencies { pending_refs: Vec<CommitRef> },
    Fatal { code: String },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MaterializedProjectionEntityV1 {
    pub entity_key: Value,
    pub semantic_state: Option<Value>,
    pub business_value: Option<Value>,
    pub frontier: Vec<CommitRef>,
    pub conflict: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MaterializedProjectionStateV1 {
    pub state_version: u8,
    pub status: MaterializedProjectionStatusV1,
    pub basis_clock: Vec<CommitRef>,
    pub entities: Vec<MaterializedProjectionEntityV1>,
    pub relation_blocked_entity_keys: Vec<Value>,
    pub replay_input_fingerprint: String,
}

#[derive(Clone, Debug, PartialEq)]
pub enum OrdinaryCausalBaseResolutionV1 {
    Ready {
        causal_base: OrdinaryCausalBaseV1,
        base_frontier: Vec<CommitRef>,
        basis_clock: Vec<CommitRef>,
    },
    PendingDependencies,
    ConflictBlocked,
    Fatal,
}

fn key_id(value: &Value) -> Result<Vec<u8>> {
    jcs_bytes(value)
}

fn sorted_unique_refs(mut refs: Vec<CommitRef>) -> Vec<CommitRef> {
    refs.sort_by(compare_commit_ref_v1);
    refs.dedup_by(|left, right| compare_commit_ref_v1(left, right).is_eq());
    refs
}

/// Replays exact retained bytes. Any pending validity is deliberately not
/// collapsed to absence, and any invalid/forked history is fatal for outbound
/// publication.
pub fn rebuild_materialized_projection_v1(
    discovery: &DiscoveryStateV1,
) -> Result<MaterializedProjectionStateV1> {
    let bytes = retained_verified_commit_bytes_v1(discovery)?;
    let fingerprint = sha256_hex(&jcs_bytes(&bytes)?);
    let commits = bytes
        .iter()
        .map(|bytes| decode_frozen_wire_commit_v1(bytes))
        .collect::<Result<Vec<_>>>()?;
    let replay = replay_verified_history_v1(&commits)?;
    let pending_refs = replay
        .validity
        .iter()
        .filter(|entry| entry.validity.state == HistoricalValidityState::Pending)
        .map(|entry| entry.commit_ref.clone())
        .collect::<Vec<_>>();
    let invalid = replay
        .validity
        .iter()
        .any(|entry| entry.validity.state == HistoricalValidityState::Invalid);
    let status = if invalid || !replay.forks.is_empty() || !replay.unsafe_commit_refs.is_empty() {
        MaterializedProjectionStatusV1::Fatal {
            code: "verified_replay_fatal".to_string(),
        }
    } else if !pending_refs.is_empty() {
        MaterializedProjectionStatusV1::PendingDependencies { pending_refs }
    } else {
        MaterializedProjectionStatusV1::Complete
    };
    let valid_refs = replay
        .validity
        .iter()
        .filter(|entry| entry.validity.state == HistoricalValidityState::Valid)
        .map(|entry| entry.commit_ref.clone())
        .collect::<Vec<_>>();
    let mut maximal_by_writer = BTreeMap::<String, CommitRef>::new();
    for reference in valid_refs {
        maximal_by_writer
            .entry(reference.writer_id.clone())
            .and_modify(|current| {
                if compare_commit_ref_v1(current, &reference).is_lt() {
                    *current = reference.clone();
                }
            })
            .or_insert(reference);
    }
    let basis_clock = sorted_unique_refs(maximal_by_writer.into_values().collect());
    let mut relation_blocked = BTreeSet::<Vec<u8>>::new();
    let mut relation_keys = Vec::new();
    for conflict in &replay.relations.conflicts {
        for key in &conflict.core.entity_keys {
            if relation_blocked.insert(key_id(key)?) {
                relation_keys.push(key.clone());
            }
        }
    }
    for key in &replay.relations.blocked_by_entity_conflict {
        if relation_blocked.insert(key_id(key)?) {
            relation_keys.push(key.clone());
        }
    }
    relation_keys.sort_by_key(|key| key_id(key).unwrap());
    let frontiers = replay
        .frontiers
        .iter()
        .map(|item| Ok((key_id(&item.entity_key)?, item.frontier.clone())))
        .collect::<Result<BTreeMap<_, _>>>()?;
    let mut entities = replay
        .materialized
        .iter()
        .map(|item| {
            let frontier = frontiers
                .get(&key_id(&item.entity_key)?)
                .cloned()
                .unwrap_or_default();
            let (semantic_state, business_value, conflict) = match &item.value {
                MaterializedEntityV1::Absent => (None, None, false),
                MaterializedEntityV1::Resolved {
                    semantic_state,
                    business_value,
                    ..
                } => (Some(semantic_state.clone()), business_value.clone(), false),
                MaterializedEntityV1::Conflict { .. } => (None, None, true),
            };
            Ok(MaterializedProjectionEntityV1 {
                entity_key: item.entity_key.clone(),
                semantic_state,
                business_value,
                frontier,
                conflict,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    entities.sort_by(|left, right| {
        key_id(&left.entity_key)
            .unwrap()
            .cmp(&key_id(&right.entity_key).unwrap())
    });
    Ok(MaterializedProjectionStateV1 {
        state_version: 1,
        status,
        basis_clock,
        entities,
        relation_blocked_entity_keys: relation_keys,
        replay_input_fingerprint: fingerprint,
    })
}

pub fn resolve_ordinary_causal_base_v1(
    projection: &MaterializedProjectionStateV1,
    entity_key: &Value,
) -> Result<OrdinaryCausalBaseResolutionV1> {
    match projection.status {
        MaterializedProjectionStatusV1::PendingDependencies { .. } => {
            return Ok(OrdinaryCausalBaseResolutionV1::PendingDependencies)
        }
        MaterializedProjectionStatusV1::Fatal { .. } => {
            return Ok(OrdinaryCausalBaseResolutionV1::Fatal)
        }
        MaterializedProjectionStatusV1::Complete => {}
    }
    let id = key_id(entity_key)?;
    if projection
        .relation_blocked_entity_keys
        .iter()
        .any(|key| key_id(key).is_ok_and(|candidate| candidate == id))
    {
        return Ok(OrdinaryCausalBaseResolutionV1::ConflictBlocked);
    }
    let Some(entity) = projection
        .entities
        .iter()
        .find(|item| key_id(&item.entity_key).is_ok_and(|candidate| candidate == id))
    else {
        return Ok(OrdinaryCausalBaseResolutionV1::Ready {
            causal_base: OrdinaryCausalBaseV1::Absent,
            base_frontier: vec![],
            basis_clock: projection.basis_clock.clone(),
        });
    };
    if entity.conflict {
        return Ok(OrdinaryCausalBaseResolutionV1::ConflictBlocked);
    }
    let causal_base = match &entity.semantic_state {
        Some(state) if state.get("state") == Some(&Value::String("live".to_string())) => {
            OrdinaryCausalBaseV1::Live(entity.business_value.clone().unwrap_or(Value::Null))
        }
        Some(state) if state.get("state") == Some(&Value::String("tombstone".to_string())) => {
            OrdinaryCausalBaseV1::Tombstone
        }
        _ => OrdinaryCausalBaseV1::Absent,
    };
    Ok(OrdinaryCausalBaseResolutionV1::Ready {
        causal_base,
        base_frontier: entity.frontier.clone(),
        basis_clock: projection.basis_clock.clone(),
    })
}
