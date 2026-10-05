//! Explicit Android migration entry points; no mobile scheduler or ordinary writer.
use super::canonical::{ProtocolError, Result};
use super::durable_persistence::SqliteS2LiteStoreV1;
use super::immutable_publish::*;
use super::migration_orchestration::*;
use super::webdav_adapter::*;
use rusqlite::Connection;
use std::sync::Mutex;

/// Synchronous frozen executor bridge. Invoke only from a blocking worker.
pub struct BlockingWebDavRemoteV1<T: WebDavTransportV1> {
    pub adapter: WebDavS2AdapterV1<T>,
    runtime: tokio::runtime::Runtime,
    identity: u64,
}
impl<T: WebDavTransportV1> BlockingWebDavRemoteV1<T> {
    pub fn new(adapter: WebDavS2AdapterV1<T>) -> Result<Self> {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Ok(Self {
            adapter,
            runtime: tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|_| ProtocolError("S2_RUNTIME_FAILURE"))?,
            identity: NEXT.fetch_add(1, Ordering::Relaxed),
        })
    }
    pub fn discover(&mut self, conn: &Mutex<Connection>) -> Result<()> {
        let mut guard = conn
            .lock()
            .map_err(|_| ProtocolError("S2_RUNTIME_FAILURE"))?;
        self.runtime
            .block_on(super::discovery_runtime::discover_read_only_v1(
                &mut guard,
                &mut self.adapter,
                &super::remote_discovery::DiscoveryBudgetsV1::default(),
            ))?;
        Ok(())
    }
}
impl<T: WebDavTransportV1> ImmutableObjectRemoteV1 for BlockingWebDavRemoteV1<T> {
    fn physical_root_id(&self) -> Option<&str> {
        Some(self.adapter.physical_root_id())
    }
    fn execution_context_identity(&self) -> u64 {
        self.identity
    }
    fn get_exact(&mut self, path: &str) -> RemoteExactGetResultV1 {
        match self.runtime.block_on(self.adapter.get_exact(path)) {
            WebDavGetResultV1::DefinitelyPresent(bytes) => {
                RemoteExactGetResultV1::DefinitelyPresent(bytes)
            }
            WebDavGetResultV1::DefinitelyAbsent => RemoteExactGetResultV1::DefinitelyAbsent,
            WebDavGetResultV1::Indeterminate => RemoteExactGetResultV1::Indeterminate,
            WebDavGetResultV1::AuthOrCapabilityFailure => {
                RemoteExactGetResultV1::AuthOrCapabilityFailure
            }
        }
    }
    fn put_exact(
        &mut self,
        path: &str,
        bytes: &[u8],
        if_none_match_star: bool,
    ) -> RemotePutResultV1 {
        if !if_none_match_star {
            return RemotePutResultV1::Indeterminate;
        }
        match self.runtime.block_on(self.adapter.put_immutable(
            path,
            bytes,
            &super::canonical::sha256_hex(bytes),
        )) {
            ImmutablePutResultV1::Published | ImmutablePutResultV1::AlreadyPresentExact => {
                RemotePutResultV1::Success
            }
            ImmutablePutResultV1::AuthOrCapabilityFailure => {
                RemotePutResultV1::AuthOrCapabilityFailure
            }
            _ => RemotePutResultV1::Indeterminate,
        }
    }
}

