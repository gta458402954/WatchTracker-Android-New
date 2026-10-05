//! Android S2 ordinary execution. Callers hold the shared physical-root lock.
use super::canonical::{ProtocolError, Result};
use super::durable_persistence::{
    OrdinaryPublishExclusiveResultV1, OutboundCompletionResultV1, SqliteS2LiteStoreV1,
};
use super::immutable_publish::{
    persist_prepared_intent_before_publish_v1, publish_persisted_intent_v1,
    RecoverPreparedIntentResultV1,
};
use super::migration_orchestration::{MigrationStateStoreV1, MigrationStatusV1};
use super::migration_runtime::BlockingWebDavRemoteV1;
use super::webdav_adapter::WebDavTransportV1;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::sync::Mutex;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum OrdinaryCycleResultV1 {
    LegacyS1Required,
    Success,
    Pending,
    Conflicts,
    ReadOnlyFrozen,
    TargetChanged,
    AuthOrCapabilityBlocked,
    AutomaticSkipped,
}

/// Scheduler observations are expectations, never publication capabilities.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MobileSyncAdmissionV1 {
    pub target_id: String,
    pub target_epoch: u64,
    pub automatic: bool,
    pub expected_attempt_at: Option<String>,
}

fn refresh_projection(
    store: &mut SqliteS2LiteStoreV1<'_>,
) -> Result<Option<OrdinaryCycleResultV1>> {
    if store.load_discovery_state()?.map_or(true, |discovery| {
        !super::discovery_persistence::publication_discovery_ready_v1(&discovery.state)
    }) {
        return Ok(Some(OrdinaryCycleResultV1::Pending));
    }
    let Some(projection) = store.load_materialized_projection()? else {
        return Ok(Some(OrdinaryCycleResultV1::Pending));
    };
    if !matches!(
        projection.state.status,
        super::materialized_projection::MaterializedProjectionStatusV1::Complete
    ) {
        return Ok(Some(OrdinaryCycleResultV1::Pending));
    }
    if projection
        .state
        .entities
        .iter()
        .any(|entity| entity.conflict)
        || !projection.state.relation_blocked_entity_keys.is_empty()
    {
        return Ok(Some(OrdinaryCycleResultV1::Conflicts));
    }
    store.update_materialized_projection_generation(projection.projection_generation)?;
    super::business_projection::apply_complete_projection_v1(
        store,
        projection.projection_generation,
    )?;
    Ok(None)
}

