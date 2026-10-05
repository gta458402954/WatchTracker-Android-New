//! Projection-bound, zero-network ordinary outbound freezing.
//!
//! This transaction prepares immutable publication without network access or
//! advancing the published head. Proven semantic no-ops may retire locally;
//! actual mutations require a verified durable receipt before retirement.

use std::sync::Mutex;

use rusqlite::Connection;
use serde_json::json;

use super::canonical::{jcs_bytes, ProtocolError, Result};
use super::durable_persistence::{
    OutboundBatchMutationV1, OutboundBatchV1, OutboundFreezeTransactionContextV1,
    OutboundFreezeTransactionPlanV1, OutboundFreezeTransactionResultV1, SqliteS2LiteStoreV1,
};
use super::immutable_publish::{prepare_commit_intent_v1, PreparedIntentV1};
use super::materialized_projection::{
    resolve_ordinary_causal_base_v1, OrdinaryCausalBaseResolutionV1,
};
use super::ordinary_mutation::{
    map_ordinary_mutation_v1, sort_ordinary_mutations_v1, LocalCollectionMemberV1,
    LocalCollectionV1, LocalEntityValueV1, LocalEpisodeCompletionV1, LocalRecordV1,
    OrdinaryMutationRequestV1, OrdinaryPayloadV1,
};
use super::types::CommitRef;

const FREEZE_FAILURE: ProtocolError = ProtocolError("S2_OUTBOUND_FREEZE_FAILURE");

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum OutboundFreezeResultV1 {
    Frozen {
        batch: Box<OutboundBatchV1>,
        intent: Box<PreparedIntentV1>,
    },
    ExistingPendingOutbound,
    NoSemanticMutation,
    TargetChanged,
    Blocked,
    BlockedStaleEntityBases,
}

fn staged_payload(
    entry: &super::local_authority::CapturedStagingDescriptorV1,
) -> Result<OrdinaryPayloadV1> {
    if entry.operation == "delete" {
        return Ok(OrdinaryPayloadV1::Tombstone(
            entry.frozen_delete_evidence()?.clone(),
        ));
    }
    let local = entry.local.clone().ok_or(FREEZE_FAILURE)?;
    let value = match entry.entity_kind.as_str() {
        "record" => LocalEntityValueV1::Record(Box::new(
            serde_json::from_value::<LocalRecordV1>(local).map_err(|_| FREEZE_FAILURE)?,
        )),
        "collection" => LocalEntityValueV1::Collection(
            serde_json::from_value::<LocalCollectionV1>(local).map_err(|_| FREEZE_FAILURE)?,
        ),
        "collection-member" => LocalEntityValueV1::CollectionMember(
            serde_json::from_value::<LocalCollectionMemberV1>(local).map_err(|_| FREEZE_FAILURE)?,
        ),
        "episode-completion" => LocalEntityValueV1::EpisodeCompletion(
            serde_json::from_value::<LocalEpisodeCompletionV1>(local)
                .map_err(|_| FREEZE_FAILURE)?,
        ),
        _ => return Err(FREEZE_FAILURE),
    };
    Ok(OrdinaryPayloadV1::Upsert(value))
}

fn commit_bytes(
    writer_id: String,
    writer_seq: u64,
    commit_id: String,
    previous_writer_commit: Option<CommitRef>,
    basis_clock: Vec<CommitRef>,
    mutations: Vec<super::types::CommitMutationV1>,
    created_at: &str,
) -> Result<Vec<u8>> {
    // The frozen decoder derives contentHash from the exact bytes; it must not
    // appear in the self-hashed wire object.
    let value = json!({
        "protocol": "watchtracker-s2-lite",
        "protocolVersion": 1,
        "s2SemanticProfileVersion": 1,
        "requiredFeatures": [],
        "writerId": writer_id,
        "writerSeq": writer_seq.to_string(),
        "commitId": commit_id,
        "previousWriterCommit": previous_writer_commit,
        "basisClock": basis_clock,
        "commitKind": "mutation",
        "createdAt": created_at,
        "source": { "type": "native" },
        "mutations": mutations,
    });
    jcs_bytes(&value)
}