/// One restart-safe frozen step. The root execution coordinator must be held
/// by the production caller across discovery, admission and the entire step.
pub fn execute_migration_step_with_adapter_v1<T: WebDavTransportV1>(
    conn: &Mutex<Connection>,
    remote: &mut BlockingWebDavRemoteV1<T>,
    target_id: &str,
    target_epoch: u64,
    now: &str,
) -> Result<Option<MigrationStateV1>> {
    let root = remote.adapter.physical_root_id().to_owned();
    let binding = super::target_root_binding::resolve_active_target_root_binding_v1(
        conn,
        target_id,
        target_epoch,
    )?
    .binding;
    if root != binding.physical_root_id {
        return Err(ProtocolError("MIGRATION_ROOT_BINDING_MISMATCH"));
    }
    remote.discover(conn)?;
    let mut store = SqliteS2LiteStoreV1::open(conn, &root)?;
    store.refresh_from_read_authority_v1()?;
    let historical = MigrationStateStoreV1::load(&mut store, &root)?;
    if historical
        .as_ref()
        .is_some_and(|state| state.status == MigrationStatusV1::MigrationComplete)
    {
        apply_projection(&mut store)?;
        return Ok(historical);
    }
    if historical.is_none()
        && store
            .load_root_safety(&root)?
            .cutover_state
            .remote_s2_activated
    {
        // A root already activated elsewhere has no local bootstrap to publish.
        // Freeze compatibility evidence before applying any mutable business row.
        store.admit_remote_activation_projection_v1(&binding)?;
        store.initialize_desktop_writer_v1()?;
        apply_projection(&mut store)?;
        return Ok(None); // Already activated elsewhere; no local migration identity.
    }
    let (id, writer, created) = if let Some(prior) = &historical {
        (
            prior.migration_id.clone(),
            prior.writer_id.clone(),
            prior.created_at.clone(),
        )
    } else {
        let guard = conn
            .lock()
            .map_err(|_| ProtocolError("S2_RUNTIME_FAILURE"))?;
        (
            uuid::Uuid::new_v4().to_string(),
            super::local_authority::initialize_writer(&guard)?.writer_id,
            now.to_owned(),
        )
    };
    let admitted = super::migration_admission::admit_and_capture_migration_v1(
        conn,
        target_id,
        target_epoch,
        &id,
        &writer,
        &created,
    )?;
    let current = admitted.state;
    if current.status == MigrationStatusV1::RootFrozen
        || current.status == MigrationStatusV1::MigrationComplete
    {
        return Ok(Some(current));
    }
    if store
        .load_root_safety(&root)?
        .cutover_state
        .remote_s2_activated
        && matches!(
            current.status,
            MigrationStatusV1::StageBComplete
                | MigrationStatusV1::ActivationPublishing
                | MigrationStatusV1::ActivationVerified
        )
    {
        let _ = store.adopt_verified_activation_v1()?;
    }
    let current = MigrationStateStoreV1::load(&mut store, &root)?
        .ok_or(ProtocolError("S2_MIGRATION_MISSING"))?;
    if current.status == MigrationStatusV1::ActivationVerified {
        store.recover_durable_verified_activation_cutover_v1()?;
        let _ = store.finalize_verified_migration_v1()?;
    } else {
        let attachment = start_or_attach_migration_v1(&current, &mut store)?;
        let capability = if current.status == MigrationStatusV1::ActivationPublishing {
            create_activation_publication_execution_capability_v1(
                &attachment,
                remote,
                &store,
                super::durable_persistence::migration_execution_identity_v1(
                    &admitted.execution_binding,
                ),
            )?
        } else {
            create_migration_root_execution_capability_v1(&attachment, remote, &store)?
        };
        let mut intents = store;
        let mut receipts = store;
        let mut activation_intents = store;
        let mut activation_receipts = store;
        execute_migration_step_v1(
            &current,
            &capability,
            remote,
            &mut store,
            &mut intents,
            &mut receipts,
            &mut activation_intents,
            &mut activation_receipts,
            now,
        )?;
    }
    let result = MigrationStateStoreV1::load(&mut store, &root)?
        .ok_or(ProtocolError("S2_MIGRATION_MISSING"))?;
    if result.status == MigrationStatusV1::MigrationComplete {
        store.refresh_from_read_authority_v1()?;
        apply_projection(&mut store)?;
    }
    Ok(Some(result))
}

fn apply_projection(store: &mut SqliteS2LiteStoreV1<'_>) -> Result<()> {
    if let Some(projection) = store.load_materialized_projection()? {
        store.update_materialized_projection_generation(projection.projection_generation)?;
        super::business_projection::apply_complete_projection_v1(
            store,
            projection.projection_generation,
        )?;
    }
    Ok(())
}

/// Final legacy mutation gate. A validated observation is reconciled durably
/// before a target-bound ticket is checked again under SQLite write admission.
pub fn run_legacy_put_with_adapter_v1<T: WebDavTransportV1, V, F: FnOnce() -> Result<V>>(
    conn: &Mutex<Connection>,
    remote: &mut BlockingWebDavRemoteV1<T>,
    target_id: &str,
    target_epoch: u64,
    operation: F,
) -> Result<super::durable_persistence::LegacyS1PublishAdmissionV1<V>> {
    let binding = super::target_root_binding::resolve_active_target_root_binding_v1(
        conn,
        target_id,
        target_epoch,
    )?
    .binding;
    if remote.adapter.physical_root_id() != binding.physical_root_id {
        return Err(ProtocolError("MIGRATION_ROOT_BINDING_MISMATCH"));
    }
    remote.discover(conn)?;
    let mut store = SqliteS2LiteStoreV1::open(conn, &binding.physical_root_id)?;
    store.refresh_from_read_authority_v1()?;
    let ticket = super::durable_persistence::capture_legacy_route_ticket_v1(conn, &binding)?;
    store.run_legacy_bound_put_v1(&ticket, operation)
}