/// One bounded read/publish/complete/read cycle. No implicit migration is started.
/// Incomplete explicitly admitted migration is resumed with the approved executor.
pub fn run_ordinary_cycle_v1<T: WebDavTransportV1>(
    conn: &Mutex<Connection>,
    remote: &mut BlockingWebDavRemoteV1<T>,
    target_id: &str,
    target_epoch: u64,
    now: &str,
) -> Result<OrdinaryCycleResultV1> {
    let binding = super::target_root_binding::resolve_active_target_root_binding_v1(
        conn,
        target_id,
        target_epoch,
    )?
    .binding;
    let root = remote.adapter.physical_root_id().to_owned();
    if root != binding.physical_root_id {
        return Err(ProtocolError("S2_REMOTE_ROOT_MISMATCH"));
    }
    remote.discover(conn)?;
    let mut store = SqliteS2LiteStoreV1::open(conn, &root)?;
    store.refresh_from_read_authority_v1()?;
    let safety = store.load_root_safety(&root)?;
    if !safety.root_fatal_signals.is_empty() {
        return Ok(OrdinaryCycleResultV1::ReadOnlyFrozen);
    }
    let migration = store.load(&root)?;
    if migration.as_ref().is_some_and(|state| {
        !matches!(
            state.status,
            MigrationStatusV1::MigrationComplete | MigrationStatusV1::RootFrozen
        )
    }) {
        let state = super::migration_runtime::execute_migration_step_with_adapter_v1(
            conn,
            remote,
            target_id,
            target_epoch,
            now,
        )?;
        return Ok(
            if state.is_some_and(|state| state.status == MigrationStatusV1::RootFrozen) {
                OrdinaryCycleResultV1::ReadOnlyFrozen
            } else {
                OrdinaryCycleResultV1::Pending
            },
        );
    }
    if !safety.cutover_state.remote_s2_activated {
        return Ok(OrdinaryCycleResultV1::LegacyS1Required);
    }
    if migration.is_none() {
        store.admit_remote_activation_projection_v1(&binding)?;
    }
    if store.load_desktop_root_state()?.is_none() {
        store.initialize_desktop_writer_v1()?;
    }
    if !store.verify_ordinary_writer_ownership_v1()? {
        return Ok(OrdinaryCycleResultV1::ReadOnlyFrozen);
    }
    if let Some(blocked) = refresh_projection(&mut store)? {
        return Ok(blocked);
    }
    let batch = match store.load_unfinished_outbound_batch()? {
        Some(batch) => batch,
        None => match super::outbound_freeze::freeze_active_outbound_v1(
            conn,
            target_id,
            target_epoch,
            now,
        )? {
            super::outbound_freeze::OutboundFreezeResultV1::Frozen { batch, .. } => *batch,
            super::outbound_freeze::OutboundFreezeResultV1::NoSemanticMutation => {
                return Ok(
                    if super::local_authority::load_staged_descriptors(
                        &*conn
                            .lock()
                            .map_err(|_| ProtocolError("S2_RUNTIME_FAILURE"))?,
                    )?
                    .is_empty()
                    {
                        OrdinaryCycleResultV1::Success
                    } else {
                        OrdinaryCycleResultV1::Pending
                    },
                );
            }
            super::outbound_freeze::OutboundFreezeResultV1::TargetChanged => {
                return Ok(OrdinaryCycleResultV1::TargetChanged)
            }
            _ => return Ok(OrdinaryCycleResultV1::Pending),
        },
    };
    if batch.target_id != target_id || batch.target_epoch != target_epoch {
        return Ok(OrdinaryCycleResultV1::TargetChanged);
    }
    let intent = store
        .load_prepared_intent(&batch.prepared_intent_path)?
        .ok_or(ProtocolError("S2_INTENT_MISSING"))?;
    if store.load_published_receipt(&intent.remote_path)?.is_none() {
        let persisted = persist_prepared_intent_before_publish_v1(&intent, &mut store)?;
        let result = match store.run_ordinary_publish_exclusive(&root, &intent, || {
            publish_persisted_intent_v1(&persisted, remote, now)
        })? {
            OrdinaryPublishExclusiveResultV1::Executed(value) => value,
            OrdinaryPublishExclusiveResultV1::RejectedRootFrozen => {
                return Ok(OrdinaryCycleResultV1::ReadOnlyFrozen)
            }
            OrdinaryPublishExclusiveResultV1::RejectedAuthority => {
                return Ok(OrdinaryCycleResultV1::Pending)
            }
        };
        match result {
            RecoverPreparedIntentResultV1::AlreadyPublishedExact(_) => {}
            RecoverPreparedIntentResultV1::CorruptionMismatch(_) => {
                store.persist_root_fatal(&root, "REMOTE_IMMUTABLE_PATH_CONTENT_MISMATCH")?;
                return Ok(OrdinaryCycleResultV1::ReadOnlyFrozen);
            }
            RecoverPreparedIntentResultV1::AuthOrCapabilityFailure => {
                return Ok(OrdinaryCycleResultV1::AuthOrCapabilityBlocked)
            }
            _ => return Ok(OrdinaryCycleResultV1::Pending),
        }
        match store.verify_and_persist_commit_receipt(&intent, remote, now)? {
            RecoverPreparedIntentResultV1::AlreadyPublishedExact(_) => {}
            RecoverPreparedIntentResultV1::CorruptionMismatch(_) => {
                store.persist_root_fatal(&root, "REMOTE_IMMUTABLE_PATH_CONTENT_MISMATCH")?;
                return Ok(OrdinaryCycleResultV1::ReadOnlyFrozen);
            }
            RecoverPreparedIntentResultV1::AuthOrCapabilityFailure => {
                return Ok(OrdinaryCycleResultV1::AuthOrCapabilityBlocked)
            }
            _ => return Ok(OrdinaryCycleResultV1::Pending),
        }
    }
    if store.complete_verified_outbound_batch()? == OutboundCompletionResultV1::PendingReceipt {
        return Ok(OrdinaryCycleResultV1::Pending);
    }
    remote.discover(conn)?;
    store.refresh_from_read_authority_v1()?;
    if !store.load_root_safety(&root)?.root_fatal_signals.is_empty() {
        return Ok(OrdinaryCycleResultV1::ReadOnlyFrozen);
    }
    if let Some(blocked) = refresh_projection(&mut store)? {
        return Ok(blocked);
    }
    let guard = conn
        .lock()
        .map_err(|_| ProtocolError("S2_RUNTIME_FAILURE"))?;
    Ok(
        if super::local_authority::load_staged_descriptors(&guard)?.is_empty() {
            OrdinaryCycleResultV1::Success
        } else {
            OrdinaryCycleResultV1::Pending
        },
    )
}

/// All mobile events and manual runs share the I6.4 root coordinator. Durable
/// admission is rechecked after waiting; a stale timer cannot bypass backoff.
pub fn run_mobile_sync_with_adapter_v1<T: WebDavTransportV1>(
    conn: &Mutex<Connection>,
    coordinator: &super::root_coordinator::RootExecutionCoordinatorV1,
    remote: &mut BlockingWebDavRemoteV1<T>,
    admission: &MobileSyncAdmissionV1,
    now: &str,
) -> Result<OrdinaryCycleResultV1> {
    let target_id = admission.target_id.as_str();
    let target_epoch = admission.target_epoch;
    let automatic = admission.automatic;
    let expected_attempt_at = admission.expected_attempt_at.as_deref();
    let root = remote.adapter.physical_root_id().to_owned();
    let _root_lock = coordinator.acquire_blocking(&root)?;
    let binding = super::target_root_binding::resolve_active_target_root_binding_v1(
        conn,
        target_id,
        target_epoch,
    )?
    .binding;
    if binding.physical_root_id != root {
        return Err(ProtocolError("S2_REMOTE_ROOT_MISMATCH"));
    }
    if automatic {
        let mut guard = conn
            .lock()
            .map_err(|_| ProtocolError("S2_RUNTIME_FAILURE"))?;
        if !crate::sync_state::admit_mobile_automatic_v1(
            &mut guard,
            target_id,
            target_epoch,
            expected_attempt_at,
        )
        .map_err(|_| ProtocolError("S2_SCHEDULER_FAILURE"))?
        {
            return Ok(OrdinaryCycleResultV1::AutomaticSkipped);
        }
    }
    let result = run_ordinary_cycle_v1(conn, remote, target_id, target_epoch, now);
    let mut guard = conn
        .lock()
        .map_err(|_| ProtocolError("S2_RUNTIME_FAILURE"))?;
    let classified = crate::sync_state::record_mobile_s2_result_v1(
        &mut guard,
        &binding,
        result
            .as_ref()
            .copied()
            .unwrap_or(OrdinaryCycleResultV1::Pending),
    )
    .map_err(|_| ProtocolError("S2_SCHEDULER_FAILURE"))?;
    result.map(|_| classified)
}