/// Freezes one root-bound batch. It uses no remote type and therefore cannot
/// make a network call. A later checkpoint alone may consume the resulting
/// intent.
pub fn freeze_active_outbound_v1(
    conn: &Mutex<Connection>,
    target_id: &str,
    target_epoch: u64,
    created_at_diagnostic: &str,
) -> Result<OutboundFreezeResultV1> {
    let root_id = match active_root_id(conn, target_id, target_epoch) {
        Ok(value) => value,
        Err(_) => return Ok(OutboundFreezeResultV1::TargetChanged),
    };
    let mut store = SqliteS2LiteStoreV1::open(conn, &root_id)?;
    let result = store.run_outbound_freeze_transaction(target_id, target_epoch, |context| {
        build_freeze_plan(context, created_at_diagnostic)
    })?;
    Ok(map_transaction_result(result))
}

fn active_root_id(conn: &Mutex<Connection>, target_id: &str, target_epoch: u64) -> Result<String> {
    let guard = conn.lock().map_err(|_| FREEZE_FAILURE)?;
    let registry = crate::sync_targets::registry(&guard).map_err(|_| FREEZE_FAILURE)?;
    let registry = registry.ok_or(FREEZE_FAILURE)?;
    if registry.active_target_id.as_deref() != Some(target_id)
        || registry.target_epoch != target_epoch
    {
        return Err(FREEZE_FAILURE);
    }
    let target = registry
        .targets
        .iter()
        .find(|target| target.id == target_id)
        .ok_or(FREEZE_FAILURE)?;
    Ok(
        super::webdav_adapter::webdav_root_v1(&target.normalized_url, &target.username)
            .map_err(|_| FREEZE_FAILURE)?
            .physical_root_id,
    )
}

fn build_freeze_plan(
    context: &OutboundFreezeTransactionContextV1,
    created_at_diagnostic: &str,
) -> Result<OutboundFreezeTransactionPlanV1> {
    let mut mutations = Vec::new();
    let mut covered = Vec::new();
    let mut basis_clock: Option<Vec<CommitRef>> = None;
    for entry in &context.descriptors {
        let key = super::local_authority::descriptor_entity_key(entry)?;
        let resolution = resolve_ordinary_causal_base_v1(&context.projection.state, &key)?;
        let OrdinaryCausalBaseResolutionV1::Ready {
            causal_base,
            base_frontier,
            basis_clock: entity_basis,
        } = resolution
        else {
            return Ok(OutboundFreezeTransactionPlanV1::Blocked);
        };
        let Some(proof) = &entry.verified_basis else {
            return Ok(OutboundFreezeTransactionPlanV1::BlockedStaleEntityBases);
        };
        let matches_base = match (&entry.causal_anchor, &causal_base) {
            (
                super::local_authority::StagingAnchorStateV1::Live { value },
                super::ordinary_mutation::OrdinaryCausalBaseV1::Live(current),
            ) => value == current,
            (
                super::local_authority::StagingAnchorStateV1::Absent,
                super::ordinary_mutation::OrdinaryCausalBaseV1::Absent
                | super::ordinary_mutation::OrdinaryCausalBaseV1::Tombstone,
            ) => true,
            _ => false,
        };
        if !matches_base
            || proof.physical_root_id != context.binding.physical_root_id
            || proof.base_frontier != base_frontier
        {
            return Ok(OutboundFreezeTransactionPlanV1::BlockedStaleEntityBases);
        }
        if let Some(expected) = &basis_clock {
            if expected != &entity_basis {
                return Err(FREEZE_FAILURE);
            }
        } else {
            basis_clock = Some(entity_basis);
        }
        // A locally-created entity which is deleted before it ever enters the
        // projection has no remote semantic state to tombstone.  Its durable
        // delete evidence remains in staging for local bookkeeping, but it
        // must not manufacture a remote deletion.
        if entry.operation == "delete"
            && matches!(
                causal_base,
                super::ordinary_mutation::OrdinaryCausalBaseV1::Absent
            )
        {
            continue;
        }
        let local_mutation_id = entry.local_mutation_id.clone();
        let mapped = map_ordinary_mutation_v1(&OrdinaryMutationRequestV1 {
            local_mutation_id: local_mutation_id.clone(),
            payload: staged_payload(entry)?,
            causal_base,
            base_frontier,
        })?;
        if let Some(mapped) = mapped {
            covered.push(OutboundBatchMutationV1 {
                entity_kind: entry.entity_kind.clone(),
                entity_id: entry.entity_id.clone(),
                entity_key: key,
                captured_last_generation: entry.last_generation,
                local_mutation_id,
            });
            mutations.push(mapped);
        }
    }
    if mutations.is_empty() {
        return Ok(OutboundFreezeTransactionPlanV1::NoSemanticMutation);
    }
    sort_ordinary_mutations_v1(&mut mutations)?;
    covered.sort_by(|left, right| {
        super::canonical::compare_entity_key_v1(&left.entity_key, &right.entity_key)
    });
    let basis_clock = basis_clock.ok_or(FREEZE_FAILURE)?;
    let writer_seq = context.root_state.next_writer_sequence;
    let previous = context.root_state.writer_head.clone();
    if writer_seq == 1 {
        if previous.is_some()
            || basis_clock
                .iter()
                .any(|item| item.writer_id == context.root_state.local_writer_id)
        {
            return Ok(OutboundFreezeTransactionPlanV1::Blocked);
        }
    } else {
        let previous = previous.as_ref().ok_or(FREEZE_FAILURE)?;
        if previous.writer_id != context.root_state.local_writer_id
            || !basis_clock.iter().any(|item| item == previous)
        {
            return Ok(OutboundFreezeTransactionPlanV1::Blocked);
        }
    }
    let exact_bytes = commit_bytes(
        context.root_state.local_writer_id.clone(),
        writer_seq,
        uuid::Uuid::new_v4().to_string(),
        previous.clone(),
        basis_clock.clone(),
        mutations,
        created_at_diagnostic,
    )?;
    let intent = prepare_commit_intent_v1(&exact_bytes, created_at_diagnostic)?;
    let batch = OutboundBatchV1 {
        state_version: 1,
        batch_id: uuid::Uuid::new_v4().to_string(),
        target_id: context.binding.target_id.clone(),
        target_epoch: context.binding.target_epoch,
        physical_root_id: context.binding.physical_root_id.clone(),
        projection_generation: context.projection.projection_generation,
        source_discovery_generation: context.discovery_generation,
        source_root_safety_generation: context.root_safety_generation,
        captured_local_generation: covered
            .iter()
            .map(|item| item.captured_last_generation)
            .max()
            .unwrap_or(0),
        mutations: covered,
        basis_clock,
        writer_id: context.root_state.local_writer_id.clone(),
        writer_sequence: writer_seq,
        previous_writer_ref: previous,
        commit_ref: intent.commit_ref.clone(),
        prepared_intent_path: intent.remote_path.clone(),
        prepared_intent_fingerprint: intent.intent_fingerprint.clone(),
        state: "frozen".into(),
        bookkeeping_completed: false,
        bookkeeping_generation: 0,
    };
    Ok(OutboundFreezeTransactionPlanV1::Frozen {
        batch: Box::new(batch),
        intent: Box::new(intent),
    })
}

fn map_transaction_result(result: OutboundFreezeTransactionResultV1) -> OutboundFreezeResultV1 {
    match result {
        OutboundFreezeTransactionResultV1::Frozen { batch, intent } => {
            OutboundFreezeResultV1::Frozen { batch, intent }
        }
        OutboundFreezeTransactionResultV1::ExistingPendingOutbound => {
            OutboundFreezeResultV1::ExistingPendingOutbound
        }
        OutboundFreezeTransactionResultV1::NoSemanticMutation => {
            OutboundFreezeResultV1::NoSemanticMutation
        }
        OutboundFreezeTransactionResultV1::TargetChanged => OutboundFreezeResultV1::TargetChanged,
        OutboundFreezeTransactionResultV1::Blocked => OutboundFreezeResultV1::Blocked,
        OutboundFreezeTransactionResultV1::BlockedStaleEntityBases => {
            OutboundFreezeResultV1::BlockedStaleEntityBases
        }
    }
}
