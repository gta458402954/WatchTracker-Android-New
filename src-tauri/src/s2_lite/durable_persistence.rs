//! Production SQLite persistence for the frozen S2 Lite v1 core.
//!
//! Protocol structs remain the in-memory source of truth. SQLite JSON BLOBs use
//! a persistence-specific, versioned envelope and a strict canonical payload:
//! missing default-sensitive fields and unknown fields at any nesting depth are
//! rejected before the value is admitted to the frozen core. Generation columns
//! are independently bound as canonical decimal TEXT for atomic comparisons.
//! Prepared object bytes are removed from metadata and stored in dedicated BLOB
//! columns without parsing or rewriting.

use std::sync::{Mutex, MutexGuard};

use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::Value;

use super::activation_cutover::{
    decide_legacy_put_v1, recover_activation_cutover_v1, ActivationCutoverStateV1,
    ActivationFingerprintConsistencyV1, LegacyPutDecisionV1,
};
use super::canonical::{
    jcs_bytes, sha256_hex, validate_canonical_uuid_v4, validate_commit_ref, ProtocolError, Result,
};
use super::causal::decode_frozen_wire_commit_v1;
use super::immutable_publish::{
    restart_durable_activation_publish_v1, restart_durable_publish_v1,
    validate_prepared_activation_intent_v1, validate_prepared_intent_v1,
    validate_published_activation_receipt_v1, validate_published_receipt_v1,
    ImmutableObjectRemoteV1, PreparedActivationIntentStoreV1, PreparedActivationIntentV1,
    PreparedIntentStoreV1, PreparedIntentV1, PublishedActivationReceiptStoreV1,
    PublishedActivationReceiptV1, PublishedReceiptStoreV1, RecoverActivationIntentResultV1,
    RecoverPreparedIntentResultV1, RemotePublishedReceiptV1,
};
use super::materialized_projection::{
    MaterializedProjectionEntityV1, MaterializedProjectionStateV1,
};
use super::migration_orchestration::{
    create_migration_root_safety_state_v1, create_migration_state_v1,
    merge_migration_root_cutover_state_v1, plan_captured_migration_v1,
    reconcile_migration_state_v1, retain_captured_snapshot_v1, validate_attempt_transition,
    ActivationCutoverStateStoreV1, CapturedLegacySnapshotV1, MigrationExecutionIdentityV1,
    MigrationRootFatalV1, MigrationRootSafetyStateV1, MigrationStateStoreV1, MigrationStateV1,
    MigrationStatusV1, PublishExclusiveResultV1,
};
use super::remote_discovery::{
    create_discovery_state_v1, DiscoveryStateV1, VerifiedFingerprintEvidenceV1,
    VerifiedRemoteObjectV1,
};
use super::types::CommitRef;

const STORE_FAILURE: ProtocolError = ProtocolError("S2_DURABLE_PERSISTENCE_FAILURE");
const STORE_CORRUPTION: ProtocolError = ProtocolError("S2_DURABLE_STATE_CORRUPTION");
const ACTIVATION_EXECUTION_IDENTITY_MISMATCH: ProtocolError =
    ProtocolError("S2_ACTIVATION_EXECUTION_IDENTITY_MISMATCH");
const ROOT_MISMATCH: ProtocolError = ProtocolError("MIGRATION_ROOT_BINDING_MISMATCH");
const SCHEMA_VERSION: &str = "1";
const PERSISTENCE_FORMAT_VERSION: u8 = 1;

/// Result of an ordinary root-bound publication admission. A transport result
/// is intentionally not a receipt and carries no publication authority.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum OrdinaryPublishExclusiveResultV1<T> {
    Executed(T),
    RejectedRootFrozen,
}

/// Admission outcome for the fixed normal-S2 discovery infrastructure
/// mutation. It carries no object-publication authority; it only gates the
/// idempotent preparation of the canonical top-level writers collection.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum NormalDiscoveryInfrastructureAdmissionV1<T> {
    Executed(T),
    RejectedRootFrozen,
    AutomaticSkipped,
}

/// The caller context that must be revalidated at the final durable boundary
/// before writers/ can be created. Manual work deliberately has no scheduler
/// pause restriction; automatic work must remain tied to its exact runnable
/// target observation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum NormalDiscoveryInfrastructureCallerAdmissionV1 {
    Manual,
    Automatic {
        expected_target_id: String,
        expected_target_epoch: u64,
    },
}

/// The final transaction-local result of admitting a normal S2 lifecycle
/// route.  A preliminary router result is deliberately not authority to
/// create a writer until this admission has re-read root state.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum NormalS2RouteAdmissionV1 {
    EnterNormalS2(DesktopRootStateV1),
    ReadOnlyFrozen,
}

/// The final, read-only authority decision used immediately before normal-S2
/// success bookkeeping.  It deliberately performs no writer initialization:
/// a completed remote cycle may only clear scheduler failure state when the
/// already-established root authority still admits normal S2.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NormalS2CompletionAuthorityV1 {
    Valid,
    TargetChanged,
    ReadOnlyFrozen,
    NoLongerNormalS2,
}

/// Outcome of the local-only, root-authoritative migration cutover
/// finalization. This API has no remote collaborator and never publishes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MigrationFinalizationResultV1 {
    Finalized,
    AlreadyFinalized,
    ReadOnlyFrozen,
}

/// Result of adopting already-verified compatible remote activation evidence.
/// This transition never publishes, receipts, completes, or retires source
/// authority; those remain the finalizer's separate responsibility.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VerifiedActivationAdoptionResultV1 {
    Adopted,
    AlreadyVerified,
    AlreadyComplete,
    ReadOnlyFrozen,
}

/// Test-only fault locations inside the single migration-finalization
/// transaction.  Kept transaction-local so the production path cannot expose
/// a partial cutover state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum MigrationFinalizationFaultV1 {
    MigrationCompletePersisted,
    WriterHandoffPersisted,
    SourceRetired,
}

/// Admission outcome for the legacy S1 writer. A process lock can supplement
/// this path, but only this SQLite transaction decides whether its one PUT is
/// admitted across independent desktop processes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LegacyS1PublishAdmissionV1<T> {
    Executed(T),
    RejectedRootFrozen,
    RejectedActivation,
    RejectedMigrationSourceProtected,
}

/// A one-cycle, non-authoritative handoff from the Rust lifecycle router to the
/// legacy S1 caller.  It intentionally contains only durable identities that
/// must be re-read under the later S1 write transaction.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LegacyRouteTicketV1 {
    pub target_id: String,
    pub target_epoch: u64,
    pub physical_root_id: String,
    pub root_safety_generation: u64,
}

/// A ticket is never authority on its own.  This result describes what a fresh
/// SQLite validation observed, so the coordinator can preserve its public
/// result taxonomy without treating a stale ticket as permission to mutate.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LegacyRouteTicketValidationV1 {
    Valid,
    TargetChanged,
    ReadOnlyFrozen,
    NoLongerLegacy,
}

/// Transaction-local migration admission outcome for a completed legacy route.
/// A non-valid ticket is a legal concurrent authority advance, not corruption.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LegacyTicketMigrationAdmissionV1 {
    Admitted(Box<MigrationAdmissionResultV1>),
    TicketInvalid(LegacyRouteTicketValidationV1),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LegacyS1PublicationRejectionV1 {
    RootFrozen,
    Activation,
    MigrationSourceProtected,
}

/// Immutable input needed to admit the first local bootstrap capture. The
/// writer is the future migration writer, never an ordinary desktop writer.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MigrationAdmissionInputV1 {
    pub target_binding: TargetRootBindingV1,
    pub migration_id: String,
    pub migration_writer_id: String,
    pub created_at: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MigrationAdmissionResultV1 {
    pub execution_binding: MigrationExecutionBindingV1,
    pub state: MigrationStateV1,
    pub attached_existing: bool,
}

/// Local-only desktop bookkeeping. Frozen root safety, discovery, activation,
/// and receipts intentionally live in their established authoritative tables.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DesktopRootStateV1 {
    pub state_version: u8,
    pub physical_root_id: String,
    pub local_writer_id: String,
    pub next_writer_sequence: u64,
    pub writer_head: Option<CommitRef>,
    pub lifecycle_generation: u64,
    pub materialized_projection_generation: Option<u64>,
    pub business_applied_projection_generation: Option<u64>,
}

/// Immutable local record binding one authoritative SyncTarget epoch to the
/// frozen WebDAV physical-root identity. It is deliberately independent from
/// lifecycle and publication state so old epochs remain recoverable.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TargetRootBindingV1 {
    pub binding_version: u8,
    pub target_id: String,
    pub target_epoch: u64,
    pub canonical_url: String,
    pub normalized_account: String,
    pub physical_root_id: String,
}

/// Immutable local execution identity for one pending or future migration.
/// Credentials deliberately remain in target authority and never enter this
/// root-scoped durable row.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MigrationExecutionBindingV1 {
    pub binding_version: u8,
    pub physical_root_id: String,
    pub target_id: String,
    pub target_epoch: u64,
    pub captured_records_generation: i64,
    pub legacy_fingerprint: Option<String>,
    pub migration_id: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OutboundBatchMutationV1 {
    pub entity_kind: String,
    pub entity_id: String,
    pub entity_key: Value,
    pub captured_last_generation: i64,
    pub local_mutation_id: String,
}

/// Local recovery binding for one ordinary commit. It references the durable
/// prepared intent and authoritative receipt; it never copies receipt facts.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OutboundBatchV1 {
    pub state_version: u8,
    pub batch_id: String,
    pub target_id: String,
    pub target_epoch: u64,
    pub physical_root_id: String,
    pub projection_generation: u64,
    pub source_discovery_generation: u64,
    pub source_root_safety_generation: u64,
    pub captured_local_generation: i64,
    pub mutations: Vec<OutboundBatchMutationV1>,
    pub basis_clock: Vec<CommitRef>,
    pub writer_id: String,
    pub writer_sequence: u64,
    pub previous_writer_ref: Option<CommitRef>,
    pub commit_ref: CommitRef,
    pub prepared_intent_path: String,
    pub prepared_intent_fingerprint: String,
    pub state: String,
    pub bookkeeping_completed: bool,
    pub bookkeeping_generation: u64,
}

/// Rebuildable cache binding frozen replay output to the exact durable inputs
/// used to derive it. It is never receipt, safety, or discovery authority.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DurableMaterializedProjectionV1 {
    pub projection_version: u8,
    pub physical_root_id: String,
    pub projection_generation: u64,
    pub source_discovery_generation: u64,
    pub source_root_safety_generation: u64,
    pub replay_input_fingerprint: String,
    pub business_projection_applied_generation: Option<u64>,
    pub state: MaterializedProjectionStateV1,
}

/// Local provenance proving that a projection pass deliberately left one
/// entity's business row untouched because its target-scoped staging overlay
/// was present.  It is not protocol, receipt, or publication authority.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EntityProjectionOverlayBlockerV1 {
    pub state_version: u8,
    pub physical_root_id: String,
    pub target_id: String,
    pub entity_key: Value,
    pub projection_generation: u64,
    pub projection_entity_fingerprint: String,
}

/// The authority available to a local mutation that is about to capture an
/// immutable S2 entity anchor.  A materialized projection is not sufficient
/// by itself: the business rows being edited must have been completely
/// projected from that exact generation.
pub(crate) enum StagingAnchorProjectionAdmissionV1 {
    Ready(Box<DurableMaterializedProjectionV1>),
    Unavailable(&'static str),
}

/// Local bookkeeping result for one complete overlay-aware business projection.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BusinessProjectionTransactionResultV1 {
    Applied,
    AlreadyApplied,
}

/// Result of applying local bookkeeping for an exactly receipted outbound
/// batch. A missing receipt deliberately has no acknowledgement authority.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OutboundCompletionResultV1 {
    Completed,
    AlreadyCompleted,
    PendingReceipt,
}

/// Inputs captured under the outbound freezer's one authoritative SQLite
/// transaction. Nothing in this value is a transport fact.
#[derive(Clone, Debug)]
#[allow(dead_code)] // Frozen ordinary publication authority; execution deferred to I6.5.
pub(crate) struct OutboundFreezeTransactionContextV1 {
    pub binding: TargetRootBindingV1,
    pub root_state: DesktopRootStateV1,
    pub projection: DurableMaterializedProjectionV1,
    pub discovery_generation: u64,
    pub root_safety_generation: u64,
    pub staging: crate::sync_staging::SyncStaging,
}

#[allow(dead_code)] // Frozen ordinary publication authority; execution deferred to I6.5.
pub(crate) enum OutboundFreezeTransactionPlanV1 {
    NoSemanticMutation,
    Blocked,
    BlockedStaleEntityBases,
    Frozen {
        batch: Box<OutboundBatchV1>,
        intent: Box<PreparedIntentV1>,
    },
}

#[allow(dead_code)] // Frozen ordinary publication authority; execution deferred to I6.5.
pub(crate) enum OutboundFreezeTransactionResultV1 {
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

/// Test-only failure points for the one durable incompatible-activation
/// authority transition.  Keeping the failure points inside the transaction
/// makes the old two-commit crash window directly regression-testable.
#[cfg(test)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum IncompatibleActivationFreezeFaultV1 {
    AfterCutoverMerge,
    AfterRootFatalWrite,
    BeforeCommit,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RemoteActivationAdoptionV1 {
    state_version: u8,
    physical_root_id: String,
    captured_records_generation: i64,
    legacy_fingerprint: Option<String>,
    has_legacy_baseline: bool,
    snapshot: CapturedLegacySnapshotV1,
}
fn validate_remote_activation_adoption_v1(
    state: &RemoteActivationAdoptionV1,
    root: &str,
) -> Result<()> {
    if state.state_version != 1
        || state.physical_root_id != root
        || state.captured_records_generation < 0
        || state.snapshot.snapshot_version != 1
    {
        return Err(STORE_CORRUPTION);
    }
    let plan = super::bootstrap::build_bootstrap_plan_v1(&state.snapshot.canonical_entities)?;
    let entities = plan
        .stage_a_ordered_mutations
        .into_iter()
        .chain(plan.stage_b_ordered_mutations)
        .collect::<Vec<_>>();
    let fingerprint = sha256_hex(&jcs_bytes(
        &serde_json::json!({"domain":"watchtracker-s2-lite-legacy-snapshot-v1","canonicalEntities":entities}),
    )?);
    let expected = if state.has_legacy_baseline || !state.snapshot.canonical_entities.is_empty() {
        Some(fingerprint.clone())
    } else {
        None
    };
    if fingerprint != state.snapshot.legacy_fingerprint || state.legacy_fingerprint != expected {
        return Err(STORE_CORRUPTION);
    }
    Ok(())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PersistenceEnvelopeRefV1<'a, T> {
    persistence_version: u8,
    payload: &'a T,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PersistenceEnvelopeValueV1 {
    persistence_version: u8,
    payload: Value,
}

/// Private local proof created only after an exact GET against one physical
/// remote root. Raw wire/durable receipts have no conversion into this type.
#[derive(Clone, Debug, Eq, PartialEq)]
struct RootBoundPublishedReceiptV1 {
    physical_root_id: String,
    receipt: RemotePublishedReceiptV1,
}

/// Local proof that an activation receipt came from verification against one
/// physical remote root.
#[derive(Clone, Debug, Eq, PartialEq)]
struct RootBoundPublishedActivationReceiptV1 {
    physical_root_id: String,
    receipt: PublishedActivationReceiptV1,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PersistedPublishedReceiptV1<T> {
    physical_root_id: String,
    receipt: T,
}

pub(crate) fn migrate_schema(conn: &Connection) -> rusqlite::Result<()> {
    let existing = conn
        .query_row(
            "SELECT value FROM settings WHERE key='s2_lite_persistence_schema_version'",
            [],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    if existing
        .as_deref()
        .is_some_and(|value| value != SCHEMA_VERSION)
    {
        return Err(rusqlite::Error::InvalidQuery);
    }
    let transaction = conn.unchecked_transaction()?;
    transaction.execute_batch(
        "CREATE TABLE IF NOT EXISTS s2_lite_root_authority_v1 (
            root_id TEXT PRIMARY KEY NOT NULL CHECK(length(root_id) > 0),
            authority_generation TEXT NOT NULL,
            state_json BLOB NOT NULL CHECK(typeof(state_json) = 'blob')
         );
         CREATE TABLE IF NOT EXISTS s2_lite_remote_activation_adoption_v1 (
            root_id TEXT PRIMARY KEY NOT NULL,
            state_json BLOB NOT NULL CHECK(typeof(state_json) = 'blob'),
            FOREIGN KEY(root_id) REFERENCES s2_lite_root_authority_v1(root_id) ON DELETE RESTRICT
         );
         CREATE TABLE IF NOT EXISTS s2_lite_migration_v1 (
            root_id TEXT PRIMARY KEY NOT NULL,
            migration_id TEXT NOT NULL,
            migration_generation TEXT NOT NULL,
            state_json BLOB NOT NULL CHECK(typeof(state_json) = 'blob'),
            FOREIGN KEY(root_id) REFERENCES s2_lite_root_authority_v1(root_id) ON DELETE RESTRICT
         );
         CREATE TABLE IF NOT EXISTS s2_lite_migration_discovery_v1 (
            root_id TEXT PRIMARY KEY NOT NULL,
            storage_generation TEXT NOT NULL,
            state_json BLOB NOT NULL CHECK(typeof(state_json) = 'blob'),
            FOREIGN KEY(root_id) REFERENCES s2_lite_root_authority_v1(root_id) ON DELETE RESTRICT
         );
         CREATE TABLE IF NOT EXISTS s2_lite_prepared_intent_v1 (
            root_id TEXT NOT NULL,
            intent_kind TEXT NOT NULL CHECK(intent_kind IN ('commit', 'activation')),
            remote_path TEXT NOT NULL,
            intent_fingerprint TEXT NOT NULL,
            metadata_json BLOB NOT NULL CHECK(typeof(metadata_json) = 'blob'),
            exact_bytes BLOB NOT NULL CHECK(typeof(exact_bytes) = 'blob'),
            PRIMARY KEY(root_id, intent_kind, remote_path),
            FOREIGN KEY(root_id) REFERENCES s2_lite_root_authority_v1(root_id) ON DELETE RESTRICT
         );
         CREATE TABLE IF NOT EXISTS s2_lite_published_receipt_v1 (
            root_id TEXT NOT NULL,
            receipt_kind TEXT NOT NULL CHECK(receipt_kind IN ('commit', 'activation')),
            remote_path TEXT NOT NULL,
            receipt_json BLOB NOT NULL CHECK(typeof(receipt_json) = 'blob'),
            PRIMARY KEY(root_id, receipt_kind, remote_path),
            FOREIGN KEY(root_id, receipt_kind, remote_path)
              REFERENCES s2_lite_prepared_intent_v1(root_id, intent_kind, remote_path)
              ON DELETE RESTRICT
         );
         CREATE TABLE IF NOT EXISTS s2_lite_desktop_root_state_v1 (
            root_id TEXT PRIMARY KEY NOT NULL,
            state_json BLOB NOT NULL CHECK(typeof(state_json) = 'blob'),
            FOREIGN KEY(root_id) REFERENCES s2_lite_root_authority_v1(root_id) ON DELETE RESTRICT
         );
         CREATE TABLE IF NOT EXISTS s2_lite_outbound_batch_v1 (
            root_id TEXT PRIMARY KEY NOT NULL,
            batch_id TEXT NOT NULL UNIQUE,
            intent_path TEXT NOT NULL,
            intent_fingerprint TEXT NOT NULL,
            state_json BLOB NOT NULL CHECK(typeof(state_json) = 'blob'),
            FOREIGN KEY(root_id) REFERENCES s2_lite_root_authority_v1(root_id) ON DELETE RESTRICT
         );
         CREATE TABLE IF NOT EXISTS s2_lite_materialized_projection_v1 (
            root_id TEXT PRIMARY KEY NOT NULL,
            projection_generation TEXT NOT NULL,
            state_json BLOB NOT NULL CHECK(typeof(state_json) = 'blob'),
            FOREIGN KEY(root_id) REFERENCES s2_lite_root_authority_v1(root_id) ON DELETE RESTRICT
         );
         CREATE TABLE IF NOT EXISTS s2_lite_target_root_binding_v1 (
            binding_version INTEGER NOT NULL CHECK(binding_version = 1),
            target_id TEXT NOT NULL,
            target_epoch TEXT NOT NULL,
            canonical_url TEXT NOT NULL,
            normalized_account TEXT NOT NULL,
            physical_root_id TEXT NOT NULL,
            PRIMARY KEY(target_id, target_epoch),
            FOREIGN KEY(physical_root_id) REFERENCES s2_lite_root_authority_v1(root_id) ON DELETE RESTRICT
         );
         CREATE TABLE IF NOT EXISTS s2_lite_migration_execution_binding_v1 (
            root_id TEXT PRIMARY KEY NOT NULL,
            state_json BLOB NOT NULL CHECK(typeof(state_json) = 'blob'),
            FOREIGN KEY(root_id) REFERENCES s2_lite_root_authority_v1(root_id) ON DELETE RESTRICT
         );
         CREATE TABLE IF NOT EXISTS s2_lite_migration_source_guard_v1 (
            root_id TEXT PRIMARY KEY NOT NULL,
            migration_id TEXT NOT NULL,
            captured_records_generation TEXT NOT NULL,
            FOREIGN KEY(root_id) REFERENCES s2_lite_root_authority_v1(root_id) ON DELETE RESTRICT
         );
         CREATE TABLE IF NOT EXISTS s2_lite_migration_source_owner_v1 (
            owner_key INTEGER PRIMARY KEY NOT NULL CHECK(owner_key = 1),
            root_id TEXT NOT NULL UNIQUE,
            migration_id TEXT NOT NULL,
            FOREIGN KEY(root_id) REFERENCES s2_lite_migration_source_guard_v1(root_id) ON DELETE RESTRICT
         );
         CREATE TABLE IF NOT EXISTS s2_lite_entity_projection_overlay_blocker_v1 (
            root_id TEXT NOT NULL,
            target_id TEXT NOT NULL,
            entity_key_jcs BLOB NOT NULL CHECK(typeof(entity_key_jcs) = 'blob'),
            state_json BLOB NOT NULL CHECK(typeof(state_json) = 'blob'),
            PRIMARY KEY(root_id, target_id, entity_key_jcs),
            FOREIGN KEY(root_id) REFERENCES s2_lite_root_authority_v1(root_id) ON DELETE RESTRICT
         );
         INSERT INTO settings(key, value) VALUES('s2_lite_persistence_schema_version', '1')
           ON CONFLICT(key) DO NOTHING;",
    )?;
    transaction.commit()?;
    // Upgrade source protection in its own committed unit before inspecting
    // authority.  A cold-start corruption error must never roll this upgrade
    // back and reopen the legacy business source through an old guard-only
    // trigger set.
    install_migration_source_guard_triggers(conn)?;
    // The owner and guard are one source-authority fact.  In particular, do
    // not "repair" a partial legacy row here: that could turn corruption into
    // an active migration or, worse, silently change the protected source.
    load_migration_source_owner(conn).map_err(|_| rusqlite::Error::InvalidQuery)?;
    Ok(())
}

fn install_migration_source_guard_triggers(conn: &Connection) -> rusqlite::Result<()> {
    let tables = [
        "records",
        "episode_completions",
        "collections",
        "collection_members",
    ];
    if tables.iter().any(|table| {
        conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)",
            [table],
            |row| row.get::<_, i64>(0),
        )
        .map_or(true, |exists| exists == 0)
    }) {
        return Ok(());
    }
    conn.execute_batch(
        "DROP TRIGGER IF EXISTS s2_lite_guard_records_insert_v1;
         DROP TRIGGER IF EXISTS s2_lite_guard_records_update_v1;
         DROP TRIGGER IF EXISTS s2_lite_guard_records_delete_v1;
         DROP TRIGGER IF EXISTS s2_lite_guard_episode_insert_v1;
         DROP TRIGGER IF EXISTS s2_lite_guard_episode_update_v1;
         DROP TRIGGER IF EXISTS s2_lite_guard_episode_delete_v1;
         DROP TRIGGER IF EXISTS s2_lite_guard_collections_insert_v1;
         DROP TRIGGER IF EXISTS s2_lite_guard_collections_update_v1;
         DROP TRIGGER IF EXISTS s2_lite_guard_collections_delete_v1;
         DROP TRIGGER IF EXISTS s2_lite_guard_members_insert_v1;
         DROP TRIGGER IF EXISTS s2_lite_guard_members_update_v1;
         DROP TRIGGER IF EXISTS s2_lite_guard_members_delete_v1;
         CREATE TRIGGER s2_lite_guard_records_insert_v1 BEFORE INSERT ON records WHEN (SELECT COUNT(*) FROM s2_lite_migration_source_owner_v1) != 0 OR (SELECT COUNT(*) FROM s2_lite_migration_source_guard_v1) != 0 BEGIN SELECT RAISE(ABORT, 'S2_MIGRATION_SOURCE_PROTECTED'); END;
         CREATE TRIGGER s2_lite_guard_records_update_v1 BEFORE UPDATE ON records WHEN (SELECT COUNT(*) FROM s2_lite_migration_source_owner_v1) != 0 OR (SELECT COUNT(*) FROM s2_lite_migration_source_guard_v1) != 0 BEGIN SELECT RAISE(ABORT, 'S2_MIGRATION_SOURCE_PROTECTED'); END;
         CREATE TRIGGER s2_lite_guard_records_delete_v1 BEFORE DELETE ON records WHEN (SELECT COUNT(*) FROM s2_lite_migration_source_owner_v1) != 0 OR (SELECT COUNT(*) FROM s2_lite_migration_source_guard_v1) != 0 BEGIN SELECT RAISE(ABORT, 'S2_MIGRATION_SOURCE_PROTECTED'); END;
         CREATE TRIGGER s2_lite_guard_episode_insert_v1 BEFORE INSERT ON episode_completions WHEN (SELECT COUNT(*) FROM s2_lite_migration_source_owner_v1) != 0 OR (SELECT COUNT(*) FROM s2_lite_migration_source_guard_v1) != 0 BEGIN SELECT RAISE(ABORT, 'S2_MIGRATION_SOURCE_PROTECTED'); END;
         CREATE TRIGGER s2_lite_guard_episode_update_v1 BEFORE UPDATE ON episode_completions WHEN (SELECT COUNT(*) FROM s2_lite_migration_source_owner_v1) != 0 OR (SELECT COUNT(*) FROM s2_lite_migration_source_guard_v1) != 0 BEGIN SELECT RAISE(ABORT, 'S2_MIGRATION_SOURCE_PROTECTED'); END;
         CREATE TRIGGER s2_lite_guard_episode_delete_v1 BEFORE DELETE ON episode_completions WHEN (SELECT COUNT(*) FROM s2_lite_migration_source_owner_v1) != 0 OR (SELECT COUNT(*) FROM s2_lite_migration_source_guard_v1) != 0 BEGIN SELECT RAISE(ABORT, 'S2_MIGRATION_SOURCE_PROTECTED'); END;
         CREATE TRIGGER s2_lite_guard_collections_insert_v1 BEFORE INSERT ON collections WHEN (SELECT COUNT(*) FROM s2_lite_migration_source_owner_v1) != 0 OR (SELECT COUNT(*) FROM s2_lite_migration_source_guard_v1) != 0 BEGIN SELECT RAISE(ABORT, 'S2_MIGRATION_SOURCE_PROTECTED'); END;
         CREATE TRIGGER s2_lite_guard_collections_update_v1 BEFORE UPDATE ON collections WHEN (SELECT COUNT(*) FROM s2_lite_migration_source_owner_v1) != 0 OR (SELECT COUNT(*) FROM s2_lite_migration_source_guard_v1) != 0 BEGIN SELECT RAISE(ABORT, 'S2_MIGRATION_SOURCE_PROTECTED'); END;
         CREATE TRIGGER s2_lite_guard_collections_delete_v1 BEFORE DELETE ON collections WHEN (SELECT COUNT(*) FROM s2_lite_migration_source_owner_v1) != 0 OR (SELECT COUNT(*) FROM s2_lite_migration_source_guard_v1) != 0 BEGIN SELECT RAISE(ABORT, 'S2_MIGRATION_SOURCE_PROTECTED'); END;
         CREATE TRIGGER s2_lite_guard_members_insert_v1 BEFORE INSERT ON collection_members WHEN (SELECT COUNT(*) FROM s2_lite_migration_source_owner_v1) != 0 OR (SELECT COUNT(*) FROM s2_lite_migration_source_guard_v1) != 0 BEGIN SELECT RAISE(ABORT, 'S2_MIGRATION_SOURCE_PROTECTED'); END;
         CREATE TRIGGER s2_lite_guard_members_update_v1 BEFORE UPDATE ON collection_members WHEN (SELECT COUNT(*) FROM s2_lite_migration_source_owner_v1) != 0 OR (SELECT COUNT(*) FROM s2_lite_migration_source_guard_v1) != 0 BEGIN SELECT RAISE(ABORT, 'S2_MIGRATION_SOURCE_PROTECTED'); END;
         CREATE TRIGGER s2_lite_guard_members_delete_v1 BEFORE DELETE ON collection_members WHEN (SELECT COUNT(*) FROM s2_lite_migration_source_owner_v1) != 0 OR (SELECT COUNT(*) FROM s2_lite_migration_source_guard_v1) != 0 BEGIN SELECT RAISE(ABORT, 'S2_MIGRATION_SOURCE_PROTECTED'); END;",
    )
}

fn encode<T: Serialize>(value: &T) -> Result<Vec<u8>> {
    serde_json::to_vec(&PersistenceEnvelopeRefV1 {
        persistence_version: PERSISTENCE_FORMAT_VERSION,
        payload: value,
    })
    .map_err(|_| STORE_CORRUPTION)
}

fn decode<T: DeserializeOwned + Serialize>(bytes: &[u8]) -> Result<T> {
    let envelope: PersistenceEnvelopeValueV1 =
        s2_serde_json::from_slice(bytes).map_err(|_| STORE_CORRUPTION)?;
    if envelope.persistence_version != PERSISTENCE_FORMAT_VERSION {
        return Err(STORE_CORRUPTION);
    }

    // Deserialize through a Value so the original durable representation is
    // retained. Comparing it with the frozen type's serialization catches both
    // unknown nested fields (which permissive core structs might ignore) and
    // omitted fields supplied by core serde defaults. This is intentionally a
    // persistence boundary check; frozen wire/core serde behavior is unchanged.
    let value: T =
        serde_json::from_value(envelope.payload.clone()).map_err(|_| STORE_CORRUPTION)?;
    let canonical_payload = serde_json::to_value(&value).map_err(|_| STORE_CORRUPTION)?;
    if canonical_payload != envelope.payload {
        return Err(STORE_CORRUPTION);
    }
    Ok(value)
}

fn database<T>(result: rusqlite::Result<T>) -> Result<T> {
    result.map_err(|_| STORE_FAILURE)
}

fn canonical_generation(value: &str) -> Result<u64> {
    if value.is_empty()
        || (value.len() > 1 && value.starts_with('0'))
        || !value.bytes().all(|byte| byte.is_ascii_digit())
    {
        return Err(STORE_CORRUPTION);
    }
    value.parse().map_err(|_| STORE_CORRUPTION)
}

fn valid_outbound_entity_kind(value: &str) -> bool {
    matches!(
        value,
        "record" | "collection" | "collection-member" | "episode-completion"
    )
}

fn validate_desktop_root_state(state: &DesktopRootStateV1, root_id: &str) -> Result<()> {
    if state.state_version != 1
        || state.physical_root_id != root_id
        || validate_canonical_uuid_v4(&state.local_writer_id).is_err()
        || state.next_writer_sequence == 0
        || state.writer_head.as_ref().is_some_and(|value| {
            validate_commit_ref(value).is_err() || value.writer_id != state.local_writer_id
        })
        || state
            .business_applied_projection_generation
            .zip(state.materialized_projection_generation)
            .is_some_and(|(applied, materialized)| applied > materialized)
    {
        return Err(STORE_CORRUPTION);
    }
    Ok(())
}

fn validate_target_root_binding(binding: &TargetRootBindingV1) -> Result<()> {
    if binding.binding_version != 1
        || binding.target_id.is_empty()
        || binding.canonical_url.is_empty()
        || binding.normalized_account.is_empty()
        || binding.physical_root_id.is_empty()
    {
        return Err(STORE_CORRUPTION);
    }
    Ok(())
}

fn validate_migration_execution_binding(binding: &MigrationExecutionBindingV1) -> Result<()> {
    if binding.binding_version != 1
        || binding.physical_root_id.is_empty()
        || binding.target_id.is_empty()
        || binding.captured_records_generation < 0
        || validate_canonical_uuid_v4(&binding.migration_id).is_err()
        || binding
            .legacy_fingerprint
            .as_ref()
            .is_some_and(|fingerprint| {
                fingerprint.len() != 64
                    || !fingerprint
                        .bytes()
                        .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
            })
    {
        return Err(STORE_CORRUPTION);
    }
    Ok(())
}

pub fn migration_execution_identity_v1(
    binding: &MigrationExecutionBindingV1,
) -> MigrationExecutionIdentityV1 {
    MigrationExecutionIdentityV1 {
        physical_root_id: binding.physical_root_id.clone(),
        target_id: binding.target_id.clone(),
        target_epoch: binding.target_epoch,
        captured_records_generation: binding.captured_records_generation,
        legacy_fingerprint: binding.legacy_fingerprint.clone(),
        migration_id: binding.migration_id.clone(),
    }
}

fn load_target_root_binding_from(
    conn: &Connection,
    target_id: &str,
    target_epoch: u64,
) -> Result<Option<TargetRootBindingV1>> {
    let row = database(
        conn.query_row(
            "SELECT binding_version, target_id, target_epoch, canonical_url,
                    normalized_account, physical_root_id
             FROM s2_lite_target_root_binding_v1
             WHERE target_id=?1 AND target_epoch=?2",
            params![target_id, target_epoch.to_string()],
            |row| {
                Ok(TargetRootBindingV1 {
                    binding_version: row.get(0)?,
                    target_id: row.get(1)?,
                    target_epoch: canonical_generation(&row.get::<_, String>(2)?)
                        .map_err(|_| rusqlite::Error::InvalidQuery)?,
                    canonical_url: row.get(3)?,
                    normalized_account: row.get(4)?,
                    physical_root_id: row.get(5)?,
                })
            },
        )
        .optional(),
    )?;
    row.map(|binding| {
        validate_target_root_binding(&binding)?;
        if binding.target_id != target_id || binding.target_epoch != target_epoch {
            return Err(STORE_CORRUPTION);
        }
        Ok(binding)
    })
    .transpose()
}

fn validate_migration_execution_target_authority(
    conn: &Connection,
    binding: &MigrationExecutionBindingV1,
    root_id: &str,
) -> Result<()> {
    let target_binding =
        load_target_root_binding_from(conn, &binding.target_id, binding.target_epoch)?
            .ok_or(ROOT_MISMATCH)?;
    if target_binding.physical_root_id != binding.physical_root_id
        || binding.physical_root_id != root_id
    {
        return Err(ROOT_MISMATCH);
    }
    Ok(())
}

/// Strictly decodes the root-scoped persistence value without yet treating the
/// binding's target/epoch as authority. Activation admission uses this narrow
/// boundary to compare its pinned identity before any target dereference.
fn load_migration_execution_binding_structural_from(
    conn: &Connection,
    root_id: &str,
) -> Result<Option<MigrationExecutionBindingV1>> {
    let bytes = database(
        conn.query_row(
            "SELECT state_json FROM s2_lite_migration_execution_binding_v1 WHERE root_id=?1",
            [root_id],
            |row| row.get::<_, Vec<u8>>(0),
        )
        .optional(),
    )?;
    bytes
        .map(|bytes| {
            let binding: MigrationExecutionBindingV1 = decode(&bytes)?;
            validate_migration_execution_binding(&binding)?;
            if binding.physical_root_id != root_id {
                return Err(STORE_CORRUPTION);
            }
            Ok(binding)
        })
        .transpose()
}

fn load_migration_execution_binding_from(
    conn: &Connection,
    root_id: &str,
) -> Result<Option<MigrationExecutionBindingV1>> {
    let binding = load_migration_execution_binding_structural_from(conn, root_id)?;
    binding
        .map(|binding| {
            validate_migration_execution_target_authority(conn, &binding, root_id)?;
            Ok(binding)
        })
        .transpose()
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct MigrationSourceOwnerV1 {
    root_id: String,
    migration_id: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct MigrationSourceGuardV1 {
    root_id: String,
    migration_id: String,
    captured_records_generation: i64,
}

fn load_migration_source_owner(conn: &Connection) -> Result<Option<MigrationSourceOwnerV1>> {
    let owners = database(
        (|| -> rusqlite::Result<Vec<(i64, MigrationSourceOwnerV1)>> {
            let mut statement = conn.prepare(
                "SELECT typeof(owner_key), owner_key, root_id, migration_id
             FROM s2_lite_migration_source_owner_v1
             ORDER BY owner_key, root_id, migration_id",
            )?;
            let owners = statement
                .query_map([], |row| {
                    let key_type: String = row.get(0)?;
                    if key_type != "integer" {
                        return Err(rusqlite::Error::InvalidQuery);
                    }
                    Ok((
                        row.get::<_, i64>(1)?,
                        MigrationSourceOwnerV1 {
                            root_id: row.get(2)?,
                            migration_id: row.get(3)?,
                        },
                    ))
                })?
                .collect::<rusqlite::Result<Vec<_>>>();
            owners
        })(),
    )?;
    let guards = database((|| -> rusqlite::Result<Vec<MigrationSourceGuardV1>> {
        let mut statement = conn.prepare(
            "SELECT typeof(captured_records_generation), root_id, migration_id,
                    captured_records_generation
             FROM s2_lite_migration_source_guard_v1
             ORDER BY root_id, migration_id",
        )?;
        let guards = statement
            .query_map([], |row| {
                let generation_type: String = row.get(0)?;
                let raw_generation: String = row.get(3)?;
                if generation_type != "text" {
                    return Err(rusqlite::Error::InvalidQuery);
                }
                let generation = canonical_generation(&raw_generation)
                    .map_err(|_| rusqlite::Error::InvalidQuery)?;
                Ok(MigrationSourceGuardV1 {
                    root_id: row.get(1)?,
                    migration_id: row.get(2)?,
                    captured_records_generation: i64::try_from(generation)
                        .map_err(|_| rusqlite::Error::InvalidQuery)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>();
        guards
    })())?;

    let (owner_key, owner, guard) = match (owners.as_slice(), guards.as_slice()) {
        ([], []) => return Ok(None),
        ([(owner_key, owner)], [guard]) => (*owner_key, owner, guard),
        _ => return Err(STORE_CORRUPTION),
    };
    if owner_key != 1
        || owner.root_id.is_empty()
        || guard.root_id.is_empty()
        || validate_canonical_uuid_v4(&owner.migration_id).is_err()
        || validate_canonical_uuid_v4(&guard.migration_id).is_err()
        || owner.root_id != guard.root_id
        || owner.migration_id != guard.migration_id
        || guard.captured_records_generation < 0
    {
        return Err(STORE_CORRUPTION);
    }
    let bytes = database(
        conn.query_row(
            "SELECT state_json FROM s2_lite_migration_execution_binding_v1 WHERE root_id=?1",
            [&guard.root_id],
            |row| row.get::<_, Vec<u8>>(0),
        )
        .optional(),
    )?
    .ok_or(STORE_CORRUPTION)?;
    let execution_binding: MigrationExecutionBindingV1 = decode(&bytes)?;
    validate_migration_execution_binding(&execution_binding)?;
    if execution_binding.physical_root_id != guard.root_id
        || execution_binding.migration_id != guard.migration_id
        || execution_binding.captured_records_generation != guard.captured_records_generation
    {
        return Err(STORE_CORRUPTION);
    }
    validate_migration_execution_target_authority(conn, &execution_binding, &guard.root_id)?;
    Ok(Some(owner.clone()))
}

fn validate_active_migration_binding(
    conn: &Connection,
    candidate: &TargetRootBindingV1,
    root_id: &str,
) -> Result<()> {
    validate_target_root_binding(candidate)?;
    if candidate.physical_root_id != root_id {
        return Err(ROOT_MISMATCH);
    }
    let registry = crate::sync_targets::registry(conn).map_err(|_| STORE_FAILURE)?;
    let registry = registry.ok_or(ROOT_MISMATCH)?;
    if registry.active_target_id.as_deref() != Some(candidate.target_id.as_str())
        || registry.target_epoch != candidate.target_epoch
    {
        return Err(ROOT_MISMATCH);
    }
    let target = registry
        .targets
        .iter()
        .find(|target| target.id == candidate.target_id)
        .ok_or(ROOT_MISMATCH)?;
    let root = super::webdav_adapter::webdav_root_v1(&target.normalized_url, &target.username)
        .map_err(|_| ROOT_MISMATCH)?;
    if root.canonical_url != candidate.canonical_url
        || root.normalized_account != candidate.normalized_account
        || root.physical_root_id != candidate.physical_root_id
    {
        return Err(ROOT_MISMATCH);
    }
    let durable =
        load_target_root_binding_from(conn, &candidate.target_id, candidate.target_epoch)?
            .ok_or(ROOT_MISMATCH)?;
    if durable != *candidate {
        return Err(ROOT_MISMATCH);
    }
    Ok(())
}

/// Revalidates the exact target/root/lifecycle authority at the final local
/// scheduler-success boundary.  The caller owns the surrounding `BEGIN
/// IMMEDIATE` transaction, so this decision and any subsequent scheduler write
/// observe one durable state and cannot be separated by a later freeze or
/// lifecycle transition.
pub fn validate_normal_s2_completion_authority_in_transaction_v1(
    conn: &Connection,
    binding: &TargetRootBindingV1,
) -> Result<NormalS2CompletionAuthorityV1> {
    validate_target_root_binding(binding)?;
    if !matches!(
        crate::sync_targets::active_target(conn).map_err(|_| STORE_FAILURE)?,
        Some((active_target_id, active_target_epoch))
            if active_target_id == binding.target_id && active_target_epoch == binding.target_epoch
    ) {
        return Ok(NormalS2CompletionAuthorityV1::TargetChanged);
    }
    if load_target_root_binding_from(conn, &binding.target_id, binding.target_epoch)?.as_ref()
        != Some(binding)
    {
        return Ok(NormalS2CompletionAuthorityV1::TargetChanged);
    }
    // The durable row alone is not enough: the current target configuration
    // must still derive this exact physical root as well.
    if validate_active_migration_binding(conn, binding, &binding.physical_root_id).is_err() {
        return Ok(NormalS2CompletionAuthorityV1::TargetChanged);
    }

    let safety = load_root_safety_from(conn, &binding.physical_root_id)?.ok_or(STORE_CORRUPTION)?;
    if !safety.root_fatal_signals.is_empty() || !safety.cutover_state.root_fatal_signals.is_empty()
    {
        return Ok(NormalS2CompletionAuthorityV1::ReadOnlyFrozen);
    }
    if !safety.cutover_state.remote_s2_activated {
        return Ok(NormalS2CompletionAuthorityV1::NoLongerNormalS2);
    }

    let migration = load_migration_from(conn, &binding.physical_root_id)?;
    let owner = load_migration_source_owner(conn)?;
    match migration {
        Some(migration) => {
            if migration.status == MigrationStatusV1::RootFrozen {
                return Ok(NormalS2CompletionAuthorityV1::ReadOnlyFrozen);
            }
            if owner.is_some() || migration.status != MigrationStatusV1::MigrationComplete {
                return Ok(NormalS2CompletionAuthorityV1::NoLongerNormalS2);
            }
            let execution = load_migration_execution_binding_from(conn, &binding.physical_root_id)?
                .ok_or(STORE_CORRUPTION)?;
            if execution.migration_id != migration.migration_id {
                return Err(STORE_CORRUPTION);
            }
            load_continued_completed_migration_writer_from(
                conn,
                &migration,
                &binding.physical_root_id,
            )?;
        }
        None => {
            if owner.is_some() {
                return Ok(NormalS2CompletionAuthorityV1::NoLongerNormalS2);
            }
            load_desktop_root_state_from(conn, &binding.physical_root_id)?
                .ok_or(STORE_CORRUPTION)?;
        }
    }
    Ok(NormalS2CompletionAuthorityV1::Valid)
}

pub(crate) fn completed_migration_writer_seed(
    migration: &MigrationStateV1,
    root_id: &str,
) -> Result<Option<(String, Option<CommitRef>, u64)>> {
    if migration.root_id != root_id {
        return Err(STORE_CORRUPTION);
    }
    if migration.status != MigrationStatusV1::MigrationComplete {
        return Ok(None);
    }
    let final_task = migration
        .stage_b
        .last()
        .or_else(|| migration.stage_a.last());
    let Some(final_task) = final_task else {
        return Ok(Some((migration.writer_id.clone(), None, 1)));
    };
    if final_task.receipt.is_none()
        || final_task.receipt_root_id.as_deref() != Some(root_id)
        || final_task.intent.commit_ref.writer_id != migration.writer_id
    {
        return Err(STORE_CORRUPTION);
    }
    let sequence = canonical_generation(&final_task.intent.commit_ref.writer_seq)?;
    Ok(Some((
        migration.writer_id.clone(),
        Some(final_task.intent.commit_ref.clone()),
        sequence.checked_add(1).ok_or(STORE_CORRUPTION)?,
    )))
}

fn validate_outbound_batch(batch: &OutboundBatchV1, root_id: &str) -> Result<()> {
    if batch.state_version != 1
        || batch.target_id.is_empty()
        || batch.physical_root_id != root_id
        || batch.state != "frozen"
        || validate_canonical_uuid_v4(&batch.batch_id).is_err()
        || validate_canonical_uuid_v4(&batch.writer_id).is_err()
        || validate_commit_ref(&batch.commit_ref).is_err()
        || batch.commit_ref.writer_id != batch.writer_id
        || canonical_generation(&batch.commit_ref.writer_seq)? != batch.writer_sequence
        || batch.captured_local_generation < 0
        || batch.prepared_intent_path.is_empty()
        || batch.prepared_intent_fingerprint.is_empty()
        || batch
            .previous_writer_ref
            .as_ref()
            .is_some_and(|value| validate_commit_ref(value).is_err())
        || batch
            .basis_clock
            .iter()
            .any(|value| validate_commit_ref(value).is_err())
        || batch.mutations.is_empty()
        || batch.mutations.iter().any(|mutation| {
            !valid_outbound_entity_kind(&mutation.entity_kind)
                || mutation.entity_id.is_empty()
                || mutation.entity_key.is_null()
                || mutation.captured_last_generation < 0
                || validate_canonical_uuid_v4(&mutation.local_mutation_id).is_err()
        })
    {
        return Err(STORE_CORRUPTION);
    }
    Ok(())
}

fn validate_materialized_projection(
    projection: &DurableMaterializedProjectionV1,
    root_id: &str,
) -> Result<()> {
    if projection.projection_version != 1
        || projection.physical_root_id != root_id
        || projection.state.state_version != 1
        || projection.replay_input_fingerprint != projection.state.replay_input_fingerprint
        || projection.replay_input_fingerprint.len() != 64
        || !projection
            .replay_input_fingerprint
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
        || projection
            .business_projection_applied_generation
            .is_some_and(|value| value > projection.projection_generation)
    {
        return Err(STORE_CORRUPTION);
    }
    Ok(())
}

fn validate_entity_projection_overlay_blocker(
    blocker: &EntityProjectionOverlayBlockerV1,
    root_id: &str,
    target_id: &str,
    entity_key: &Value,
) -> Result<()> {
    if blocker.state_version != 1
        || blocker.physical_root_id != root_id
        || blocker.target_id != target_id
        || blocker.entity_key != *entity_key
        || blocker.projection_entity_fingerprint.len() != 64
        || !blocker
            .projection_entity_fingerprint
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(STORE_CORRUPTION);
    }
    Ok(())
}

fn entity_projection_fingerprint(entity: &MaterializedProjectionEntityV1) -> Result<String> {
    jcs_bytes(entity).map(|bytes| sha256_hex(&bytes))
}

pub(crate) fn upsert_entity_projection_overlay_blocker_v1(
    conn: &Connection,
    root_id: &str,
    target_id: &str,
    projection_generation: u64,
    entity: &MaterializedProjectionEntityV1,
) -> Result<()> {
    if root_id.is_empty() || target_id.is_empty() {
        return Err(STORE_CORRUPTION);
    }
    let key = jcs_bytes(&entity.entity_key)?;
    let blocker = EntityProjectionOverlayBlockerV1 {
        state_version: 1,
        physical_root_id: root_id.to_string(),
        target_id: target_id.to_string(),
        entity_key: entity.entity_key.clone(),
        projection_generation,
        projection_entity_fingerprint: entity_projection_fingerprint(entity)?,
    };
    validate_entity_projection_overlay_blocker(&blocker, root_id, target_id, &entity.entity_key)?;
    database(conn.execute(
        "INSERT INTO s2_lite_entity_projection_overlay_blocker_v1(
            root_id, target_id, entity_key_jcs, state_json
         ) VALUES(?1, ?2, ?3, ?4)
         ON CONFLICT(root_id, target_id, entity_key_jcs) DO UPDATE SET state_json=excluded.state_json",
        params![root_id, target_id, key, encode(&blocker)?],
    ))?;
    Ok(())
}

pub fn load_entity_projection_overlay_blocker_v1(
    conn: &Connection,
    root_id: &str,
    target_id: &str,
    entity_key: &Value,
) -> Result<Option<EntityProjectionOverlayBlockerV1>> {
    let key = jcs_bytes(entity_key)?;
    let row = database(
        conn.query_row(
            "SELECT state_json FROM s2_lite_entity_projection_overlay_blocker_v1
         WHERE root_id=?1 AND target_id=?2 AND entity_key_jcs=?3",
            params![root_id, target_id, key],
            |row| row.get::<_, Vec<u8>>(0),
        )
        .optional(),
    )?;
    let Some(bytes) = row else {
        return Ok(None);
    };
    let blocker: EntityProjectionOverlayBlockerV1 = decode(&bytes)?;
    validate_entity_projection_overlay_blocker(&blocker, root_id, target_id, entity_key)?;
    Ok(Some(blocker))
}

pub fn entity_projection_overlay_blocker_exists_v1(
    conn: &Connection,
    root_id: &str,
    target_id: &str,
    entity_key: &Value,
) -> Result<bool> {
    Ok(load_entity_projection_overlay_blocker_v1(conn, root_id, target_id, entity_key)?.is_some())
}

pub(crate) fn clear_entity_projection_overlay_blocker_v1(
    conn: &Connection,
    root_id: &str,
    target_id: &str,
    entity_key: &Value,
) -> Result<()> {
    let key = jcs_bytes(entity_key)?;
    database(conn.execute(
        "DELETE FROM s2_lite_entity_projection_overlay_blocker_v1
         WHERE root_id=?1 AND target_id=?2 AND entity_key_jcs=?3",
        params![root_id, target_id, key],
    ))?;
    Ok(())
}

/// Strictly loads the current projection while a local business mutation is
/// already inside its SQLite transaction. Staging uses this only to capture an
/// entity-specific causal anchor; it cannot create or update projection state.
pub(crate) fn load_materialized_projection_for_staging_v1(
    conn: &Connection,
    root_id: &str,
) -> Result<Option<DurableMaterializedProjectionV1>> {
    let row = database(
        conn.query_row(
            "SELECT projection_generation, state_json
             FROM s2_lite_materialized_projection_v1 WHERE root_id=?1",
            [root_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?)),
        )
        .optional(),
    )?;
    row.map(|(generation, bytes)| {
        let projection: DurableMaterializedProjectionV1 = decode(&bytes)?;
        if projection.projection_generation != canonical_generation(&generation)? {
            return Err(STORE_CORRUPTION);
        }
        validate_materialized_projection(&projection, root_id)?;
        Ok(projection)
    })
    .transpose()
}

/// Loads the sole projection generation that can be used as causal authority
/// for a newly staged local entity.  This deliberately runs on the caller's
/// mutation transaction: the business-row edit, its staging record, and the
/// proof that those rows already reflect the projection are one SQLite view.
pub(crate) fn admit_applied_projection_for_staging_anchor_v1(
    conn: &Connection,
    root_id: &str,
) -> Result<StagingAnchorProjectionAdmissionV1> {
    let Some(projection) = load_materialized_projection_for_staging_v1(conn, root_id)? else {
        return Ok(StagingAnchorProjectionAdmissionV1::Unavailable(
            "materialized_projection_missing",
        ));
    };
    if !matches!(
        projection.state.status,
        super::materialized_projection::MaterializedProjectionStatusV1::Complete
    ) {
        return Ok(StagingAnchorProjectionAdmissionV1::Unavailable(
            "materialized_projection_incomplete",
        ));
    }
    if projection.business_projection_applied_generation != Some(projection.projection_generation) {
        return Ok(StagingAnchorProjectionAdmissionV1::Unavailable(
            "projection_business_not_applied",
        ));
    }

    let Some(safety) = load_root_safety_from(conn, root_id)? else {
        return Ok(StagingAnchorProjectionAdmissionV1::Unavailable(
            "root_safety_unavailable",
        ));
    };
    if !safety.root_fatal_signals.is_empty() {
        return Ok(StagingAnchorProjectionAdmissionV1::Unavailable(
            "root_fatal",
        ));
    }
    if projection.source_root_safety_generation != safety.generation {
        return Ok(StagingAnchorProjectionAdmissionV1::Unavailable(
            "projection_root_safety_generation_stale",
        ));
    }

    let Some(discovery) = load_discovery_state_from(conn, root_id)? else {
        return Ok(StagingAnchorProjectionAdmissionV1::Unavailable(
            "discovery_unavailable",
        ));
    };
    if projection.source_discovery_generation != discovery.storage_generation {
        return Ok(StagingAnchorProjectionAdmissionV1::Unavailable(
            "projection_discovery_generation_stale",
        ));
    }

    let root_bytes = database(
        conn.query_row(
            "SELECT state_json FROM s2_lite_desktop_root_state_v1 WHERE root_id=?1",
            [root_id],
            |row| row.get::<_, Vec<u8>>(0),
        )
        .optional(),
    )?;
    let Some(root_bytes) = root_bytes else {
        return Ok(StagingAnchorProjectionAdmissionV1::Unavailable(
            "desktop_root_state_missing",
        ));
    };
    let root_state: DesktopRootStateV1 = decode(&root_bytes)?;
    validate_desktop_root_state(&root_state, root_id)?;
    if root_state.materialized_projection_generation != Some(projection.projection_generation) {
        return Ok(StagingAnchorProjectionAdmissionV1::Unavailable(
            "desktop_materialized_projection_generation_stale",
        ));
    }
    if root_state.business_applied_projection_generation != Some(projection.projection_generation) {
        return Ok(StagingAnchorProjectionAdmissionV1::Unavailable(
            "desktop_business_projection_not_applied",
        ));
    }
    Ok(StagingAnchorProjectionAdmissionV1::Ready(Box::new(
        projection,
    )))
}

fn ensure_root_authority(conn: &Connection, root_id: &str) -> Result<()> {
    if root_id.is_empty() {
        return Err(ROOT_MISMATCH);
    }
    let state = create_migration_root_safety_state_v1(root_id);
    database(conn.execute(
        "INSERT INTO s2_lite_root_authority_v1(root_id, authority_generation, state_json)
         VALUES(?1, '0', ?2) ON CONFLICT(root_id) DO NOTHING",
        params![root_id, encode(&state)?],
    ))?;
    Ok(())
}

fn load_root_safety_from(
    conn: &Connection,
    root_id: &str,
) -> Result<Option<MigrationRootSafetyStateV1>> {
    let row = database(
        conn.query_row(
            "SELECT authority_generation, state_json
             FROM s2_lite_root_authority_v1 WHERE root_id=?1",
            [root_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?)),
        )
        .optional(),
    )?;
    let Some((generation, bytes)) = row else {
        return Ok(None);
    };
    let state: MigrationRootSafetyStateV1 = decode(&bytes)?;
    let normalized_cutover = merge_migration_root_cutover_state_v1(
        &super::activation_cutover::create_activation_cutover_state_v1(),
        &state.cutover_state,
    )?;
    if state.state_version != 1
        || state.root_id != root_id
        || state.generation != canonical_generation(&generation)?
        || normalized_cutover != state.cutover_state
        || !strictly_sorted_unique(
            state
                .root_fatal_signals
                .iter()
                .map(|fatal| fatal.code.as_str()),
        )
        || state.cutover_state.root_fatal_signals.iter().any(|fatal| {
            !state
                .root_fatal_signals
                .iter()
                .any(|root_fatal| root_fatal.code == fatal.code)
        })
    {
        return Err(STORE_CORRUPTION);
    }
    Ok(Some(state))
}

fn validate_legacy_route_ticket_shape_v1(ticket: &LegacyRouteTicketV1) -> Result<()> {
    if ticket.target_id.is_empty() || ticket.physical_root_id.is_empty() {
        return Err(STORE_CORRUPTION);
    }
    Ok(())
}

/// Re-reads every authority fact represented by a legacy handoff ticket.  The
/// caller owns the surrounding SQLite write transaction; in particular, every
/// S1 mutation calls this after `BEGIN IMMEDIATE` and before its first write.
pub fn validate_legacy_route_ticket_v1(
    conn: &Connection,
    ticket: &LegacyRouteTicketV1,
) -> Result<LegacyRouteTicketValidationV1> {
    validate_legacy_route_ticket_shape_v1(ticket)?;
    let active = crate::sync_targets::active_target(conn).map_err(|_| STORE_FAILURE)?;
    if !matches!(
        active,
        Some((target_id, target_epoch)) if target_id == ticket.target_id && target_epoch == ticket.target_epoch
    ) {
        return Ok(LegacyRouteTicketValidationV1::TargetChanged);
    }
    let binding = load_target_root_binding_from(conn, &ticket.target_id, ticket.target_epoch)?
        .ok_or(STORE_CORRUPTION)?;
    if binding.physical_root_id != ticket.physical_root_id {
        return Ok(LegacyRouteTicketValidationV1::TargetChanged);
    }
    let safety = load_root_safety_from(conn, &ticket.physical_root_id)?.ok_or(STORE_CORRUPTION)?;
    if !safety.root_fatal_signals.is_empty() {
        return Ok(LegacyRouteTicketValidationV1::ReadOnlyFrozen);
    }
    if safety.generation != ticket.root_safety_generation {
        return Ok(LegacyRouteTicketValidationV1::NoLongerLegacy);
    }
    if load_migration_source_owner(conn)?.is_some() {
        return Ok(LegacyRouteTicketValidationV1::NoLongerLegacy);
    }
    let discovery = load_discovery_state_from(conn, &ticket.physical_root_id)?
        .map(|value| value.state)
        .unwrap_or_else(create_discovery_state_v1);
    if decide_legacy_put_v1(&recover_activation_cutover_v1(
        &discovery,
        Some(&safety.cutover_state),
    )) != LegacyPutDecisionV1::AllowedS2NotActivated
    {
        return Ok(LegacyRouteTicketValidationV1::NoLongerLegacy);
    }
    Ok(LegacyRouteTicketValidationV1::Valid)
}

/// Captures a ticket only from a still-current, still-legacy authority state.
/// It holds `BEGIN IMMEDIATE` while checking the binding and root safety so the
/// resulting ticket corresponds to one coherent durable snapshot.
pub fn capture_legacy_route_ticket_v1(
    conn: &Mutex<Connection>,
    binding: &TargetRootBindingV1,
) -> Result<LegacyRouteTicketV1> {
    validate_target_root_binding(binding)?;
    let mut guard = conn.lock().map_err(|_| STORE_FAILURE)?;
    let transaction = database(guard.transaction_with_behavior(TransactionBehavior::Immediate))?;
    let safety =
        load_root_safety_from(&transaction, &binding.physical_root_id)?.ok_or(STORE_CORRUPTION)?;
    let ticket = LegacyRouteTicketV1 {
        target_id: binding.target_id.clone(),
        target_epoch: binding.target_epoch,
        physical_root_id: binding.physical_root_id.clone(),
        root_safety_generation: safety.generation,
    };
    if validate_legacy_route_ticket_v1(&transaction, &ticket)?
        != LegacyRouteTicketValidationV1::Valid
    {
        return Err(STORE_FAILURE);
    }
    database(transaction.commit())?;
    Ok(ticket)
}

fn strictly_sorted_unique<'a>(values: impl Iterator<Item = &'a str>) -> bool {
    let mut prior: Option<&str> = None;
    for value in values {
        if value.is_empty() || prior.is_some_and(|prior| prior >= value) {
            return false;
        }
        prior = Some(value);
    }
    true
}

fn save_root_safety(conn: &Connection, state: &MigrationRootSafetyStateV1) -> Result<()> {
    let changed = database(conn.execute(
        "UPDATE s2_lite_root_authority_v1
         SET authority_generation=?2, state_json=?3 WHERE root_id=?1",
        params![state.root_id, state.generation.to_string(), encode(state)?],
    ))?;
    if changed != 1 {
        return Err(STORE_FAILURE);
    }
    Ok(())
}

fn load_migration_from(conn: &Connection, root_id: &str) -> Result<Option<MigrationStateV1>> {
    let row = database(
        conn.query_row(
            "SELECT migration_id, migration_generation, state_json
             FROM s2_lite_migration_v1 WHERE root_id=?1",
            [root_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Vec<u8>>(2)?,
                ))
            },
        )
        .optional(),
    )?;
    let Some((migration_id, generation, bytes)) = row else {
        return Ok(None);
    };
    let stored: MigrationStateV1 = decode(&bytes)?;
    let state = reconcile_migration_state_v1(&stored).map_err(|_| STORE_CORRUPTION)?;
    if state != stored
        || state.root_id != root_id
        || state.migration_id != migration_id
        || state.generation != canonical_generation(&generation)?
    {
        return Err(STORE_CORRUPTION);
    }
    Ok(Some(state))
}

fn insert_migration(conn: &Connection, state: &MigrationStateV1) -> Result<()> {
    database(conn.execute(
        "INSERT INTO s2_lite_migration_v1(
            root_id, migration_id, migration_generation, state_json
         ) VALUES(?1, ?2, ?3, ?4)",
        params![
            state.root_id,
            state.migration_id,
            state.generation.to_string(),
            encode(state)?,
        ],
    ))?;
    Ok(())
}

fn save_migration(conn: &Connection, state: &MigrationStateV1) -> Result<()> {
    let changed = database(conn.execute(
        "UPDATE s2_lite_migration_v1 SET migration_id=?2,
         migration_generation=?3, state_json=?4 WHERE root_id=?1",
        params![
            state.root_id,
            state.migration_id,
            state.generation.to_string(),
            encode(state)?,
        ],
    ))?;
    if changed != 1 {
        return Err(STORE_FAILURE);
    }
    Ok(())
}

fn add_fatal(state: &mut MigrationStateV1, code: &str) -> Result<bool> {
    if state
        .root_fatal_signals
        .iter()
        .any(|fatal| fatal.code == code)
    {
        return Ok(false);
    }
    state.generation = state.generation.checked_add(1).ok_or(STORE_CORRUPTION)?;
    state.status = MigrationStatusV1::RootFrozen;
    state.root_fatal_signals.push(MigrationRootFatalV1 {
        code: code.to_string(),
    });
    state
        .root_fatal_signals
        .sort_by(|left, right| left.code.cmp(&right.code));
    Ok(true)
}

/// Discovery facts are independently durable authority. A discovery CAS that
/// commits a root-fatal fact must latch the root in the very same transaction;
/// waiting for a later replay would leave a publication-admission window.
fn merge_discovery_fatals_into_root_authority(
    conn: &Connection,
    root_id: &str,
    discovery: &DiscoveryStateV1,
) -> Result<()> {
    if discovery
        .root_fatal_signals
        .iter()
        .any(|signal| signal.code.is_empty())
    {
        return Err(STORE_CORRUPTION);
    }
    if discovery.root_fatal_signals.is_empty() {
        return Ok(());
    }
    let mut safety = load_root_safety_from(conn, root_id)?.ok_or(STORE_CORRUPTION)?;
    let mut codes = safety
        .root_fatal_signals
        .iter()
        .map(|fatal| fatal.code.clone())
        .chain(
            discovery
                .root_fatal_signals
                .iter()
                .map(|signal| signal.code.clone()),
        )
        .collect::<Vec<_>>();
    codes.sort();
    codes.dedup();
    if codes.len() == safety.root_fatal_signals.len() {
        return Ok(());
    }
    safety.generation = safety.generation.checked_add(1).ok_or(STORE_CORRUPTION)?;
    safety.root_fatal_signals = codes
        .iter()
        .cloned()
        .map(|code| MigrationRootFatalV1 { code })
        .collect();
    save_root_safety(conn, &safety)?;
    if let Some(mut migration) = load_migration_from(conn, root_id)? {
        let mut changed = false;
        for code in codes {
            changed |= add_fatal(&mut migration, &code)?;
        }
        if changed {
            save_migration(conn, &migration)?;
        }
    }
    Ok(())
}

fn inherit_root_fatals_on_claim(
    state: &mut MigrationStateV1,
    root_fatals: &[MigrationRootFatalV1],
) -> Result<()> {
    let mut codes = state
        .root_fatal_signals
        .iter()
        .map(|fatal| fatal.code.clone())
        .chain(root_fatals.iter().map(|fatal| fatal.code.clone()))
        .collect::<Vec<_>>();
    codes.sort();
    codes.dedup();
    if codes.len() != state.root_fatal_signals.len() {
        state.generation = state.generation.checked_add(1).ok_or(STORE_CORRUPTION)?;
        state.status = MigrationStatusV1::RootFrozen;
        state.root_fatal_signals = codes
            .into_iter()
            .map(|code| MigrationRootFatalV1 { code })
            .collect();
    }
    Ok(())
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VersionedDiscoveryStateV1 {
    pub storage_generation: u64,
    pub state: DiscoveryStateV1,
}

fn load_desktop_root_state_from(
    conn: &Connection,
    root_id: &str,
) -> Result<Option<DesktopRootStateV1>> {
    let bytes = database(
        conn.query_row(
            "SELECT state_json FROM s2_lite_desktop_root_state_v1 WHERE root_id=?1",
            [root_id],
            |row| row.get::<_, Vec<u8>>(0),
        )
        .optional(),
    )?;
    bytes
        .map(|bytes| {
            let state: DesktopRootStateV1 = decode(&bytes)?;
            validate_desktop_root_state(&state, root_id)?;
            Ok(state)
        })
        .transpose()
}

fn initialize_desktop_writer_from(conn: &Connection, root_id: &str) -> Result<DesktopRootStateV1> {
    ensure_root_authority(conn, root_id)?;
    let migration = load_migration_from(conn, root_id)?;
    let migration_seed = migration
        .as_ref()
        .map(|state| completed_migration_writer_seed(state, root_id))
        .transpose()?
        .flatten();
    let existing = database(
        conn.query_row(
            "SELECT state_json FROM s2_lite_desktop_root_state_v1 WHERE root_id=?1",
            [root_id],
            |row| row.get::<_, Vec<u8>>(0),
        )
        .optional(),
    )?;
    match existing {
        Some(bytes) => {
            let state: DesktopRootStateV1 = decode(&bytes)?;
            validate_desktop_root_state(&state, root_id)?;
            if let Some((writer_id, writer_head, minimum_next_sequence)) = migration_seed {
                if state.local_writer_id != writer_id
                    || state.writer_head != writer_head
                    || state.next_writer_sequence < minimum_next_sequence
                {
                    return Err(STORE_CORRUPTION);
                }
            }
            Ok(state)
        }
        None => {
            let (local_writer_id, writer_head, next_writer_sequence) = match migration_seed {
                Some(seed) => seed,
                None => (
                    super::local_authority::initialize_writer(conn)?.writer_id,
                    None,
                    1,
                ),
            };
            let state = DesktopRootStateV1 {
                state_version: 1,
                physical_root_id: root_id.to_string(),
                local_writer_id,
                next_writer_sequence,
                writer_head,
                lifecycle_generation: 0,
                materialized_projection_generation: None,
                business_applied_projection_generation: None,
            };
            validate_desktop_root_state(&state, root_id)?;
            database(conn.execute(
                "INSERT INTO s2_lite_desktop_root_state_v1(root_id, state_json) VALUES(?1, ?2)",
                params![root_id, encode(&state)?],
            ))?;
            Ok(state)
        }
    }
}

/// A verified migration may only hand off to a writer that is absent or
/// already exactly equal to its frozen seed. This stricter pre-finalization
/// check prevents corrupt pre-existing authority from being accepted merely
/// because its sequence happens to be ahead of the bootstrap seed.
fn validate_verified_migration_writer_handoff_from(
    conn: &Connection,
    migration: &MigrationStateV1,
    root_id: &str,
) -> Result<()> {
    let Some((writer_id, writer_head, next_writer_sequence)) =
        completed_migration_writer_seed(migration, root_id)?
    else {
        return Err(STORE_CORRUPTION);
    };
    let existing = database(
        conn.query_row(
            "SELECT state_json FROM s2_lite_desktop_root_state_v1 WHERE root_id=?1",
            [root_id],
            |row| row.get::<_, Vec<u8>>(0),
        )
        .optional(),
    )?;
    if let Some(bytes) = existing {
        let state: DesktopRootStateV1 = decode(&bytes)?;
        validate_desktop_root_state(&state, root_id)?;
        if state.local_writer_id != writer_id
            || state.writer_head != writer_head
            || state.next_writer_sequence != next_writer_sequence
        {
            return Err(STORE_CORRUPTION);
        }
    }
    Ok(())
}

/// Completed migration state is validation-only: the finalization transaction
/// was solely responsible for installing this exact frozen writer seed.
fn load_exact_completed_migration_writer_from(
    conn: &Connection,
    migration: &MigrationStateV1,
    root_id: &str,
) -> Result<DesktopRootStateV1> {
    let Some((writer_id, writer_head, next_writer_sequence)) =
        completed_migration_writer_seed(migration, root_id)?
    else {
        return Err(STORE_CORRUPTION);
    };
    let bytes = database(
        conn.query_row(
            "SELECT state_json FROM s2_lite_desktop_root_state_v1 WHERE root_id=?1",
            [root_id],
            |row| row.get::<_, Vec<u8>>(0),
        )
        .optional(),
    )?
    .ok_or(STORE_CORRUPTION)?;
    let state: DesktopRootStateV1 = decode(&bytes)?;
    validate_desktop_root_state(&state, root_id)?;
    if state.local_writer_id != writer_id
        || state.writer_head != writer_head
        || state.next_writer_sequence != next_writer_sequence
    {
        return Err(STORE_CORRUPTION);
    }
    Ok(state)
}

fn has_root_bound_commit_receipt_from(
    conn: &Connection,
    root_id: &str,
    expected: &CommitRef,
) -> Result<bool> {
    let mut statement = database(conn.prepare(
        "SELECT remote_path FROM s2_lite_published_receipt_v1
         WHERE root_id=?1 AND receipt_kind='commit' ORDER BY remote_path",
    ))?;
    let rows = database(statement.query_map([root_id], |row| row.get::<_, String>(0)))?;
    let paths = database(rows.collect::<std::result::Result<Vec<_>, _>>())?;
    paths.into_iter().try_fold(false, |found, path| {
        Ok(found
            || load_commit_receipt_from(conn, root_id, &path)?
                .is_some_and(|receipt| receipt.commit_ref == *expected))
    })
}

/// Once the finalization transaction has installed the exact migration writer
/// seed, ordinary S2 completion is allowed to advance that same writer. A
/// later normal-route admission therefore verifies continuity, not the stale
/// handoff snapshot. The exact check above remains mandatory for the one
/// finalization admission that still carries the historical execution binding.
fn load_continued_completed_migration_writer_from(
    conn: &Connection,
    migration: &MigrationStateV1,
    root_id: &str,
) -> Result<DesktopRootStateV1> {
    let Some((writer_id, initial_writer_head, minimum_next_sequence)) =
        completed_migration_writer_seed(migration, root_id)?
    else {
        return Err(STORE_CORRUPTION);
    };
    let bytes = database(
        conn.query_row(
            "SELECT state_json FROM s2_lite_desktop_root_state_v1 WHERE root_id=?1",
            [root_id],
            |row| row.get::<_, Vec<u8>>(0),
        )
        .optional(),
    )?
    .ok_or(STORE_CORRUPTION)?;
    let state: DesktopRootStateV1 = decode(&bytes)?;
    validate_desktop_root_state(&state, root_id)?;
    if state.local_writer_id != writer_id || state.next_writer_sequence < minimum_next_sequence {
        return Err(STORE_CORRUPTION);
    }
    if state.writer_head == initial_writer_head
        && state.next_writer_sequence == minimum_next_sequence
    {
        return Ok(state);
    }

    // There is exactly one durable outbound row per root. It is the latest
    // writer authority, whether its bookkeeping is still pending or has
    // completed. Retained historical receipts cannot replace this ordering
    // evidence.
    let latest = database(
        conn.query_row(
            "SELECT state_json FROM s2_lite_outbound_batch_v1 WHERE root_id=?1",
            [root_id],
            |row| row.get::<_, Vec<u8>>(0),
        )
        .optional(),
    )?
    .map(|bytes| {
        let batch: OutboundBatchV1 = decode(&bytes)?;
        validate_outbound_batch(&batch, root_id)?;
        Ok(batch)
    })
    .transpose()?;

    let Some(batch) = latest else {
        return Err(STORE_CORRUPTION);
    };
    if !batch.bookkeeping_completed {
        // A reservation is the only legal pre-receipt continuation. It
        // retains its exact predecessor head and advances the sequence once.
        let predecessor_is_verified = match batch.previous_writer_ref.as_ref() {
            None => initial_writer_head.is_none(),
            Some(head) => {
                initial_writer_head.as_ref() == Some(head)
                    || has_root_bound_commit_receipt_from(conn, root_id, head)?
            }
        };
        if predecessor_is_verified
            && state.writer_head == batch.previous_writer_ref
            && batch.writer_id == writer_id
            && batch.writer_sequence.checked_add(1) == Some(state.next_writer_sequence)
        {
            return Ok(state);
        }
        return Err(STORE_CORRUPTION);
    }

    // A completed latest batch must remain the exact current head. A receipt
    // proves publication of this batch, not merely that some older head was
    // once published.
    let Some(head) = state.writer_head.as_ref() else {
        return Err(STORE_CORRUPTION);
    };
    let head_sequence = head
        .writer_seq
        .parse::<u64>()
        .map_err(|_| STORE_CORRUPTION)?;
    if batch.commit_ref != *head
        || batch.writer_id != writer_id
        || batch.writer_sequence != head_sequence
        || head.writer_id != writer_id
        || head_sequence.checked_add(1) != Some(state.next_writer_sequence)
    {
        return Err(STORE_CORRUPTION);
    }
    let receipt = load_commit_receipt_from(conn, root_id, &batch.prepared_intent_path)?
        .ok_or(STORE_CORRUPTION)?;
    if receipt.commit_ref != batch.commit_ref
        || receipt.remote_path != batch.prepared_intent_path
        || receipt.prepared_intent_fingerprint != batch.prepared_intent_fingerprint
    {
        return Err(STORE_CORRUPTION);
    }
    Ok(state)
}

fn load_frozen_migration_execution_from(
    conn: &Connection,
    migration: &MigrationStateV1,
    root_id: &str,
) -> Result<MigrationExecutionBindingV1> {
    let execution =
        load_migration_execution_binding_from(conn, root_id)?.ok_or(STORE_CORRUPTION)?;
    let snapshot = migration.snapshot.as_ref().ok_or(STORE_CORRUPTION)?;
    if execution.physical_root_id != root_id
        || execution.migration_id != migration.migration_id
        || execution.legacy_fingerprint.as_deref() != Some(snapshot.legacy_fingerprint.as_str())
    {
        return Err(STORE_CORRUPTION);
    }
    Ok(execution)
}

/// SQLite-backed S2 Lite state bound to one physical remote root.
///
/// Every clone shares the application's existing connection mutex. This lets the
/// separate frozen store traits participate in one root-scoped SQLite authority.
#[derive(Clone, Copy)]
pub struct SqliteS2LiteStoreV1<'a> {
    conn: &'a Mutex<Connection>,
    root_id: &'a str,
}

impl<'a> SqliteS2LiteStoreV1<'a> {
    pub fn open(conn: &'a Mutex<Connection>, root_id: &'a str) -> Result<Self> {
        if root_id.is_empty() {
            return Err(ROOT_MISMATCH);
        }
        {
            let guard = conn.lock().map_err(|_| STORE_FAILURE)?;
            database(migrate_schema(&guard))?;
        }
        Ok(Self { conn, root_id })
    }

    /// Bridges validated I6.3 authority into frozen migration storage. The
    /// observation, activation latch, fatal facts and replay cache commit together.
    pub fn refresh_from_read_authority_v1(&mut self) -> Result<()> {
        let mut conn = self.connection()?;
        let transaction = database(conn.transaction_with_behavior(TransactionBehavior::Immediate))?;
        ensure_root_authority(&transaction, self.root_id)?;
        let Some(read) =
            super::discovery_persistence::load_read_state_v1(&transaction, self.root_id)?
        else {
            database(transaction.commit())?;
            return Ok(());
        };
        let old = load_discovery_state_from(&transaction, self.root_id)?;
        let changed = old.as_ref().map(|old| &old.state) != Some(&read.discovery.state);
        let gen = if changed {
            old.as_ref().map_or(Ok(1), |old| {
                old.storage_generation
                    .checked_add(1)
                    .ok_or(STORE_CORRUPTION)
            })?
        } else {
            old.as_ref().unwrap().storage_generation
        };
        if changed {
            database(transaction.execute("INSERT INTO s2_lite_migration_discovery_v1(root_id,storage_generation,state_json) VALUES(?1,?2,?3) ON CONFLICT(root_id) DO UPDATE SET storage_generation=excluded.storage_generation,state_json=excluded.state_json",params![self.root_id,gen.to_string(),encode(&read.discovery.state)?]))?;
        }
        merge_discovery_fatals_into_root_authority(
            &transaction,
            self.root_id,
            &read.discovery.state,
        )?;
        let mut safety =
            load_root_safety_from(&transaction, self.root_id)?.ok_or(STORE_CORRUPTION)?;
        let next_cutover =
            recover_activation_cutover_v1(&read.discovery.state, Some(&safety.cutover_state))
                .diagnostic_state()
                .ok_or(STORE_CORRUPTION)?;
        let mut codes = safety
            .root_fatal_signals
            .iter()
            .map(|fatal| fatal.code.clone())
            .chain(read.fatal_codes)
            .chain(
                next_cutover
                    .root_fatal_signals
                    .iter()
                    .map(|fatal| fatal.code.clone()),
            )
            .collect::<Vec<_>>();
        if let Some(binding) = load_migration_execution_binding_from(&transaction, self.root_id)? {
            if next_cutover.remote_s2_activated
                && !matches!(&next_cutover.fingerprint_consistency,ActivationFingerprintConsistencyV1::Consistent{legacy_fingerprint} if *legacy_fingerprint==binding.legacy_fingerprint)
            {
                codes.push("SYNC_ROOT_FROZEN_LEGACY_CHANGE".into());
            }
        }
        codes.sort();
        codes.dedup();
        let fatals = codes
            .into_iter()
            .map(|code| MigrationRootFatalV1 { code })
            .collect::<Vec<_>>();
        if safety.cutover_state != next_cutover || safety.root_fatal_signals != fatals {
            safety.generation = safety.generation.checked_add(1).ok_or(STORE_CORRUPTION)?;
            safety.cutover_state = next_cutover;
            safety.root_fatal_signals = fatals;
            save_root_safety(&transaction, &safety)?;
        }
        if let Some(mut migration) = load_migration_from(&transaction, self.root_id)? {
            let mut changed = false;
            for fatal in &safety.root_fatal_signals {
                changed |= add_fatal(&mut migration, &fatal.code)?;
            }
            if changed {
                save_migration(&transaction, &migration)?;
            }
        }
        let row=database(transaction.query_row("SELECT projection_generation,state_json FROM s2_lite_materialized_projection_v1 WHERE root_id=?1",[self.root_id],|row|Ok((row.get::<_,String>(0)?,row.get::<_,Vec<u8>>(1)?))).optional())?;
        let previous = row
            .as_ref()
            .map(|(generation, bytes)| {
                let projection: DurableMaterializedProjectionV1 = decode(bytes)?;
                if projection.projection_generation != canonical_generation(generation)? {
                    return Err(STORE_CORRUPTION);
                }
                validate_materialized_projection(&projection, self.root_id)?;
                Ok(projection)
            })
            .transpose()?;
        let same = previous.as_ref().is_some_and(|p| {
            p.source_discovery_generation == gen
                && p.source_root_safety_generation == safety.generation
                && p.state == read.projection.state
        });
        if !same {
            let projection = DurableMaterializedProjectionV1 {
                projection_version: 1,
                physical_root_id: self.root_id.into(),
                projection_generation: previous.as_ref().map_or(Ok(1), |p| {
                    p.projection_generation
                        .checked_add(1)
                        .ok_or(STORE_CORRUPTION)
                })?,
                source_discovery_generation: gen,
                source_root_safety_generation: safety.generation,
                replay_input_fingerprint: read.projection.state.replay_input_fingerprint.clone(),
                business_projection_applied_generation: None,
                state: read.projection.state,
            };
            database(transaction.execute("INSERT INTO s2_lite_materialized_projection_v1(root_id,projection_generation,state_json) VALUES(?1,?2,?3) ON CONFLICT(root_id) DO UPDATE SET projection_generation=excluded.projection_generation,state_json=excluded.state_json",params![self.root_id,projection.projection_generation.to_string(),encode(&projection)?]))?;
        }
        database(transaction.commit())?;
        Ok(())
    }

    /// Captures the original local compatibility basis once. Subsequent remote
    /// projection or local edits must never rederive the activation fingerprint.
    pub fn admit_remote_activation_projection_v1(
        &mut self,
        binding: &TargetRootBindingV1,
    ) -> Result<()> {
        let mut conn = self.connection()?;
        let tx = database(conn.transaction_with_behavior(TransactionBehavior::Immediate))?;
        validate_active_migration_binding(&tx, binding, self.root_id)?;
        let mut safety = load_root_safety_from(&tx, self.root_id)?.ok_or(STORE_CORRUPTION)?;
        if !safety.root_fatal_signals.is_empty() || !safety.cutover_state.remote_s2_activated {
            return Err(ROOT_MISMATCH);
        }
        let retained = database(
            tx.query_row(
                "SELECT state_json FROM s2_lite_remote_activation_adoption_v1 WHERE root_id=?1",
                [self.root_id],
                |row| row.get::<_, Vec<u8>>(0),
            )
            .optional(),
        )?;
        let adoption = if let Some(bytes) = retained {
            let adoption: RemoteActivationAdoptionV1 = decode(&bytes)?;
            validate_remote_activation_adoption_v1(&adoption, self.root_id)?;
            adoption
        } else {
            let (generation, snapshot) =
                super::migration_admission::capture_production_legacy_snapshot_v1(&tx)?;
            let has_legacy_baseline = crate::db_atomic_helpers::get_setting_tx(
                &tx,
                &crate::sync_targets::scoped_key(&binding.target_id, "baseline_v3"),
            )
            .map_err(|_| STORE_FAILURE)?
            .is_some();
            RemoteActivationAdoptionV1 {
                state_version: 1,
                physical_root_id: self.root_id.into(),
                captured_records_generation: generation,
                legacy_fingerprint: if has_legacy_baseline
                    || !snapshot.canonical_entities.is_empty()
                {
                    Some(snapshot.legacy_fingerprint.clone())
                } else {
                    None
                },
                has_legacy_baseline,
                snapshot,
            }
        };
        validate_remote_activation_adoption_v1(&adoption, self.root_id)?;
        if !matches!(&safety.cutover_state.fingerprint_consistency,ActivationFingerprintConsistencyV1::Consistent{legacy_fingerprint} if *legacy_fingerprint==adoption.legacy_fingerprint)
        {
            safety.generation = safety.generation.checked_add(1).ok_or(STORE_CORRUPTION)?;
            safety.root_fatal_signals.push(MigrationRootFatalV1 {
                code: "SYNC_ROOT_FROZEN_LEGACY_CHANGE".into(),
            });
            safety
                .root_fatal_signals
                .sort_by(|left, right| left.code.cmp(&right.code));
            save_root_safety(&tx, &safety)?;
            database(tx.commit())?;
            return Err(ProtocolError("SYNC_ROOT_FROZEN_LEGACY_CHANGE"));
        }
        database(tx.execute("INSERT INTO s2_lite_remote_activation_adoption_v1(root_id,state_json) VALUES(?1,?2) ON CONFLICT(root_id) DO NOTHING",params![self.root_id,encode(&adoption)?]))?;
        database(tx.commit())?;
        Ok(())
    }

    pub fn root_id(&self) -> &str {
        self.root_id
    }

    /// Loads a historical target/epoch binding without consulting the active
    /// target registry. Recovery callers must use this exact record rather than
    /// retargeting work to a newer active target.
    pub fn load_target_root_binding_v1(
        conn: &Mutex<Connection>,
        target_id: &str,
        target_epoch: u64,
    ) -> Result<Option<TargetRootBindingV1>> {
        if target_id.is_empty() {
            return Err(STORE_CORRUPTION);
        }
        let guard = conn.lock().map_err(|_| STORE_FAILURE)?;
        database(migrate_schema(&guard))?;
        let row = database(
            guard
                .query_row(
                    "SELECT binding_version, target_id, target_epoch, canonical_url,
                            normalized_account, physical_root_id
                     FROM s2_lite_target_root_binding_v1
                     WHERE target_id=?1 AND target_epoch=?2",
                    params![target_id, target_epoch.to_string()],
                    |row| {
                        Ok(TargetRootBindingV1 {
                            binding_version: row.get(0)?,
                            target_id: row.get(1)?,
                            target_epoch: canonical_generation(&row.get::<_, String>(2)?)
                                .map_err(|_| rusqlite::Error::InvalidQuery)?,
                            canonical_url: row.get(3)?,
                            normalized_account: row.get(4)?,
                            physical_root_id: row.get(5)?,
                        })
                    },
                )
                .optional(),
        )?;
        row.map(|binding| {
            validate_target_root_binding(&binding)?;
            if binding.target_id != target_id || binding.target_epoch != target_epoch {
                return Err(STORE_CORRUPTION);
            }
            Ok(binding)
        })
        .transpose()
    }

    /// Claims or reuses an immutable binding. The root authority is created in
    /// the same transaction, but an existing target/epoch row can never be
    /// updated or rebound to a different root.
    pub fn bind_target_root_v1(
        &mut self,
        candidate: &TargetRootBindingV1,
    ) -> Result<TargetRootBindingV1> {
        validate_target_root_binding(candidate)?;
        if candidate.physical_root_id != self.root_id {
            return Err(ROOT_MISMATCH);
        }
        let mut conn = self.connection()?;
        let transaction = database(conn.transaction_with_behavior(TransactionBehavior::Immediate))?;
        ensure_root_authority(&transaction, self.root_id)?;
        let existing = database(
            transaction
                .query_row(
                    "SELECT binding_version, target_id, target_epoch, canonical_url,
                            normalized_account, physical_root_id
                     FROM s2_lite_target_root_binding_v1
                     WHERE target_id=?1 AND target_epoch=?2",
                    params![candidate.target_id, candidate.target_epoch.to_string()],
                    |row| {
                        Ok(TargetRootBindingV1 {
                            binding_version: row.get(0)?,
                            target_id: row.get(1)?,
                            target_epoch: canonical_generation(&row.get::<_, String>(2)?)
                                .map_err(|_| rusqlite::Error::InvalidQuery)?,
                            canonical_url: row.get(3)?,
                            normalized_account: row.get(4)?,
                            physical_root_id: row.get(5)?,
                        })
                    },
                )
                .optional(),
        )?;
        let resolved = match existing {
            Some(existing) => {
                validate_target_root_binding(&existing)?;
                if existing != *candidate {
                    return Err(ROOT_MISMATCH);
                }
                existing
            }
            None => {
                database(transaction.execute(
                    "INSERT INTO s2_lite_target_root_binding_v1(
                        binding_version, target_id, target_epoch, canonical_url,
                        normalized_account, physical_root_id
                     ) VALUES(?1, ?2, ?3, ?4, ?5, ?6)",
                    params![
                        candidate.binding_version,
                        candidate.target_id,
                        candidate.target_epoch.to_string(),
                        candidate.canonical_url,
                        candidate.normalized_account,
                        candidate.physical_root_id,
                    ],
                ))?;
                candidate.clone()
            }
        };

        // All root-scoped rows are loaded through their authoritative root
        // key. Their strict decoders reject a copied or mismatched root id.
        if let Some(bytes) = database(
            transaction
                .query_row(
                    "SELECT state_json FROM s2_lite_desktop_root_state_v1 WHERE root_id=?1",
                    [self.root_id],
                    |row| row.get::<_, Vec<u8>>(0),
                )
                .optional(),
        )? {
            let state: DesktopRootStateV1 = decode(&bytes)?;
            validate_desktop_root_state(&state, self.root_id)?;
        }
        if let Some(batch_bytes) = database(
            transaction
                .query_row(
                    "SELECT state_json FROM s2_lite_outbound_batch_v1 WHERE root_id=?1",
                    [self.root_id],
                    |row| row.get::<_, Vec<u8>>(0),
                )
                .optional(),
        )? {
            let batch: OutboundBatchV1 = decode(&batch_bytes)?;
            validate_outbound_batch(&batch, self.root_id)?;
        }
        if let Some(migration) = load_migration_from(&transaction, self.root_id)? {
            let _ = completed_migration_writer_seed(&migration, self.root_id)?;
        }
        database(transaction.commit())?;
        Ok(resolved)
    }

    /// Loads the immutable migration execution identity for this physical
    /// root.  It has no relationship to ordinary writer allocation.
    pub fn load_migration_execution_binding_v1(
        &mut self,
    ) -> Result<Option<MigrationExecutionBindingV1>> {
        let conn = self.connection()?;
        load_migration_execution_binding_from(&conn, self.root_id)
    }

    /// Claims the one execution identity for this physical root.  A retry may
    /// observe the exact same immutable row; every attempted retarget or
    /// changed snapshot input fails closed.
    pub fn bind_migration_execution_v1(
        &mut self,
        candidate: &MigrationExecutionBindingV1,
    ) -> Result<MigrationExecutionBindingV1> {
        validate_migration_execution_binding(candidate)?;
        if candidate.physical_root_id != self.root_id {
            return Err(ROOT_MISMATCH);
        }
        let mut conn = self.connection()?;
        let transaction = database(conn.transaction_with_behavior(TransactionBehavior::Immediate))?;
        ensure_root_authority(&transaction, self.root_id)?;
        validate_migration_execution_target_authority(&transaction, candidate, self.root_id)?;
        let existing = database(
            transaction
                .query_row(
                    "SELECT state_json FROM s2_lite_migration_execution_binding_v1 WHERE root_id=?1",
                    [self.root_id],
                    |row| row.get::<_, Vec<u8>>(0),
                )
                .optional(),
        )?;
        let resolved = match existing {
            Some(bytes) => {
                let binding: MigrationExecutionBindingV1 = decode(&bytes)?;
                validate_migration_execution_binding(&binding)?;
                if binding.physical_root_id != self.root_id || binding != *candidate {
                    return Err(ROOT_MISMATCH);
                }
                validate_migration_execution_target_authority(
                    &transaction,
                    &binding,
                    self.root_id,
                )?;
                binding
            }
            None => {
                database(transaction.execute(
                    "INSERT INTO s2_lite_migration_execution_binding_v1(root_id, state_json)
                     VALUES(?1, ?2)",
                    params![self.root_id, encode(candidate)?],
                ))?;
                candidate.clone()
            }
        };
        database(transaction.commit())?;
        Ok(resolved)
    }

    /// Captures the one immutable legacy bootstrap source under the same
    /// SQLite admission that records its migration identity and source guard.
    /// The callback receives the transaction's consistent business view; it
    /// has no network collaborator and cannot allocate an ordinary writer.
    pub fn admit_and_capture_migration_v1<F>(
        &mut self,
        input: &MigrationAdmissionInputV1,
        capture: F,
    ) -> Result<MigrationAdmissionResultV1>
    where
        F: FnOnce(&Connection) -> Result<(i64, CapturedLegacySnapshotV1)>,
    {
        if input.target_binding.physical_root_id != self.root_id {
            return Err(ROOT_MISMATCH);
        }
        let mut conn = self.connection()?;
        let transaction = database(conn.transaction_with_behavior(TransactionBehavior::Immediate))?;
        database(install_migration_source_guard_triggers(&transaction))?;
        if let Some(owner) = load_migration_source_owner(&transaction)? {
            if owner.root_id != self.root_id || owner.migration_id != input.migration_id {
                return Err(ROOT_MISMATCH);
            }
            let existing_state =
                load_migration_from(&transaction, self.root_id)?.ok_or(STORE_CORRUPTION)?;
            let existing_binding = transaction
                .query_row(
                    "SELECT state_json FROM s2_lite_migration_execution_binding_v1 WHERE root_id=?1",
                    [self.root_id],
                    |row| row.get::<_, Vec<u8>>(0),
                )
                .optional()
                .map_err(|_| STORE_FAILURE)?
                .ok_or(STORE_CORRUPTION)?;
            let execution_binding: MigrationExecutionBindingV1 = decode(&existing_binding)?;
            validate_migration_execution_binding(&execution_binding)?;
            validate_migration_execution_target_authority(
                &transaction,
                &execution_binding,
                self.root_id,
            )?;
            if execution_binding.target_id != input.target_binding.target_id
                || execution_binding.target_epoch != input.target_binding.target_epoch
                || execution_binding.physical_root_id != self.root_id
                || existing_state.migration_id != execution_binding.migration_id
                || existing_state
                    .snapshot
                    .as_ref()
                    .map(|snapshot| snapshot.legacy_fingerprint.as_str())
                    != execution_binding.legacy_fingerprint.as_deref()
            {
                return Err(STORE_CORRUPTION);
            }
            database(transaction.commit())?;
            return Ok(MigrationAdmissionResultV1 {
                execution_binding,
                state: existing_state,
                attached_existing: true,
            });
        }

        validate_active_migration_binding(&transaction, &input.target_binding, self.root_id)?;
        let safety = load_root_safety_from(&transaction, self.root_id)?.ok_or(ROOT_MISMATCH)?;
        if !safety.root_fatal_signals.is_empty()
            || decide_legacy_put_v1(&recover_activation_cutover_v1(
                &load_discovery_state_from(&transaction, self.root_id)?
                    .map(|state| state.state)
                    .unwrap_or_else(create_discovery_state_v1),
                Some(&safety.cutover_state),
            )) != LegacyPutDecisionV1::AllowedS2NotActivated
        {
            return Err(ROOT_MISMATCH);
        }

        if transaction
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM s2_lite_migration_execution_binding_v1 WHERE root_id=?1)",
                [self.root_id],
                |row| row.get::<_, i64>(0),
            )
            .map_err(|_| STORE_FAILURE)?
            != 0
            || load_migration_source_owner(&transaction)?.is_some()
        {
            return Err(STORE_CORRUPTION);
        }
        let (captured_records_generation, snapshot) = capture(&transaction)?;
        if captured_records_generation < 0 || snapshot.snapshot_version != 1 {
            return Err(STORE_CORRUPTION);
        }
        // `reconcile` recomputes this exact fingerprint from the frozen
        // canonical entity representation, preventing a caller supplied hash.
        let initial = create_migration_state_v1(
            &input.migration_id,
            self.root_id,
            &input.migration_writer_id,
            &input.created_at,
            "legacy-bootstrap",
        )?;
        let captured = retain_captured_snapshot_v1(&initial, &snapshot)?;
        let state = plan_captured_migration_v1(&captured)?;
        let state = reconcile_migration_state_v1(&state)?;
        let execution_binding = MigrationExecutionBindingV1 {
            binding_version: 1,
            physical_root_id: self.root_id.to_string(),
            target_id: input.target_binding.target_id.clone(),
            target_epoch: input.target_binding.target_epoch,
            captured_records_generation,
            legacy_fingerprint: Some(snapshot.legacy_fingerprint.clone()),
            migration_id: input.migration_id.clone(),
        };
        validate_migration_execution_binding(&execution_binding)?;
        insert_migration(&transaction, &state)?;
        database(transaction.execute(
            "INSERT INTO s2_lite_migration_execution_binding_v1(root_id, state_json)
             VALUES(?1, ?2)",
            params![self.root_id, encode(&execution_binding)?],
        ))?;
        database(transaction.execute(
            "INSERT INTO s2_lite_migration_source_guard_v1(
                 root_id, migration_id, captured_records_generation
             ) VALUES(?1, ?2, ?3)",
            params![
                self.root_id,
                execution_binding.migration_id,
                execution_binding.captured_records_generation.to_string(),
            ],
        ))?;
        database(transaction.execute(
            "INSERT INTO s2_lite_migration_source_owner_v1(owner_key, root_id, migration_id)
             VALUES(1, ?1, ?2)",
            params![self.root_id, execution_binding.migration_id],
        ))?;
        database(transaction.commit())?;
        Ok(MigrationAdmissionResultV1 {
            execution_binding,
            state,
            attached_existing: false,
        })
    }

    /// Coordinator-facing admission variant.  The input factory is evaluated
    /// only after this `BEGIN IMMEDIATE` transaction has proved that no
    /// durable migration owner or execution row exists for the root.  A retry
    /// or concurrent loser therefore attaches to the immutable winner without
    /// manufacturing replacement migration or writer identities.
    pub fn admit_and_capture_migration_with_input_factory_v1<F, G>(
        &mut self,
        target_binding: &TargetRootBindingV1,
        legacy_ticket: Option<&LegacyRouteTicketV1>,
        capture: F,
        create_input: G,
    ) -> Result<LegacyTicketMigrationAdmissionV1>
    where
        F: FnOnce(&Connection) -> Result<(i64, CapturedLegacySnapshotV1)>,
        G: FnOnce() -> MigrationAdmissionInputV1,
    {
        if target_binding.physical_root_id != self.root_id {
            return Err(ROOT_MISMATCH);
        }
        let mut conn = self.connection()?;
        let transaction = database(conn.transaction_with_behavior(TransactionBehavior::Immediate))?;
        database(install_migration_source_guard_triggers(&transaction))?;
        if let Some(ticket) = legacy_ticket {
            let ticket_outcome = validate_legacy_route_ticket_v1(&transaction, ticket)?;
            if ticket_outcome != LegacyRouteTicketValidationV1::Valid {
                database(transaction.commit())?;
                return Ok(LegacyTicketMigrationAdmissionV1::TicketInvalid(
                    ticket_outcome,
                ));
            }
        }
        if let Some(owner) = load_migration_source_owner(&transaction)? {
            if owner.root_id != self.root_id {
                return Err(ROOT_MISMATCH);
            }
            let state = load_migration_from(&transaction, self.root_id)?.ok_or(STORE_CORRUPTION)?;
            let execution = load_migration_execution_binding_from(&transaction, self.root_id)?
                .ok_or(STORE_CORRUPTION)?;
            validate_migration_execution_target_authority(&transaction, &execution, self.root_id)?;
            if owner.migration_id != execution.migration_id
                || execution.target_id != target_binding.target_id
                || execution.target_epoch != target_binding.target_epoch
                || execution.physical_root_id != target_binding.physical_root_id
                || state.migration_id != execution.migration_id
                || state
                    .snapshot
                    .as_ref()
                    .map(|snapshot| snapshot.legacy_fingerprint.as_str())
                    != execution.legacy_fingerprint.as_deref()
            {
                return Err(STORE_CORRUPTION);
            }
            database(transaction.commit())?;
            return Ok(LegacyTicketMigrationAdmissionV1::Admitted(Box::new(
                MigrationAdmissionResultV1 {
                    execution_binding: execution,
                    state,
                    attached_existing: true,
                },
            )));
        }

        validate_active_migration_binding(&transaction, target_binding, self.root_id)?;
        let safety = load_root_safety_from(&transaction, self.root_id)?.ok_or(ROOT_MISMATCH)?;
        if !safety.root_fatal_signals.is_empty()
            || decide_legacy_put_v1(&recover_activation_cutover_v1(
                &load_discovery_state_from(&transaction, self.root_id)?
                    .map(|state| state.state)
                    .unwrap_or_else(create_discovery_state_v1),
                Some(&safety.cutover_state),
            )) != LegacyPutDecisionV1::AllowedS2NotActivated
        {
            return Err(ROOT_MISMATCH);
        }
        if transaction
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM s2_lite_migration_execution_binding_v1 WHERE root_id=?1)",
                [self.root_id],
                |row| row.get::<_, i64>(0),
            )
            .map_err(|_| STORE_FAILURE)?
            != 0
            || load_migration_source_owner(&transaction)?.is_some()
        {
            return Err(STORE_CORRUPTION);
        }

        let input = create_input();
        if input.target_binding != *target_binding
            || input.target_binding.physical_root_id != self.root_id
        {
            return Err(ROOT_MISMATCH);
        }
        let (captured_records_generation, snapshot) = capture(&transaction)?;
        if captured_records_generation < 0 || snapshot.snapshot_version != 1 {
            return Err(STORE_CORRUPTION);
        }
        let initial = create_migration_state_v1(
            &input.migration_id,
            self.root_id,
            &input.migration_writer_id,
            &input.created_at,
            "legacy-bootstrap",
        )?;
        let captured = retain_captured_snapshot_v1(&initial, &snapshot)?;
        let state = reconcile_migration_state_v1(&plan_captured_migration_v1(&captured)?)?;
        let execution_binding = MigrationExecutionBindingV1 {
            binding_version: 1,
            physical_root_id: self.root_id.to_string(),
            target_id: input.target_binding.target_id,
            target_epoch: input.target_binding.target_epoch,
            captured_records_generation,
            legacy_fingerprint: Some(snapshot.legacy_fingerprint.clone()),
            migration_id: input.migration_id,
        };
        validate_migration_execution_binding(&execution_binding)?;
        insert_migration(&transaction, &state)?;
        database(transaction.execute(
            "INSERT INTO s2_lite_migration_execution_binding_v1(root_id, state_json)
             VALUES(?1, ?2)",
            params![self.root_id, encode(&execution_binding)?],
        ))?;
        database(transaction.execute(
            "INSERT INTO s2_lite_migration_source_guard_v1(
                 root_id, migration_id, captured_records_generation
             ) VALUES(?1, ?2, ?3)",
            params![
                self.root_id,
                execution_binding.migration_id,
                execution_binding.captured_records_generation.to_string(),
            ],
        ))?;
        database(transaction.execute(
            "INSERT INTO s2_lite_migration_source_owner_v1(owner_key, root_id, migration_id)
             VALUES(1, ?1, ?2)",
            params![self.root_id, execution_binding.migration_id],
        ))?;
        database(transaction.commit())?;
        Ok(LegacyTicketMigrationAdmissionV1::Admitted(Box::new(
            MigrationAdmissionResultV1 {
                execution_binding,
                state,
                attached_existing: false,
            },
        )))
    }

    pub fn migration_source_protected_v1(&self) -> Result<bool> {
        let conn = self.connection()?;
        Ok(load_migration_source_owner(&conn)?.is_some())
    }

    /// Recovers the activation cutover latch only after one SQLite authority
    /// transaction has proved that every independently persisted activation
    /// fact belongs to the current frozen migration.  This is deliberately
    /// local-only: the durable exact receipt is the recovery evidence, and no
    /// remote operation is performed while (or before) this transaction runs.
    pub fn recover_durable_verified_activation_cutover_v1(&mut self) -> Result<()> {
        let mut conn = self.connection()?;
        let transaction = database(conn.transaction_with_behavior(TransactionBehavior::Immediate))?;
        ensure_root_authority(&transaction, self.root_id)?;

        let mut safety =
            load_root_safety_from(&transaction, self.root_id)?.ok_or(STORE_CORRUPTION)?;
        if !safety.root_fatal_signals.is_empty()
            || !safety.cutover_state.root_fatal_signals.is_empty()
        {
            database(transaction.commit())?;
            return Ok(());
        }

        // `load_migration_from` strictly reconciles the persisted snapshot and
        // deterministic plan before it can be used as a cutover input.
        let migration = load_migration_from(&transaction, self.root_id)?.ok_or(STORE_CORRUPTION)?;
        if migration.status != MigrationStatusV1::ActivationVerified
            || migration.root_id != self.root_id
        {
            return Err(STORE_CORRUPTION);
        }
        let execution =
            load_frozen_migration_execution_from(&transaction, &migration, self.root_id)?;
        let owner = load_migration_source_owner(&transaction)?.ok_or(STORE_CORRUPTION)?;
        if owner.root_id != self.root_id
            || owner.migration_id != migration.migration_id
            || execution.migration_id != migration.migration_id
        {
            return Err(STORE_CORRUPTION);
        }

        let snapshot = migration.snapshot.as_ref().ok_or(STORE_CORRUPTION)?;
        if execution.legacy_fingerprint.as_deref() != Some(snapshot.legacy_fingerprint.as_str()) {
            return Err(STORE_CORRUPTION);
        }
        // A compatible third-party activation is adopted with an already
        // durable cutover latch but deliberately has no local publication
        // receipt.  It is not eligible for receipt-based recovery (and cannot
        // create a latch here); preserve that established, read-only path.
        if migration.activation_receipt.is_none() {
            if safety.cutover_state.remote_s2_activated
                && matches!(
                    &safety.cutover_state.fingerprint_consistency,
                    ActivationFingerprintConsistencyV1::Consistent { legacy_fingerprint }
                        if legacy_fingerprint.as_deref()
                            == Some(snapshot.legacy_fingerprint.as_str())
                )
            {
                database(transaction.commit())?;
                return Ok(());
            }
            return Err(STORE_CORRUPTION);
        }
        let embedded_intent = migration
            .activation_intent
            .as_ref()
            .ok_or(STORE_CORRUPTION)?;
        if migration.activation_intent_root_id.as_deref() != Some(self.root_id) {
            return Err(STORE_CORRUPTION);
        }
        let durable_intent =
            load_activation_intent_from(&transaction, self.root_id, &embedded_intent.remote_path)?
                .ok_or(STORE_CORRUPTION)?;
        // Equality includes immutable path, activation id, exact bytes,
        // content hash, and the prepared-intent fingerprint.
        if durable_intent != *embedded_intent {
            return Err(STORE_CORRUPTION);
        }
        let embedded_receipt = migration
            .activation_receipt
            .as_ref()
            .ok_or(STORE_CORRUPTION)?;
        if migration.activation_receipt_root_id.as_deref() != Some(self.root_id) {
            return Err(STORE_CORRUPTION);
        }
        let durable_receipt =
            load_activation_receipt_from(&transaction, self.root_id, &embedded_intent.remote_path)?
                .ok_or(STORE_CORRUPTION)?;
        // The durable receipt loader validates its root binding and every
        // receipt-to-intent identity; equality additionally rejects a merely
        // internally consistent receipt embedded in a replaced migration row.
        if durable_receipt != *embedded_receipt {
            return Err(STORE_CORRUPTION);
        }
        validate_published_activation_receipt_v1(embedded_receipt, embedded_intent)
            .map_err(|_| STORE_CORRUPTION)?;

        let mut discovery = create_discovery_state_v1();
        discovery.verified_objects.push(VerifiedRemoteObjectV1 {
            path: durable_intent.remote_path.clone(),
            kind: "activation".to_string(),
            exact_bytes_hash: durable_receipt.verified_exact_bytes_hash.clone(),
            exact_bytes_hex: String::new(),
            content_hash: durable_intent.content_hash.clone(),
            commit_ref: None,
            activation_id: Some(durable_intent.activation_id.clone()),
            fingerprint_evidence: VerifiedFingerprintEvidenceV1::Value {
                value: snapshot.legacy_fingerprint.clone(),
            },
        });
        let cutover = recover_activation_cutover_v1(&discovery, Some(&safety.cutover_state))
            .diagnostic_state()
            .ok_or(STORE_CORRUPTION)?;
        if !cutover.remote_s2_activated {
            return Err(STORE_CORRUPTION);
        }
        let merged = merge_migration_root_cutover_state_v1(&safety.cutover_state, &cutover)?;
        let mut fatal_codes = safety
            .root_fatal_signals
            .iter()
            .map(|fatal| fatal.code.clone())
            .chain(
                merged
                    .root_fatal_signals
                    .iter()
                    .map(|fatal| fatal.code.clone()),
            )
            .collect::<Vec<_>>();
        fatal_codes.sort();
        fatal_codes.dedup();
        let next_fatals = fatal_codes
            .into_iter()
            .map(|code| MigrationRootFatalV1 { code })
            .collect::<Vec<_>>();
        if safety.cutover_state != merged || safety.root_fatal_signals != next_fatals {
            safety.generation = safety.generation.checked_add(1).ok_or(STORE_CORRUPTION)?;
            safety.cutover_state = merged;
            safety.root_fatal_signals = next_fatals;
            save_root_safety(&transaction, &safety)?;
        }
        database(transaction.commit())?;
        Ok(())
    }

    /// Atomically converts a durably verified compatible activation into the
    /// normal S2 authority state. The migration state, frozen writer seed, and
    /// database-wide source-owner/guard retirement share one BEGIN IMMEDIATE
    /// transaction so none can become visible without the other two.
    pub fn finalize_verified_migration_v1(&mut self) -> Result<MigrationFinalizationResultV1> {
        self.finalize_verified_migration_with_fault_v1(None)
    }

    /// Atomically adopts compatible, durably verified remote activation into
    /// the local migration state. It deliberately stops before completion,
    /// writer installation, or source-authority retirement.
    pub fn adopt_verified_activation_v1(&mut self) -> Result<VerifiedActivationAdoptionResultV1> {
        let mut conn = self.connection()?;
        let transaction = database(conn.transaction_with_behavior(TransactionBehavior::Immediate))?;
        ensure_root_authority(&transaction, self.root_id)?;
        let safety = load_root_safety_from(&transaction, self.root_id)?.ok_or(STORE_CORRUPTION)?;
        if !safety.root_fatal_signals.is_empty()
            || !safety.cutover_state.root_fatal_signals.is_empty()
        {
            database(transaction.commit())?;
            return Ok(VerifiedActivationAdoptionResultV1::ReadOnlyFrozen);
        }
        let migration = load_migration_from(&transaction, self.root_id)?.ok_or(STORE_CORRUPTION)?;
        let execution =
            load_frozen_migration_execution_from(&transaction, &migration, self.root_id)?;
        let fingerprint = migration
            .snapshot
            .as_ref()
            .ok_or(STORE_CORRUPTION)?
            .legacy_fingerprint
            .as_str();
        if execution.legacy_fingerprint.as_deref() != Some(fingerprint)
            || !safety.cutover_state.remote_s2_activated
            || !safety
                .cutover_state
                .verified_activation_evidence
                .iter()
                .any(|evidence| evidence.legacy_fingerprint.as_deref() == Some(fingerprint))
            || !matches!(
                &safety.cutover_state.fingerprint_consistency,
                ActivationFingerprintConsistencyV1::Consistent { legacy_fingerprint }
                    if legacy_fingerprint.as_deref() == Some(fingerprint)
            )
        {
            return Err(STORE_CORRUPTION);
        }
        if migration.status == MigrationStatusV1::MigrationComplete {
            database(transaction.commit())?;
            return Ok(VerifiedActivationAdoptionResultV1::AlreadyComplete);
        }
        let owner = load_migration_source_owner(&transaction)?.ok_or(STORE_CORRUPTION)?;
        if owner.root_id != self.root_id || owner.migration_id != migration.migration_id {
            return Err(STORE_CORRUPTION);
        }
        let result = match migration.status {
            MigrationStatusV1::ActivationVerified => {
                VerifiedActivationAdoptionResultV1::AlreadyVerified
            }
            MigrationStatusV1::StageBComplete | MigrationStatusV1::ActivationPublishing => {
                let mut adopted = migration.clone();
                adopted.status = MigrationStatusV1::ActivationVerified;
                adopted.generation = adopted.generation.checked_add(1).ok_or(STORE_CORRUPTION)?;
                validate_attempt_transition(&migration, &adopted)?;
                save_migration(&transaction, &adopted)?;
                VerifiedActivationAdoptionResultV1::Adopted
            }
            _ => return Err(STORE_CORRUPTION),
        };
        database(transaction.commit())?;
        Ok(result)
    }

    #[cfg(test)]
    pub(crate) fn finalize_verified_migration_with_injected_fault_v1(
        &mut self,
        fault: MigrationFinalizationFaultV1,
    ) -> Result<MigrationFinalizationResultV1> {
        self.finalize_verified_migration_with_fault_v1(Some(fault))
    }

    fn finalize_verified_migration_with_fault_v1(
        &mut self,
        fault: Option<MigrationFinalizationFaultV1>,
    ) -> Result<MigrationFinalizationResultV1> {
        let mut conn = self.connection()?;
        let transaction = database(conn.transaction_with_behavior(TransactionBehavior::Immediate))?;
        ensure_root_authority(&transaction, self.root_id)?;
        let safety = load_root_safety_from(&transaction, self.root_id)?.ok_or(STORE_CORRUPTION)?;
        if !safety.root_fatal_signals.is_empty()
            || !safety.cutover_state.root_fatal_signals.is_empty()
        {
            database(transaction.commit())?;
            return Ok(MigrationFinalizationResultV1::ReadOnlyFrozen);
        }
        let migration = load_migration_from(&transaction, self.root_id)?.ok_or(STORE_CORRUPTION)?;
        let fingerprint = migration
            .snapshot
            .as_ref()
            .ok_or(STORE_CORRUPTION)?
            .legacy_fingerprint
            .as_str();
        if !safety.cutover_state.remote_s2_activated
            || !matches!(
                &safety.cutover_state.fingerprint_consistency,
                ActivationFingerprintConsistencyV1::Consistent { legacy_fingerprint }
                    if legacy_fingerprint.as_deref() == Some(fingerprint)
            )
        {
            return Err(STORE_CORRUPTION);
        }
        let _execution =
            load_frozen_migration_execution_from(&transaction, &migration, self.root_id)?;

        if migration.status == MigrationStatusV1::MigrationComplete {
            if load_migration_source_owner(&transaction)?.is_some() {
                return Err(STORE_CORRUPTION);
            }
            let _ =
                load_exact_completed_migration_writer_from(&transaction, &migration, self.root_id)?;
            database(transaction.commit())?;
            return Ok(MigrationFinalizationResultV1::AlreadyFinalized);
        }
        if migration.status != MigrationStatusV1::ActivationVerified {
            return Err(STORE_CORRUPTION);
        }
        let owner = load_migration_source_owner(&transaction)?.ok_or(STORE_CORRUPTION)?;
        if owner.root_id != self.root_id || owner.migration_id != migration.migration_id {
            return Err(STORE_CORRUPTION);
        }
        let mut completed = migration;
        completed.status = MigrationStatusV1::MigrationComplete;
        completed.generation = completed
            .generation
            .checked_add(1)
            .ok_or(STORE_CORRUPTION)?;
        let _ =
            completed_migration_writer_seed(&completed, self.root_id)?.ok_or(STORE_CORRUPTION)?;
        validate_verified_migration_writer_handoff_from(&transaction, &completed, self.root_id)?;
        save_migration(&transaction, &completed)?;
        if fault == Some(MigrationFinalizationFaultV1::MigrationCompletePersisted) {
            return Err(STORE_FAILURE);
        }
        initialize_desktop_writer_from(&transaction, self.root_id)?;
        if fault == Some(MigrationFinalizationFaultV1::WriterHandoffPersisted) {
            return Err(STORE_FAILURE);
        }
        database(transaction.execute(
            "DELETE FROM s2_lite_migration_source_owner_v1 WHERE owner_key=1",
            [],
        ))?;
        database(transaction.execute(
            "DELETE FROM s2_lite_migration_source_guard_v1 WHERE root_id=?1",
            [self.root_id],
        ))?;
        if fault == Some(MigrationFinalizationFaultV1::SourceRetired) {
            return Err(STORE_FAILURE);
        }
        database(transaction.commit())?;
        Ok(MigrationFinalizationResultV1::Finalized)
    }

    /// Loads the database-wide migration source owner and its immutable
    /// historical target/root execution identity.  This lookup intentionally
    /// never consults the currently active target: an in-progress migration
    /// remains bound to the root that admitted and captured it.
    pub fn load_migration_source_execution_binding_v1(
        conn: &Mutex<Connection>,
    ) -> Result<Option<MigrationExecutionBindingV1>> {
        let guard = conn.lock().map_err(|_| STORE_FAILURE)?;
        database(migrate_schema(&guard))?;
        let Some(owner) = load_migration_source_owner(&guard)? else {
            return Ok(None);
        };
        let binding = load_migration_execution_binding_from(&guard, &owner.root_id)?
            .ok_or(STORE_CORRUPTION)?;
        if binding.migration_id != owner.migration_id {
            return Err(STORE_CORRUPTION);
        }
        Ok(Some(binding))
    }

    /// Revalidates the final normal-S2 route and initializes its ordinary
    /// writer in one root-authoritative SQLite transaction.  Callers may use a
    /// preliminary route for scheduling only; it is never authority to create
    /// writer state after a later fatal or migration change.
    pub fn admit_normal_s2_route_v1(
        &mut self,
        root_id: &str,
        expected_migration_binding: Option<&MigrationExecutionBindingV1>,
    ) -> Result<NormalS2RouteAdmissionV1> {
        self.require_root(root_id)?;
        if expected_migration_binding.is_some_and(|binding| binding.physical_root_id != root_id) {
            return Err(ROOT_MISMATCH);
        }
        let mut conn = self.connection()?;
        let transaction = database(conn.transaction_with_behavior(TransactionBehavior::Immediate))?;
        ensure_root_authority(&transaction, root_id)?;
        let safety = load_root_safety_from(&transaction, root_id)?.ok_or(STORE_CORRUPTION)?;
        if !safety.root_fatal_signals.is_empty()
            || !safety.cutover_state.root_fatal_signals.is_empty()
        {
            database(transaction.commit())?;
            return Ok(NormalS2RouteAdmissionV1::ReadOnlyFrozen);
        }
        if !safety.cutover_state.remote_s2_activated {
            return Err(STORE_CORRUPTION);
        }

        let migration = load_migration_from(&transaction, root_id)?;
        let owner = load_migration_source_owner(&transaction)?;
        if let Some(expected) = expected_migration_binding {
            let current = load_migration_execution_binding_from(&transaction, root_id)?
                .ok_or(STORE_CORRUPTION)?;
            if current != *expected {
                return Err(STORE_CORRUPTION);
            }
        }
        let state = match migration {
            Some(migration) => {
                if owner.is_some() || migration.status != MigrationStatusV1::MigrationComplete {
                    return Err(STORE_CORRUPTION);
                }
                let execution = load_migration_execution_binding_from(&transaction, root_id)?
                    .ok_or(STORE_CORRUPTION)?;
                if execution.migration_id != migration.migration_id {
                    return Err(STORE_CORRUPTION);
                }
                if expected_migration_binding.is_some() {
                    load_exact_completed_migration_writer_from(&transaction, &migration, root_id)?
                } else {
                    load_continued_completed_migration_writer_from(
                        &transaction,
                        &migration,
                        root_id,
                    )?
                }
            }
            None => {
                if owner.is_some() {
                    return Err(STORE_CORRUPTION);
                }
                initialize_desktop_writer_from(&transaction, root_id)?
            }
        };
        database(transaction.commit())?;
        Ok(NormalS2RouteAdmissionV1::EnterNormalS2(state))
    }

    /// Initializes the ordinary writer once per physical root. This establishes
    /// authority only; it does not reserve a sequence or publish anything.
    pub fn initialize_desktop_writer_v1(&mut self) -> Result<DesktopRootStateV1> {
        let mut conn = self.connection()?;
        let transaction = database(conn.transaction_with_behavior(TransactionBehavior::Immediate))?;
        let state = initialize_desktop_writer_from(&transaction, self.root_id)?;
        database(transaction.commit())?;
        Ok(state)
    }

    pub fn load_desktop_root_state(&mut self) -> Result<Option<DesktopRootStateV1>> {
        let conn = self.connection()?;
        load_desktop_root_state_from(&conn, self.root_id)
    }

    pub fn load_materialized_projection(
        &mut self,
    ) -> Result<Option<DurableMaterializedProjectionV1>> {
        let conn = self.connection()?;
        let row = database(
            conn.query_row(
                "SELECT projection_generation, state_json FROM s2_lite_materialized_projection_v1 WHERE root_id=?1",
                [self.root_id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?)),
            )
            .optional(),
        )?;
        row.map(|(generation, bytes)| {
            let projection: DurableMaterializedProjectionV1 = decode(&bytes)?;
            if projection.projection_generation != canonical_generation(&generation)? {
                return Err(STORE_CORRUPTION);
            }
            validate_materialized_projection(&projection, self.root_id)?;
            Ok(projection)
        })
        .transpose()
    }

    /// CAS persists only a replay made against the still-current discovery and
    /// root-safety generations. A stale complete cache can never authorize a
    /// later outbound freeze.
    pub fn compare_and_swap_materialized_projection(
        &mut self,
        expected_projection_generation: Option<u64>,
        projection: &DurableMaterializedProjectionV1,
    ) -> Result<bool> {
        validate_materialized_projection(projection, self.root_id)?;
        let mut conn = self.connection()?;
        let transaction = database(conn.transaction_with_behavior(TransactionBehavior::Immediate))?;
        ensure_root_authority(&transaction, self.root_id)?;
        let safety = load_root_safety_from(&transaction, self.root_id)?.ok_or(STORE_CORRUPTION)?;
        if safety.generation != projection.source_root_safety_generation {
            return Ok(false);
        }
        let discovery = database(
            transaction
                .query_row(
                    "SELECT storage_generation FROM s2_lite_migration_discovery_v1 WHERE root_id=?1",
                    [self.root_id],
                    |row| row.get::<_, String>(0),
                )
                .optional(),
        )?
        .map(|value| canonical_generation(&value))
        .transpose()?;
        if discovery != Some(projection.source_discovery_generation) {
            return Ok(false);
        }
        let current = database(
            transaction
                .query_row(
                    "SELECT projection_generation FROM s2_lite_materialized_projection_v1 WHERE root_id=?1",
                    [self.root_id],
                    |row| row.get::<_, String>(0),
                )
                .optional(),
        )?
        .map(|value| canonical_generation(&value))
        .transpose()?;
        if current != expected_projection_generation {
            return Ok(false);
        }
        database(transaction.execute(
            "INSERT INTO s2_lite_materialized_projection_v1(root_id, projection_generation, state_json)
             VALUES(?1, ?2, ?3) ON CONFLICT(root_id) DO UPDATE SET
             projection_generation=excluded.projection_generation, state_json=excluded.state_json",
            params![self.root_id, projection.projection_generation.to_string(), encode(projection)?],
        ))?;
        database(transaction.commit())?;
        Ok(true)
    }

    pub fn persist_desktop_root_state(&mut self, state: &DesktopRootStateV1) -> Result<()> {
        validate_desktop_root_state(state, self.root_id)?;
        let mut conn = self.connection()?;
        let transaction = database(conn.transaction_with_behavior(TransactionBehavior::Immediate))?;
        ensure_root_authority(&transaction, self.root_id)?;
        database(transaction.execute(
            "INSERT INTO s2_lite_desktop_root_state_v1(root_id, state_json) VALUES(?1, ?2)
             ON CONFLICT(root_id) DO UPDATE SET state_json=excluded.state_json",
            params![self.root_id, encode(state)?],
        ))?;
        database(transaction.commit())?;
        Ok(())
    }

    /// Advances only the materialized-projection bookkeeping owned by the
    /// lifecycle. The root state is read *after* acquiring SQLite's immediate
    /// write admission and written with its exact prior bytes as a CAS guard,
    /// so a stale lifecycle snapshot can never overwrite writer authority
    /// advanced by receipt completion.
    pub fn update_materialized_projection_generation(
        &mut self,
        projection_generation: u64,
    ) -> Result<()> {
        let mut conn = self.connection()?;
        let transaction = database(conn.transaction_with_behavior(TransactionBehavior::Immediate))?;
        ensure_root_authority(&transaction, self.root_id)?;

        let projection_row = database(transaction.query_row(
            "SELECT projection_generation, state_json
             FROM s2_lite_materialized_projection_v1 WHERE root_id=?1",
            [self.root_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?)),
        ))?;
        let (stored_generation, projection_bytes) = projection_row;
        let projection: DurableMaterializedProjectionV1 = decode(&projection_bytes)?;
        if canonical_generation(&stored_generation)? != projection_generation
            || projection.projection_generation != projection_generation
        {
            return Err(STORE_CORRUPTION);
        }
        validate_materialized_projection(&projection, self.root_id)?;

        let root_bytes = database(transaction.query_row(
            "SELECT state_json FROM s2_lite_desktop_root_state_v1 WHERE root_id=?1",
            [self.root_id],
            |row| row.get::<_, Vec<u8>>(0),
        ))?;
        let mut root_state: DesktopRootStateV1 = decode(&root_bytes)?;
        validate_desktop_root_state(&root_state, self.root_id)?;
        root_state.materialized_projection_generation = Some(projection_generation);
        validate_desktop_root_state(&root_state, self.root_id)?;
        let changed = database(transaction.execute(
            "UPDATE s2_lite_desktop_root_state_v1 SET state_json=?2
             WHERE root_id=?1 AND state_json=?3",
            params![self.root_id, encode(&root_state)?, root_bytes],
        ))?;
        if changed != 1 {
            return Err(STORE_CORRUPTION);
        }
        database(transaction.commit())?;
        Ok(())
    }

    /// Runs the local business projector under the same root-bound immediate
    /// transaction that validates its exact durable projection inputs. Advancing
    /// `businessAppliedProjectionGeneration` means fully processed locally; it
    /// never asserts equality between business tables and canonical replay.
    pub(crate) fn run_business_projection_transaction<T>(
        &mut self,
        expected_projection_generation: u64,
        apply: impl FnOnce(&Connection, &DurableMaterializedProjectionV1) -> Result<T>,
    ) -> Result<(BusinessProjectionTransactionResultV1, Option<T>)> {
        let mut conn = self.connection()?;
        let transaction = database(conn.transaction_with_behavior(TransactionBehavior::Immediate))?;
        ensure_root_authority(&transaction, self.root_id)?;
        let safety = load_root_safety_from(&transaction, self.root_id)?.ok_or(STORE_CORRUPTION)?;
        if !safety.root_fatal_signals.is_empty() {
            return Err(ROOT_MISMATCH);
        }
        let discovery_generation = database(transaction.query_row(
            "SELECT storage_generation FROM s2_lite_migration_discovery_v1 WHERE root_id=?1",
            [self.root_id],
            |row| row.get::<_, String>(0),
        ))?;
        let projection_row = database(transaction.query_row(
            "SELECT projection_generation, state_json FROM s2_lite_materialized_projection_v1 WHERE root_id=?1",
            [self.root_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?)),
        ))?;
        let (stored_generation, bytes) = projection_row;
        let mut projection: DurableMaterializedProjectionV1 = decode(&bytes)?;
        if canonical_generation(&stored_generation)? != expected_projection_generation
            || projection.projection_generation != expected_projection_generation
            || projection.source_discovery_generation
                != canonical_generation(&discovery_generation)?
            || projection.source_root_safety_generation != safety.generation
        {
            return Err(STORE_CORRUPTION);
        }
        validate_materialized_projection(&projection, self.root_id)?;
        if !matches!(
            projection.state.status,
            super::materialized_projection::MaterializedProjectionStatusV1::Complete
        ) {
            return Err(STORE_CORRUPTION);
        }
        let root_bytes = database(transaction.query_row(
            "SELECT state_json FROM s2_lite_desktop_root_state_v1 WHERE root_id=?1",
            [self.root_id],
            |row| row.get::<_, Vec<u8>>(0),
        ))?;
        let mut root_state: DesktopRootStateV1 = decode(&root_bytes)?;
        validate_desktop_root_state(&root_state, self.root_id)?;
        if root_state.materialized_projection_generation != Some(expected_projection_generation) {
            return Err(STORE_CORRUPTION);
        }
        if root_state.business_applied_projection_generation == Some(expected_projection_generation)
        {
            database(transaction.commit())?;
            return Ok((BusinessProjectionTransactionResultV1::AlreadyApplied, None));
        }
        let output = apply(&transaction, &projection)?;
        root_state.business_applied_projection_generation = Some(expected_projection_generation);
        validate_desktop_root_state(&root_state, self.root_id)?;
        projection.business_projection_applied_generation = Some(expected_projection_generation);
        validate_materialized_projection(&projection, self.root_id)?;
        database(transaction.execute(
            "UPDATE s2_lite_desktop_root_state_v1 SET state_json=?2 WHERE root_id=?1",
            params![self.root_id, encode(&root_state)?],
        ))?;
        database(transaction.execute(
            "UPDATE s2_lite_materialized_projection_v1 SET state_json=?2 WHERE root_id=?1 AND projection_generation=?3",
            params![self.root_id, encode(&projection)?, expected_projection_generation.to_string()],
        ))?;
        database(transaction.commit())?;
        Ok((BusinessProjectionTransactionResultV1::Applied, Some(output)))
    }

    /// Resolves one previously overlay-preserved entity under the current,
    /// already-applied projection.  The callback owns the entity write,
    /// staging removal, and blocker clear; any failure rolls all three back.
    pub(crate) fn run_entity_projection_resolution_transaction<T>(
        &mut self,
        expected_projection_generation: u64,
        resolve: impl FnOnce(&Connection, &DurableMaterializedProjectionV1) -> Result<T>,
    ) -> Result<T> {
        let mut conn = self.connection()?;
        let transaction = database(conn.transaction_with_behavior(TransactionBehavior::Immediate))?;
        ensure_root_authority(&transaction, self.root_id)?;
        let safety = load_root_safety_from(&transaction, self.root_id)?.ok_or(STORE_CORRUPTION)?;
        if !safety.root_fatal_signals.is_empty() {
            return Err(ROOT_MISMATCH);
        }
        let discovery_generation = database(transaction.query_row(
            "SELECT storage_generation FROM s2_lite_migration_discovery_v1 WHERE root_id=?1",
            [self.root_id],
            |row| row.get::<_, String>(0),
        ))?;
        let (stored_generation, projection_bytes) = database(transaction.query_row(
            "SELECT projection_generation, state_json FROM s2_lite_materialized_projection_v1 WHERE root_id=?1",
            [self.root_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?)),
        ))?;
        let projection: DurableMaterializedProjectionV1 = decode(&projection_bytes)?;
        if canonical_generation(&stored_generation)? != expected_projection_generation
            || projection.projection_generation != expected_projection_generation
            || projection.source_discovery_generation
                != canonical_generation(&discovery_generation)?
            || projection.source_root_safety_generation != safety.generation
            || projection.business_projection_applied_generation
                != Some(expected_projection_generation)
        {
            return Err(STORE_CORRUPTION);
        }
        validate_materialized_projection(&projection, self.root_id)?;
        if !matches!(
            projection.state.status,
            super::materialized_projection::MaterializedProjectionStatusV1::Complete
        ) {
            return Err(STORE_CORRUPTION);
        }
        let root_bytes = database(transaction.query_row(
            "SELECT state_json FROM s2_lite_desktop_root_state_v1 WHERE root_id=?1",
            [self.root_id],
            |row| row.get::<_, Vec<u8>>(0),
        ))?;
        let root_state: DesktopRootStateV1 = decode(&root_bytes)?;
        validate_desktop_root_state(&root_state, self.root_id)?;
        if root_state.materialized_projection_generation != Some(expected_projection_generation)
            || root_state.business_applied_projection_generation
                != Some(expected_projection_generation)
        {
            return Err(STORE_CORRUPTION);
        }
        let output = resolve(&transaction, &projection)?;
        database(transaction.commit())?;
        Ok(output)
    }

    pub fn load_unfinished_outbound_batch(&mut self) -> Result<Option<OutboundBatchV1>> {
        let conn = self.connection()?;
        let bytes = database(
            conn.query_row(
                "SELECT state_json FROM s2_lite_outbound_batch_v1 WHERE root_id=?1",
                [self.root_id],
                |row| row.get::<_, Vec<u8>>(0),
            )
            .optional(),
        )?;
        bytes
            .map(|bytes| {
                let batch: OutboundBatchV1 = decode(&bytes)?;
                validate_outbound_batch(&batch, self.root_id)?;
                if batch.bookkeeping_completed {
                    return Ok(None);
                }
                Ok(Some(batch))
            })
            .transpose()
            .map(Option::flatten)
    }

    /// Binds a local batch and its immutable bytes in one SQLite transaction.
    /// Neither record is externally publishable until this returns successfully.
    pub fn persist_outbound_batch_with_intent(
        &mut self,
        batch: &OutboundBatchV1,
        intent: &PreparedIntentV1,
    ) -> Result<()> {
        validate_outbound_batch(batch, self.root_id)?;
        validate_prepared_intent_v1(intent)?;
        if batch.prepared_intent_path != intent.remote_path
            || batch.prepared_intent_fingerprint != intent.intent_fingerprint
            || batch.writer_id != intent.commit_ref.writer_id
            || batch.writer_sequence.to_string() != intent.commit_ref.writer_seq
        {
            return Err(STORE_CORRUPTION);
        }
        let frozen_commit = decode_frozen_wire_commit_v1(&intent.exact_bytes)?;
        if batch.previous_writer_ref != frozen_commit.previous_writer_commit {
            return Err(STORE_CORRUPTION);
        }
        let mut metadata = intent.clone();
        let exact_bytes = std::mem::take(&mut metadata.exact_bytes);
        let mut conn = self.connection()?;
        let transaction = database(conn.transaction_with_behavior(TransactionBehavior::Immediate))?;
        ensure_root_authority(&transaction, self.root_id)?;
        let safety = load_root_safety_from(&transaction, self.root_id)?.ok_or(STORE_CORRUPTION)?;
        if !safety.root_fatal_signals.is_empty() {
            return Err(ProtocolError("ROOT_FROZEN"));
        }
        let existing = database(
            transaction
                .query_row(
                    "SELECT state_json FROM s2_lite_outbound_batch_v1 WHERE root_id=?1",
                    [self.root_id],
                    |row| row.get::<_, Vec<u8>>(0),
                )
                .optional(),
        )?;
        if let Some(bytes) = existing {
            let existing: OutboundBatchV1 = decode(&bytes)?;
            validate_outbound_batch(&existing, self.root_id)?;
            if !existing.bookkeeping_completed {
                return Err(STORE_CORRUPTION);
            }
        }
        persist_intent_parts(
            &transaction,
            self.root_id,
            "commit",
            &intent.remote_path,
            &intent.intent_fingerprint,
            &metadata,
            &exact_bytes,
        )?;
        database(transaction.execute(
            "INSERT INTO s2_lite_outbound_batch_v1(root_id, batch_id, intent_path, intent_fingerprint, state_json)
             VALUES(?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(root_id) DO UPDATE SET
               batch_id=excluded.batch_id,
               intent_path=excluded.intent_path,
               intent_fingerprint=excluded.intent_fingerprint,
               state_json=excluded.state_json",
            params![
                self.root_id,
                batch.batch_id,
                batch.prepared_intent_path,
                batch.prepared_intent_fingerprint,
                encode(batch)?
            ],
        ))?;
        database(transaction.commit())?;
        Ok(())
    }

    /// Captures every authority input and durably reserves one outbound commit
    /// in a single `BEGIN IMMEDIATE` transaction.  The callback has no store
    /// handle and therefore cannot publish or alter unrelated durable state.
    #[allow(dead_code)] // Frozen ordinary publication authority; execution deferred to I6.5.
    pub(crate) fn run_outbound_freeze_transaction(
        &mut self,
        target_id: &str,
        target_epoch: u64,
        build: impl FnOnce(
            &OutboundFreezeTransactionContextV1,
        ) -> Result<OutboundFreezeTransactionPlanV1>,
    ) -> Result<OutboundFreezeTransactionResultV1> {
        self.run_outbound_freeze_transaction_inner(target_id, target_epoch, false, false, build)
    }

    fn run_outbound_freeze_transaction_inner(
        &mut self,
        target_id: &str,
        target_epoch: u64,
        fault_after_writer_reservation: bool,
        fault_after_batch_persistence: bool,
        build: impl FnOnce(
            &OutboundFreezeTransactionContextV1,
        ) -> Result<OutboundFreezeTransactionPlanV1>,
    ) -> Result<OutboundFreezeTransactionResultV1> {
        let mut conn = self.connection()?;
        let transaction = database(conn.transaction_with_behavior(TransactionBehavior::Immediate))?;

        // Re-derive the active target while the authoritative write lock is
        // held.  The physical root is never a caller-selected value.
        let registry = crate::sync_targets::registry(&transaction).map_err(|_| STORE_FAILURE)?;
        let Some(registry) = registry else {
            return Ok(OutboundFreezeTransactionResultV1::TargetChanged);
        };
        if registry.active_target_id.as_deref() != Some(target_id)
            || registry.target_epoch != target_epoch
        {
            return Ok(OutboundFreezeTransactionResultV1::TargetChanged);
        }
        let Some(target) = registry
            .targets
            .iter()
            .find(|target| target.id == target_id)
        else {
            return Ok(OutboundFreezeTransactionResultV1::TargetChanged);
        };
        let root = super::webdav_adapter::webdav_root_v1(&target.normalized_url, &target.username)
            .map_err(|_| STORE_CORRUPTION)?;
        let candidate = TargetRootBindingV1 {
            binding_version: 1,
            target_id: target_id.to_string(),
            target_epoch,
            canonical_url: root.canonical_url,
            normalized_account: root.normalized_account,
            physical_root_id: root.physical_root_id,
        };
        if candidate.physical_root_id != self.root_id {
            return Ok(OutboundFreezeTransactionResultV1::TargetChanged);
        }
        let binding = database(
            transaction
                .query_row(
                    "SELECT binding_version, target_id, target_epoch, canonical_url,
                            normalized_account, physical_root_id
                     FROM s2_lite_target_root_binding_v1
                     WHERE target_id=?1 AND target_epoch=?2",
                    params![target_id, target_epoch.to_string()],
                    |row| {
                        Ok(TargetRootBindingV1 {
                            binding_version: row.get(0)?,
                            target_id: row.get(1)?,
                            target_epoch: row
                                .get::<_, String>(2)?
                                .parse()
                                .map_err(|_| rusqlite::Error::InvalidQuery)?,
                            canonical_url: row.get(3)?,
                            normalized_account: row.get(4)?,
                            physical_root_id: row.get(5)?,
                        })
                    },
                )
                .optional(),
        )?;
        let Some(binding) = binding else {
            return Ok(OutboundFreezeTransactionResultV1::Blocked);
        };
        validate_target_root_binding(&binding)?;
        if binding != candidate {
            return Ok(OutboundFreezeTransactionResultV1::Blocked);
        }
        ensure_root_authority(&transaction, self.root_id)?;
        let safety = load_root_safety_from(&transaction, self.root_id)?.ok_or(STORE_CORRUPTION)?;
        if !safety.root_fatal_signals.is_empty() {
            return Ok(OutboundFreezeTransactionResultV1::Blocked);
        }
        let discovery_generation = database(
            transaction
                .query_row(
                    "SELECT storage_generation FROM s2_lite_migration_discovery_v1 WHERE root_id=?1",
                    [self.root_id],
                    |row| row.get::<_, String>(0),
                )
                .optional(),
        )?
        .map(|value| canonical_generation(&value))
        .transpose()?
        .ok_or(STORE_CORRUPTION)?;
        let projection_row = database(
            transaction
                .query_row(
                    "SELECT projection_generation, state_json FROM s2_lite_materialized_projection_v1 WHERE root_id=?1",
                    [self.root_id],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?)),
                )
                .optional(),
        )?;
        let Some((stored_projection_generation, projection_bytes)) = projection_row else {
            return Ok(OutboundFreezeTransactionResultV1::Blocked);
        };
        let projection: DurableMaterializedProjectionV1 = decode(&projection_bytes)?;
        if projection.projection_generation != canonical_generation(&stored_projection_generation)?
        {
            return Err(STORE_CORRUPTION);
        }
        validate_materialized_projection(&projection, self.root_id)?;
        if !matches!(
            projection.state.status,
            super::materialized_projection::MaterializedProjectionStatusV1::Complete
        ) || projection.source_discovery_generation != discovery_generation
            || projection.source_root_safety_generation != safety.generation
        {
            return Ok(OutboundFreezeTransactionResultV1::Blocked);
        }
        let root_bytes = database(
            transaction
                .query_row(
                    "SELECT state_json FROM s2_lite_desktop_root_state_v1 WHERE root_id=?1",
                    [self.root_id],
                    |row| row.get::<_, Vec<u8>>(0),
                )
                .optional(),
        )?;
        let Some(root_bytes) = root_bytes else {
            return Ok(OutboundFreezeTransactionResultV1::Blocked);
        };
        let root_state: DesktopRootStateV1 = decode(&root_bytes)?;
        validate_desktop_root_state(&root_state, self.root_id)?;
        if root_state.materialized_projection_generation != Some(projection.projection_generation)
            || root_state.business_applied_projection_generation
                != Some(projection.projection_generation)
        {
            return Ok(OutboundFreezeTransactionResultV1::Blocked);
        }
        let existing_batch = database(
            transaction
                .query_row(
                    "SELECT state_json FROM s2_lite_outbound_batch_v1 WHERE root_id=?1",
                    [self.root_id],
                    |row| row.get::<_, Vec<u8>>(0),
                )
                .optional(),
        )?;
        if let Some(bytes) = existing_batch {
            let batch: OutboundBatchV1 = decode(&bytes)?;
            validate_outbound_batch(&batch, self.root_id)?;
            if !batch.bookkeeping_completed {
                return Ok(OutboundFreezeTransactionResultV1::ExistingPendingOutbound);
            }
        }
        let unresolved_intent = database(transaction.query_row(
            "SELECT EXISTS(
                SELECT 1 FROM s2_lite_prepared_intent_v1 AS intent
                LEFT JOIN s2_lite_published_receipt_v1 AS receipt
                  ON receipt.root_id=intent.root_id
                 AND receipt.receipt_kind=intent.intent_kind
                 AND receipt.remote_path=intent.remote_path
                WHERE intent.root_id=?1 AND intent.intent_kind='commit'
                  AND receipt.remote_path IS NULL
             )",
            [self.root_id],
            |row| row.get::<_, i64>(0),
        ))?;
        if unresolved_intent != 0 {
            return Ok(OutboundFreezeTransactionResultV1::ExistingPendingOutbound);
        }
        let staging = crate::sync_staging::get_staging(&transaction).map_err(|_| STORE_FAILURE)?;
        let context = OutboundFreezeTransactionContextV1 {
            binding,
            root_state: root_state.clone(),
            projection: projection.clone(),
            discovery_generation,
            root_safety_generation: safety.generation,
            staging,
        };
        let plan = build(&context)?;
        let OutboundFreezeTransactionPlanV1::Frozen { batch, intent } = plan else {
            database(transaction.commit())?;
            return Ok(match plan {
                OutboundFreezeTransactionPlanV1::NoSemanticMutation => {
                    OutboundFreezeTransactionResultV1::NoSemanticMutation
                }
                OutboundFreezeTransactionPlanV1::Blocked => {
                    OutboundFreezeTransactionResultV1::Blocked
                }
                OutboundFreezeTransactionPlanV1::BlockedStaleEntityBases => {
                    OutboundFreezeTransactionResultV1::BlockedStaleEntityBases
                }
                OutboundFreezeTransactionPlanV1::Frozen { .. } => unreachable!(),
            });
        };
        validate_outbound_batch(&batch, self.root_id)?;
        validate_prepared_intent_v1(&intent)?;
        if batch.target_id != context.binding.target_id
            || batch.target_epoch != context.binding.target_epoch
            || batch.physical_root_id != context.binding.physical_root_id
            || batch.projection_generation != context.projection.projection_generation
            || batch.source_discovery_generation != context.discovery_generation
            || batch.source_root_safety_generation != context.root_safety_generation
            || batch.writer_id != root_state.local_writer_id
            || batch.writer_sequence != root_state.next_writer_sequence
            || batch.commit_ref != intent.commit_ref
            || batch.prepared_intent_path != intent.remote_path
            || batch.prepared_intent_fingerprint != intent.intent_fingerprint
        {
            return Err(STORE_CORRUPTION);
        }
        let frozen = decode_frozen_wire_commit_v1(&intent.exact_bytes)?;
        if frozen.commit_ref() != batch.commit_ref
            || frozen.previous_writer_commit != batch.previous_writer_ref
        {
            return Err(STORE_CORRUPTION);
        }
        let mut reserved_state = root_state;
        reserved_state.next_writer_sequence = reserved_state
            .next_writer_sequence
            .checked_add(1)
            .ok_or(STORE_CORRUPTION)?;
        validate_desktop_root_state(&reserved_state, self.root_id)?;
        database(transaction.execute(
            "UPDATE s2_lite_desktop_root_state_v1 SET state_json=?2 WHERE root_id=?1",
            params![self.root_id, encode(&reserved_state)?],
        ))?;
        if fault_after_writer_reservation {
            return Err(STORE_FAILURE);
        }
        database(transaction.execute(
            "INSERT INTO s2_lite_outbound_batch_v1(root_id, batch_id, intent_path, intent_fingerprint, state_json)
             VALUES(?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(root_id) DO UPDATE SET
               batch_id=excluded.batch_id,
               intent_path=excluded.intent_path,
               intent_fingerprint=excluded.intent_fingerprint,
               state_json=excluded.state_json",
            params![
                self.root_id,
                batch.batch_id,
                batch.prepared_intent_path,
                batch.prepared_intent_fingerprint,
                encode(&batch)?
            ],
        ))?;
        if fault_after_batch_persistence {
            return Err(STORE_FAILURE);
        }
        let mut metadata = intent.clone();
        let exact_bytes = std::mem::take(&mut metadata.exact_bytes);
        persist_intent_parts(
            &transaction,
            self.root_id,
            "commit",
            &intent.remote_path,
            &intent.intent_fingerprint,
            &metadata,
            &exact_bytes,
        )?;
        database(transaction.commit())?;
        Ok(OutboundFreezeTransactionResultV1::Frozen { batch, intent })
    }

    pub fn mark_outbound_batch_bookkeeping_complete(
        &mut self,
        batch: &OutboundBatchV1,
    ) -> Result<()> {
        validate_outbound_batch(batch, self.root_id)?;
        let mut completed = batch.clone();
        completed.bookkeeping_completed = true;
        completed.bookkeeping_generation = completed
            .bookkeeping_generation
            .checked_add(1)
            .ok_or(STORE_CORRUPTION)?;
        let mut conn = self.connection()?;
        let transaction = database(conn.transaction_with_behavior(TransactionBehavior::Immediate))?;
        let changed = database(transaction.execute(
            "UPDATE s2_lite_outbound_batch_v1 SET state_json=?3
             WHERE root_id=?1 AND batch_id=?2 AND state_json=?4",
            params![
                self.root_id,
                batch.batch_id,
                encode(&completed)?,
                encode(batch)?
            ],
        ))?;
        if changed > 1 {
            return Err(STORE_CORRUPTION);
        }
        database(transaction.commit())?;
        Ok(())
    }

    /// Atomically acknowledges only the staging tokens captured by one exact
    /// receipted batch, advances its writer head, and marks its bookkeeping
    /// complete.  It has no remote collaborator: receipt persistence is the
    /// sole publication authority admitted here.
    pub fn complete_verified_outbound_batch(&mut self) -> Result<OutboundCompletionResultV1> {
        self.complete_verified_outbound_batch_inner(false, false)
    }

    fn complete_verified_outbound_batch_inner(
        &mut self,
        fault_after_staging_acknowledgement: bool,
        fault_after_writer_head_advance: bool,
    ) -> Result<OutboundCompletionResultV1> {
        let mut conn = self.connection()?;
        let transaction = database(conn.transaction_with_behavior(TransactionBehavior::Immediate))?;
        ensure_root_authority(&transaction, self.root_id)?;
        let batch_bytes = database(
            transaction
                .query_row(
                    "SELECT state_json FROM s2_lite_outbound_batch_v1 WHERE root_id=?1",
                    [self.root_id],
                    |row| row.get::<_, Vec<u8>>(0),
                )
                .optional(),
        )?;
        let Some(batch_bytes) = batch_bytes else {
            database(transaction.commit())?;
            return Ok(OutboundCompletionResultV1::AlreadyCompleted);
        };
        let batch: OutboundBatchV1 = decode(&batch_bytes)?;
        validate_outbound_batch(&batch, self.root_id)?;

        let binding = database(
            transaction
                .query_row(
                    "SELECT binding_version, target_id, target_epoch, canonical_url,
                            normalized_account, physical_root_id
                     FROM s2_lite_target_root_binding_v1
                     WHERE target_id=?1 AND target_epoch=?2",
                    params![batch.target_id, batch.target_epoch.to_string()],
                    |row| {
                        Ok(TargetRootBindingV1 {
                            binding_version: row.get(0)?,
                            target_id: row.get(1)?,
                            target_epoch: canonical_generation(&row.get::<_, String>(2)?)
                                .map_err(|_| rusqlite::Error::InvalidQuery)?,
                            canonical_url: row.get(3)?,
                            normalized_account: row.get(4)?,
                            physical_root_id: row.get(5)?,
                        })
                    },
                )
                .optional(),
        )?
        .ok_or(STORE_CORRUPTION)?;
        validate_target_root_binding(&binding)?;
        if binding.target_id != batch.target_id
            || binding.target_epoch != batch.target_epoch
            || binding.physical_root_id != batch.physical_root_id
            || binding.physical_root_id != self.root_id
        {
            return Err(ROOT_MISMATCH);
        }

        let intent =
            load_commit_intent_from(&transaction, self.root_id, &batch.prepared_intent_path)?
                .ok_or(STORE_CORRUPTION)?;
        if intent.remote_path != batch.prepared_intent_path
            || intent.intent_fingerprint != batch.prepared_intent_fingerprint
            || intent.commit_ref != batch.commit_ref
            || intent.commit_ref.writer_id != batch.writer_id
        {
            return Err(STORE_CORRUPTION);
        }
        let frozen = decode_frozen_wire_commit_v1(&intent.exact_bytes)?;
        if frozen.commit_ref() != batch.commit_ref
            || frozen.previous_writer_commit != batch.previous_writer_ref
            || frozen.basis_clock != batch.basis_clock
            || frozen.mutations.len() != batch.mutations.len()
            || batch.mutations.iter().any(|captured| {
                !frozen.mutations.iter().any(|mutation| {
                    mutation.local_mutation_id == captured.local_mutation_id
                        && mutation.entity_type == captured.entity_kind
                        && mutation.entity_key == captured.entity_key
                })
            })
        {
            return Err(STORE_CORRUPTION);
        }
        let receipt = load_commit_receipt_from(&transaction, self.root_id, &intent.remote_path)?;
        let Some(receipt) = receipt else {
            database(transaction.commit())?;
            return Ok(OutboundCompletionResultV1::PendingReceipt);
        };
        if receipt.remote_path != intent.remote_path
            || receipt.content_hash != intent.content_hash
            || receipt.commit_ref != batch.commit_ref
            || receipt.prepared_intent_fingerprint != intent.intent_fingerprint
        {
            return Err(STORE_CORRUPTION);
        }
        if batch.bookkeeping_completed {
            database(transaction.commit())?;
            return Ok(OutboundCompletionResultV1::AlreadyCompleted);
        }

        // Completion belongs to the immutable target/epoch/root captured by
        // the batch.  A later active-target switch cannot invalidate that
        // authority or redirect acknowledgement to its new staging key.
        let registry = crate::sync_targets::registry(&transaction).map_err(|_| STORE_FAILURE)?;
        let Some(registry) = registry else {
            return Err(STORE_CORRUPTION);
        };
        if !registry
            .targets
            .iter()
            .any(|target| target.id == batch.target_id)
        {
            return Err(STORE_CORRUPTION);
        }
        let root_state_bytes = database(
            transaction
                .query_row(
                    "SELECT state_json FROM s2_lite_desktop_root_state_v1 WHERE root_id=?1",
                    [self.root_id],
                    |row| row.get::<_, Vec<u8>>(0),
                )
                .optional(),
        )?
        .ok_or(STORE_CORRUPTION)?;
        let mut root_state: DesktopRootStateV1 = decode(&root_state_bytes)?;
        validate_desktop_root_state(&root_state, self.root_id)?;
        let reserved_sequence = batch
            .writer_sequence
            .checked_add(1)
            .ok_or(STORE_CORRUPTION)?;
        if root_state.local_writer_id != batch.writer_id
            || root_state.next_writer_sequence != reserved_sequence
            || root_state.writer_head != batch.previous_writer_ref
        {
            return Err(STORE_CORRUPTION);
        }

        let mut staging =
            crate::sync_staging::get_staging_for_target(&transaction, &batch.target_id)
                .map_err(|_| STORE_FAILURE)?
                .ok_or(STORE_CORRUPTION)?;
        let mut captured_keys = Vec::new();
        let mut remove_indices = Vec::new();
        for mutation in &batch.mutations {
            if captured_keys
                .iter()
                .any(|value: &Value| value == &mutation.entity_key)
            {
                return Err(STORE_CORRUPTION);
            }
            captured_keys.push(mutation.entity_key.clone());
            let mut matching = Vec::new();
            for (index, entry) in staging.entries.iter().enumerate() {
                let key = crate::sync_staging::staged_entry_entity_key(entry)
                    .map_err(|_| STORE_CORRUPTION)?;
                if key == mutation.entity_key {
                    matching.push((index, entry.last_generation));
                }
            }
            if matching.len() > 1 {
                return Err(STORE_CORRUPTION);
            }
            if matching
                .first()
                .is_some_and(|(_, generation)| *generation == mutation.captured_last_generation)
            {
                remove_indices.push(matching[0].0);
            }
        }
        remove_indices.sort_unstable();
        remove_indices.dedup();
        for index in remove_indices.into_iter().rev() {
            staging.entries.remove(index);
        }
        crate::sync_staging::set_staging_for_target(&transaction, &batch.target_id, &staging)
            .map_err(|_| STORE_FAILURE)?;
        if cfg!(test) && fault_after_staging_acknowledgement {
            return Err(STORE_FAILURE);
        }

        root_state.writer_head = Some(batch.commit_ref.clone());
        validate_desktop_root_state(&root_state, self.root_id)?;
        database(transaction.execute(
            "UPDATE s2_lite_desktop_root_state_v1 SET state_json=?2 WHERE root_id=?1",
            params![self.root_id, encode(&root_state)?],
        ))?;
        if cfg!(test) && fault_after_writer_head_advance {
            return Err(STORE_FAILURE);
        }

        let mut completed = batch.clone();
        completed.bookkeeping_completed = true;
        completed.bookkeeping_generation = completed
            .bookkeeping_generation
            .checked_add(1)
            .ok_or(STORE_CORRUPTION)?;
        database(transaction.execute(
            "UPDATE s2_lite_outbound_batch_v1 SET state_json=?3
             WHERE root_id=?1 AND batch_id=?2 AND state_json=?4",
            params![
                self.root_id,
                batch.batch_id,
                encode(&completed)?,
                batch_bytes,
            ],
        ))?;
        database(transaction.commit())?;
        Ok(OutboundCompletionResultV1::Completed)
    }

    fn connection(&self) -> Result<MutexGuard<'_, Connection>> {
        self.conn.lock().map_err(|_| STORE_FAILURE)
    }

    fn require_root(&self, root_id: &str) -> Result<()> {
        if root_id != self.root_id {
            return Err(ROOT_MISMATCH);
        }
        Ok(())
    }

    pub fn load_discovery_state(&self) -> Result<Option<VersionedDiscoveryStateV1>> {
        let conn = self.connection()?;
        load_discovery_state_from(&conn, self.root_id)
    }

    pub fn compare_and_swap_discovery_state(
        &mut self,
        expected_generation: Option<u64>,
        state: &DiscoveryStateV1,
    ) -> Result<bool> {
        if state.state_version != 1 {
            return Err(STORE_CORRUPTION);
        }
        let mut conn = self.connection()?;
        let transaction = database(conn.transaction_with_behavior(TransactionBehavior::Immediate))?;
        ensure_root_authority(&transaction, self.root_id)?;
        let current = database(
            transaction
                .query_row(
                    "SELECT storage_generation FROM s2_lite_migration_discovery_v1 WHERE root_id=?1",
                    [self.root_id],
                    |row| row.get::<_, String>(0),
                )
                .optional(),
        )?
        .map(|value| canonical_generation(&value))
        .transpose()?;
        if current != expected_generation {
            return Ok(false);
        }
        let next = current.map_or(Ok(0), |value| value.checked_add(1).ok_or(STORE_CORRUPTION))?;
        database(transaction.execute(
            "INSERT INTO s2_lite_migration_discovery_v1(root_id, storage_generation, state_json)
             VALUES(?1, ?2, ?3) ON CONFLICT(root_id) DO UPDATE SET
             storage_generation=excluded.storage_generation, state_json=excluded.state_json",
            params![self.root_id, next.to_string(), encode(state)?],
        ))?;
        merge_discovery_fatals_into_root_authority(&transaction, self.root_id, state)?;
        database(transaction.commit())?;
        Ok(true)
    }

    /// Atomically records a verified remote activation that is incompatible
    /// with this root's immutable migration fingerprint.  The discovery row,
    /// target/root-bound migration identity, cutover evidence, root fatal, and
    /// any diagnostic migration fatal mirror are all checked or updated while
    /// one `BEGIN IMMEDIATE` transaction is held.
    ///
    /// This is intentionally separate from the compatible cutover merge: an
    /// incompatible activation must never leave a committed observation that
    /// admits ordinary publication before its fatal barrier is visible.
    pub fn persist_incompatible_activation_freeze_v1(
        &mut self,
        root_id: &str,
        discovery: &VersionedDiscoveryStateV1,
    ) -> Result<()> {
        self.persist_incompatible_activation_freeze_inner_v1(
            root_id,
            discovery,
            #[cfg(test)]
            None,
        )
    }

    fn persist_incompatible_activation_freeze_inner_v1(
        &mut self,
        root_id: &str,
        discovery: &VersionedDiscoveryStateV1,
        #[cfg(test)] fault: Option<IncompatibleActivationFreezeFaultV1>,
    ) -> Result<()> {
        self.require_root(root_id)?;
        let mut conn = self.connection()?;
        let transaction = database(conn.transaction_with_behavior(TransactionBehavior::Immediate))?;
        ensure_root_authority(&transaction, root_id)?;

        // The caller may have observed an obsolete discovery generation before
        // waiting for this writer lock.  Requiring byte-for-byte state equality
        // with the persisted generation makes the exact verified evidence an
        // authority input rather than a stale diagnostic snapshot.
        let current_discovery =
            load_discovery_state_from(&transaction, root_id)?.ok_or(STORE_CORRUPTION)?;
        if current_discovery != *discovery {
            return Err(STORE_CORRUPTION);
        }
        let binding =
            load_migration_execution_binding_from(&transaction, root_id)?.ok_or(ROOT_MISMATCH)?;
        let mut safety = load_root_safety_from(&transaction, root_id)?.ok_or(STORE_CORRUPTION)?;
        let cutover =
            recover_activation_cutover_v1(&current_discovery.state, Some(&safety.cutover_state))
                .diagnostic_state()
                .ok_or(ProtocolError("activation_cutover_recovery_not_ready"))?;
        let compatible = matches!(
            &cutover.fingerprint_consistency,
            ActivationFingerprintConsistencyV1::Consistent { legacy_fingerprint }
                if *legacy_fingerprint == binding.legacy_fingerprint
        );
        if !cutover.remote_s2_activated || compatible {
            return Err(STORE_CORRUPTION);
        }
        let merged = merge_migration_root_cutover_state_v1(&safety.cutover_state, &cutover)?;
        #[cfg(test)]
        if fault == Some(IncompatibleActivationFreezeFaultV1::AfterCutoverMerge) {
            return Err(ProtocolError(
                "INJECTED_INCOMPATIBLE_ACTIVATION_FREEZE_FAILURE",
            ));
        }

        let mut fatal_codes = safety
            .root_fatal_signals
            .iter()
            .map(|fatal| fatal.code.clone())
            .chain(
                merged
                    .root_fatal_signals
                    .iter()
                    .map(|fatal| fatal.code.clone()),
            )
            .collect::<Vec<_>>();
        fatal_codes.push("SYNC_ROOT_FROZEN_LEGACY_CHANGE".to_string());
        fatal_codes.sort();
        fatal_codes.dedup();
        let next_fatals = fatal_codes
            .into_iter()
            .map(|code| MigrationRootFatalV1 { code })
            .collect::<Vec<_>>();
        if safety.cutover_state != merged || safety.root_fatal_signals != next_fatals {
            safety.generation = safety.generation.checked_add(1).ok_or(STORE_CORRUPTION)?;
            safety.cutover_state = merged;
            safety.root_fatal_signals = next_fatals;
            save_root_safety(&transaction, &safety)?;
        }
        #[cfg(test)]
        if fault == Some(IncompatibleActivationFreezeFaultV1::AfterRootFatalWrite) {
            return Err(ProtocolError(
                "INJECTED_INCOMPATIBLE_ACTIVATION_FREEZE_FAILURE",
            ));
        }

        if let Some(mut migration) = load_migration_from(&transaction, root_id)? {
            let mut changed = false;
            for fatal in &safety.root_fatal_signals {
                changed |= add_fatal(&mut migration, &fatal.code)?;
            }
            if changed {
                save_migration(&transaction, &migration)?;
            }
        }
        #[cfg(test)]
        if fault == Some(IncompatibleActivationFreezeFaultV1::BeforeCommit) {
            return Err(ProtocolError(
                "INJECTED_INCOMPATIBLE_ACTIVATION_FREEZE_FAILURE",
            ));
        }
        database(transaction.commit())?;
        Ok(())
    }

    pub fn compare_and_swap_root_safety(
        &mut self,
        expected_generation: u64,
        requested: &MigrationRootSafetyStateV1,
    ) -> Result<bool> {
        self.require_root(&requested.root_id)?;
        if requested.state_version != 1
            || requested.generation != expected_generation.checked_add(1).ok_or(STORE_CORRUPTION)?
        {
            return Err(STORE_CORRUPTION);
        }
        let mut conn = self.connection()?;
        let transaction = database(conn.transaction_with_behavior(TransactionBehavior::Immediate))?;
        ensure_root_authority(&transaction, self.root_id)?;
        let current = load_root_safety_from(&transaction, self.root_id)?.ok_or(STORE_CORRUPTION)?;
        if current.generation != expected_generation {
            return Ok(false);
        }
        let merged_cutover = merge_migration_root_cutover_state_v1(
            &current.cutover_state,
            &requested.cutover_state,
        )?;
        if merged_cutover != requested.cutover_state
            || current.root_fatal_signals.iter().any(|fatal| {
                !requested
                    .root_fatal_signals
                    .iter()
                    .any(|next| next.code == fatal.code)
            })
            || !strictly_sorted_unique(
                requested
                    .root_fatal_signals
                    .iter()
                    .map(|fatal| fatal.code.as_str()),
            )
            || requested
                .cutover_state
                .root_fatal_signals
                .iter()
                .any(|fatal| {
                    !requested
                        .root_fatal_signals
                        .iter()
                        .any(|root_fatal| root_fatal.code == fatal.code)
                })
        {
            return Err(STORE_CORRUPTION);
        }
        save_root_safety(&transaction, requested)?;
        if let Some(mut migration) = load_migration_from(&transaction, self.root_id)? {
            let mut changed = false;
            for fatal in &requested.root_fatal_signals {
                changed |= add_fatal(&mut migration, &fatal.code)?;
            }
            if changed {
                save_migration(&transaction, &migration)?;
            }
        }
        database(transaction.commit())?;
        Ok(true)
    }

    pub fn load_prepared_intent(&self, remote_path: &str) -> Result<Option<PreparedIntentV1>> {
        let conn = self.connection()?;
        load_commit_intent_from(&conn, self.root_id, remote_path)
    }

    /// Enumerates the durable commit work that must be recovered before a
    /// lifecycle is allowed to prepare a successor ordinary publication.
    pub fn list_prepared_unreceipted_intents(&self) -> Result<Vec<PreparedIntentV1>> {
        let conn = self.connection()?;
        let mut statement = database(conn.prepare(
            "SELECT remote_path FROM s2_lite_prepared_intent_v1 AS intent
             WHERE intent.root_id=?1 AND intent.intent_kind='commit'
               AND NOT EXISTS (
                 SELECT 1 FROM s2_lite_published_receipt_v1 AS receipt
                 WHERE receipt.root_id=intent.root_id
                   AND receipt.receipt_kind='commit'
                   AND receipt.remote_path=intent.remote_path
               )
             ORDER BY remote_path",
        ))?;
        let rows = database(statement.query_map([self.root_id], |row| row.get::<_, String>(0)))?;
        let paths = database(rows.collect::<std::result::Result<Vec<_>, _>>())?;
        paths
            .into_iter()
            .map(|path| {
                load_commit_intent_from(&conn, self.root_id, &path)?.ok_or(STORE_CORRUPTION)
            })
            .collect()
    }

    pub fn load_published_receipt(
        &self,
        remote_path: &str,
    ) -> Result<Option<RemotePublishedReceiptV1>> {
        let conn = self.connection()?;
        load_commit_receipt_from(&conn, self.root_id, remote_path)
    }

    pub fn load_prepared_activation_intent(
        &self,
        remote_path: &str,
    ) -> Result<Option<PreparedActivationIntentV1>> {
        let conn = self.connection()?;
        load_activation_intent_from(&conn, self.root_id, remote_path)
    }

    pub fn load_published_activation_receipt(
        &self,
        remote_path: &str,
    ) -> Result<Option<PublishedActivationReceiptV1>> {
        let conn = self.connection()?;
        load_activation_receipt_from(&conn, self.root_id, remote_path)
    }

    /// Performs a fresh exact GET against this store's physical root. Only an
    /// exact match can create and persist the private trusted receipt.
    ///
    /// ```compile_fail
    /// use app_lib::s2_lite::durable_persistence::SqliteS2LiteStoreV1;
    /// use app_lib::s2_lite::immutable_publish::RemotePublishedReceiptV1;
    /// fn resign(store: &mut SqliteS2LiteStoreV1<'_>, loaded: RemotePublishedReceiptV1) {
    ///     store.persist_root_bound_receipt(&loaded).unwrap();
    /// }
    /// ```
    pub fn verify_and_persist_commit_receipt<R: ImmutableObjectRemoteV1>(
        &mut self,
        intent: &PreparedIntentV1,
        remote: &mut R,
        verified_at_diagnostic: &str,
    ) -> Result<RecoverPreparedIntentResultV1> {
        if remote.physical_root_id() != Some(self.root_id) {
            return Err(ROOT_MISMATCH);
        }
        let durable = self
            .load_prepared_intent(&intent.remote_path)?
            .ok_or(STORE_CORRUPTION)?;
        if durable != *intent {
            return Err(STORE_CORRUPTION);
        }
        let result = restart_durable_publish_v1(intent, None, remote, verified_at_diagnostic)?;
        if let RecoverPreparedIntentResultV1::AlreadyPublishedExact(receipt) = &result {
            self.persist_root_bound_receipt(&RootBoundPublishedReceiptV1 {
                physical_root_id: self.root_id.to_string(),
                receipt: receipt.clone(),
            })?;
        }
        Ok(result)
    }

    /// Activation equivalent of `verify_and_persist_commit_receipt`.
    ///
    /// ```compile_fail
    /// use app_lib::s2_lite::durable_persistence::SqliteS2LiteStoreV1;
    /// use app_lib::s2_lite::immutable_publish::PublishedActivationReceiptV1;
    /// fn resign(store: &mut SqliteS2LiteStoreV1<'_>, loaded: PublishedActivationReceiptV1) {
    ///     store.persist_root_bound_activation_receipt(&loaded).unwrap();
    /// }
    /// ```
    pub fn verify_and_persist_activation_receipt<R: ImmutableObjectRemoteV1>(
        &mut self,
        intent: &PreparedActivationIntentV1,
        remote: &mut R,
        verified_at_diagnostic: &str,
    ) -> Result<RecoverActivationIntentResultV1> {
        if remote.physical_root_id() != Some(self.root_id) {
            return Err(ROOT_MISMATCH);
        }
        let durable = self
            .load_prepared_activation_intent(&intent.remote_path)?
            .ok_or(STORE_CORRUPTION)?;
        if durable != *intent {
            return Err(STORE_CORRUPTION);
        }
        let result =
            restart_durable_activation_publish_v1(intent, None, remote, verified_at_diagnostic)?;
        if let RecoverActivationIntentResultV1::AlreadyPublishedExact(receipt) = &result {
            self.persist_root_bound_activation_receipt(&RootBoundPublishedActivationReceiptV1 {
                physical_root_id: self.root_id.to_string(),
                receipt: receipt.clone(),
            })?;
        }
        Ok(result)
    }

    fn persist_root_bound_receipt(&mut self, bound: &RootBoundPublishedReceiptV1) -> Result<()> {
        if bound.physical_root_id != self.root_id {
            return Err(ROOT_MISMATCH);
        }
        let receipt = &bound.receipt;
        let mut conn = self.connection()?;
        let transaction = database(conn.transaction_with_behavior(TransactionBehavior::Immediate))?;
        let intent = load_commit_intent_from(&transaction, self.root_id, &receipt.remote_path)?
            .ok_or(STORE_CORRUPTION)?;
        validate_published_receipt_v1(receipt, &intent)?;
        let persisted = PersistedPublishedReceiptV1 {
            physical_root_id: self.root_id.to_string(),
            receipt: receipt.clone(),
        };
        database(transaction.execute(
            "INSERT INTO s2_lite_published_receipt_v1(
                root_id, receipt_kind, remote_path, receipt_json
             ) VALUES(?1, 'commit', ?2, ?3)
             ON CONFLICT(root_id, receipt_kind, remote_path) DO NOTHING",
            params![self.root_id, receipt.remote_path, encode(&persisted)?],
        ))?;
        let retained = load_commit_receipt_from(&transaction, self.root_id, &receipt.remote_path)?
            .ok_or(STORE_FAILURE)?;
        if retained != *receipt {
            return Err(STORE_CORRUPTION);
        }
        database(transaction.commit())?;
        Ok(())
    }

    fn persist_root_bound_activation_receipt(
        &mut self,
        bound: &RootBoundPublishedActivationReceiptV1,
    ) -> Result<()> {
        if bound.physical_root_id != self.root_id {
            return Err(ROOT_MISMATCH);
        }
        let receipt = &bound.receipt;
        let mut conn = self.connection()?;
        let transaction = database(conn.transaction_with_behavior(TransactionBehavior::Immediate))?;
        let intent = load_activation_intent_from(&transaction, self.root_id, &receipt.remote_path)?
            .ok_or(STORE_CORRUPTION)?;
        validate_published_activation_receipt_v1(receipt, &intent)?;
        let persisted = PersistedPublishedReceiptV1 {
            physical_root_id: self.root_id.to_string(),
            receipt: receipt.clone(),
        };
        database(transaction.execute(
            "INSERT INTO s2_lite_published_receipt_v1(
                root_id, receipt_kind, remote_path, receipt_json
             ) VALUES(?1, 'activation', ?2, ?3)
             ON CONFLICT(root_id, receipt_kind, remote_path) DO NOTHING",
            params![self.root_id, receipt.remote_path, encode(&persisted)?],
        ))?;
        let retained =
            load_activation_receipt_from(&transaction, self.root_id, &receipt.remote_path)?
                .ok_or(STORE_FAILURE)?;
        if retained != *receipt {
            return Err(STORE_CORRUPTION);
        }
        database(transaction.commit())?;
        Ok(())
    }

    /// Common root authority primitive for both migration and ordinary S2
    /// writes. Callers supply only their narrow admission validation; this
    /// method owns BEGIN IMMEDIATE, root binding, and authoritative freeze
    /// handling, and invokes the one admitted network operation before the
    /// transaction is released.
    fn run_root_publication_admission<T, R, FF, FA, F>(
        &mut self,
        root_id: &str,
        on_frozen: FF,
        admission: FA,
        operation: F,
    ) -> Result<std::result::Result<T, R>>
    where
        FF: FnOnce(&Connection, &MigrationRootSafetyStateV1) -> Result<R>,
        FA: FnOnce(&Connection, &MigrationRootSafetyStateV1) -> Result<Option<R>>,
        F: FnOnce() -> Result<T>,
    {
        self.require_root(root_id)?;
        let mut conn = self.connection()?;
        let transaction = database(conn.transaction_with_behavior(TransactionBehavior::Immediate))?;
        ensure_root_authority(&transaction, root_id)?;
        let safety = load_root_safety_from(&transaction, root_id)?.ok_or(STORE_CORRUPTION)?;
        if !safety.root_fatal_signals.is_empty() {
            let rejected = on_frozen(&transaction, &safety)?;
            database(transaction.commit())?;
            return Ok(Err(rejected));
        }
        if let Some(rejected) = admission(&transaction, &safety)? {
            database(transaction.commit())?;
            return Ok(Err(rejected));
        }
        let value = operation()?;
        database(transaction.commit())?;
        Ok(Ok(value))
    }

    /// Admits one ordinary S2 attempt only after its exact durable prepared
    /// intent has been found under this physical root. Batch/lifecycle callers
    /// retain their additional binding checks; this wrapper never fabricates a
    /// migration state or upgrades a transport success to a receipt.
    pub fn run_ordinary_publish_exclusive<T, F: FnOnce() -> Result<T>>(
        &mut self,
        root_id: &str,
        intent: &PreparedIntentV1,
        operation: F,
    ) -> Result<OrdinaryPublishExclusiveResultV1<T>> {
        validate_prepared_intent_v1(intent)?;
        let remote_path = intent.remote_path.clone();
        let fingerprint = intent.intent_fingerprint.clone();
        let exact_bytes = intent.exact_bytes.clone();
        match self.run_root_publication_admission(
            root_id,
            |_transaction, _safety| Ok(()),
            move |transaction, safety| {
                if safety.cutover_state.state_version != 1 {
                    return Err(STORE_CORRUPTION);
                }
                let durable = load_commit_intent_from(transaction, root_id, &remote_path)?
                    .ok_or(STORE_CORRUPTION)?;
                if durable.intent_fingerprint != fingerprint || durable.exact_bytes != exact_bytes {
                    return Err(STORE_CORRUPTION);
                }
                Ok(None::<()>)
            },
            operation,
        )? {
            Ok(value) => Ok(OrdinaryPublishExclusiveResultV1::Executed(value)),
            Err(()) => Ok(OrdinaryPublishExclusiveResultV1::RejectedRootFrozen),
        }
    }

    /// Holds the root's authoritative `BEGIN IMMEDIATE` admission through one
    /// legacy S1 PUT. Both activation and root-fatal state are checked from
    /// the same transaction that admits the supplied operation.
    pub fn run_legacy_bound_put_v1<T, F: FnOnce() -> Result<T>>(
        &mut self,
        ticket: &LegacyRouteTicketV1,
        operation: F,
    ) -> Result<LegacyS1PublishAdmissionV1<T>> {
        self.require_root(&ticket.physical_root_id)?;
        match self.run_root_publication_admission(
            &ticket.physical_root_id,
            |_conn, _safety| Ok(LegacyS1PublicationRejectionV1::RootFrozen),
            |transaction, _safety| {
                if validate_legacy_route_ticket_v1(transaction, ticket)?
                    != LegacyRouteTicketValidationV1::Valid
                {
                    return Ok(Some(LegacyS1PublicationRejectionV1::Activation));
                }
                // A listed activation awaiting exact validation is ambiguous.
                if load_discovery_state_from(transaction, &ticket.physical_root_id)?.is_some_and(
                    |discovery| {
                        discovery.state.observed_activations.iter().any(|path| {
                            !discovery
                                .state
                                .verified_objects
                                .iter()
                                .any(|object| object.path == *path)
                        })
                    },
                ) {
                    return Ok(Some(LegacyS1PublicationRejectionV1::Activation));
                }
                Ok(None)
            },
            operation,
        )? {
            Ok(value) => Ok(LegacyS1PublishAdmissionV1::Executed(value)),
            Err(LegacyS1PublicationRejectionV1::RootFrozen) => {
                Ok(LegacyS1PublishAdmissionV1::RejectedRootFrozen)
            }
            Err(_) => Ok(LegacyS1PublishAdmissionV1::RejectedActivation),
        }
    }

    pub fn run_legacy_s1_publish_exclusive<T, F: FnOnce() -> Result<T>>(
        &mut self,
        root_id: &str,
        operation: F,
    ) -> Result<LegacyS1PublishAdmissionV1<T>> {
        match self.run_root_publication_admission(
            root_id,
            |_transaction, _safety| Ok(LegacyS1PublicationRejectionV1::RootFrozen),
            |transaction, safety| {
                if load_migration_source_owner(transaction)?.is_some() {
                    return Ok(Some(
                        LegacyS1PublicationRejectionV1::MigrationSourceProtected,
                    ));
                }
                let discovery = load_discovery_state_from(transaction, root_id)?
                    .map(|value| value.state)
                    .unwrap_or_else(create_discovery_state_v1);
                if decide_legacy_put_v1(&recover_activation_cutover_v1(
                    &discovery,
                    Some(&safety.cutover_state),
                )) != LegacyPutDecisionV1::AllowedS2NotActivated
                {
                    return Ok(Some(LegacyS1PublicationRejectionV1::Activation));
                }
                Ok(None::<LegacyS1PublicationRejectionV1>)
            },
            operation,
        )? {
            Ok(value) => Ok(LegacyS1PublishAdmissionV1::Executed(value)),
            Err(LegacyS1PublicationRejectionV1::RootFrozen) => {
                Ok(LegacyS1PublishAdmissionV1::RejectedRootFrozen)
            }
            Err(LegacyS1PublicationRejectionV1::Activation) => {
                Ok(LegacyS1PublishAdmissionV1::RejectedActivation)
            }
            Err(LegacyS1PublicationRejectionV1::MigrationSourceProtected) => {
                Ok(LegacyS1PublishAdmissionV1::RejectedMigrationSourceProtected)
            }
        }
    }
}

fn load_discovery_state_from(
    conn: &Connection,
    root_id: &str,
) -> Result<Option<VersionedDiscoveryStateV1>> {
    let row = database(
        conn.query_row(
            "SELECT storage_generation, state_json FROM s2_lite_migration_discovery_v1
             WHERE root_id=?1",
            [root_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?)),
        )
        .optional(),
    )?;
    let Some((generation, bytes)) = row else {
        return Ok(None);
    };
    let state: DiscoveryStateV1 = decode(&bytes)?;
    if state.state_version != 1 {
        return Err(STORE_CORRUPTION);
    }
    Ok(Some(VersionedDiscoveryStateV1 {
        storage_generation: canonical_generation(&generation)?,
        state,
    }))
}

fn load_intent_parts(
    conn: &Connection,
    root_id: &str,
    kind: &str,
    remote_path: &str,
) -> Result<Option<IntentParts>> {
    database(
        conn.query_row(
            "SELECT intent_fingerprint, metadata_json, exact_bytes
             FROM s2_lite_prepared_intent_v1
             WHERE root_id=?1 AND intent_kind=?2 AND remote_path=?3",
            params![root_id, kind, remote_path],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional(),
    )
}

type IntentParts = (String, Vec<u8>, Vec<u8>);

fn load_commit_intent_from(
    conn: &Connection,
    root_id: &str,
    remote_path: &str,
) -> Result<Option<PreparedIntentV1>> {
    let Some((fingerprint, metadata, exact_bytes)) =
        load_intent_parts(conn, root_id, "commit", remote_path)?
    else {
        return Ok(None);
    };
    let mut intent: PreparedIntentV1 = decode(&metadata)?;
    if !intent.exact_bytes.is_empty() {
        return Err(STORE_CORRUPTION);
    }
    intent.exact_bytes = exact_bytes;
    if intent.remote_path != remote_path
        || intent.intent_fingerprint != fingerprint
        || validate_prepared_intent_v1(&intent).is_err()
    {
        return Err(STORE_CORRUPTION);
    }
    Ok(Some(intent))
}

fn load_activation_intent_from(
    conn: &Connection,
    root_id: &str,
    remote_path: &str,
) -> Result<Option<PreparedActivationIntentV1>> {
    let Some((fingerprint, metadata, exact_bytes)) =
        load_intent_parts(conn, root_id, "activation", remote_path)?
    else {
        return Ok(None);
    };
    let mut intent: PreparedActivationIntentV1 = decode(&metadata)?;
    if !intent.exact_bytes.is_empty() {
        return Err(STORE_CORRUPTION);
    }
    intent.exact_bytes = exact_bytes;
    if intent.remote_path != remote_path
        || intent.intent_fingerprint != fingerprint
        || validate_prepared_activation_intent_v1(&intent).is_err()
    {
        return Err(STORE_CORRUPTION);
    }
    Ok(Some(intent))
}

fn persist_intent_parts<T: Serialize>(
    conn: &Connection,
    root_id: &str,
    kind: &str,
    remote_path: &str,
    fingerprint: &str,
    metadata: &T,
    exact_bytes: &[u8],
) -> Result<()> {
    ensure_root_authority(conn, root_id)?;
    database(conn.execute(
        "INSERT INTO s2_lite_prepared_intent_v1(
            root_id, intent_kind, remote_path, intent_fingerprint, metadata_json, exact_bytes
         ) VALUES(?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT(root_id, intent_kind, remote_path) DO NOTHING",
        params![
            root_id,
            kind,
            remote_path,
            fingerprint,
            encode(metadata)?,
            exact_bytes,
        ],
    ))?;
    Ok(())
}

fn load_receipt_bytes(
    conn: &Connection,
    root_id: &str,
    kind: &str,
    remote_path: &str,
) -> Result<Option<Vec<u8>>> {
    database(
        conn.query_row(
            "SELECT receipt_json FROM s2_lite_published_receipt_v1
             WHERE root_id=?1 AND receipt_kind=?2 AND remote_path=?3",
            params![root_id, kind, remote_path],
            |row| row.get(0),
        )
        .optional(),
    )
}

fn load_commit_receipt_from(
    conn: &Connection,
    root_id: &str,
    remote_path: &str,
) -> Result<Option<RemotePublishedReceiptV1>> {
    let Some(bytes) = load_receipt_bytes(conn, root_id, "commit", remote_path)? else {
        return Ok(None);
    };
    let persisted: PersistedPublishedReceiptV1<RemotePublishedReceiptV1> = decode(&bytes)?;
    if persisted.physical_root_id != root_id {
        return Err(ROOT_MISMATCH);
    }
    let receipt = persisted.receipt;
    let intent = load_commit_intent_from(conn, root_id, remote_path)?.ok_or(STORE_CORRUPTION)?;
    validate_published_receipt_v1(&receipt, &intent).map_err(|_| STORE_CORRUPTION)?;
    Ok(Some(receipt))
}

fn load_activation_receipt_from(
    conn: &Connection,
    root_id: &str,
    remote_path: &str,
) -> Result<Option<PublishedActivationReceiptV1>> {
    let Some(bytes) = load_receipt_bytes(conn, root_id, "activation", remote_path)? else {
        return Ok(None);
    };
    let persisted: PersistedPublishedReceiptV1<PublishedActivationReceiptV1> = decode(&bytes)?;
    if persisted.physical_root_id != root_id {
        return Err(ROOT_MISMATCH);
    }
    let receipt = persisted.receipt;
    let intent =
        load_activation_intent_from(conn, root_id, remote_path)?.ok_or(STORE_CORRUPTION)?;
    validate_published_activation_receipt_v1(&receipt, &intent).map_err(|_| STORE_CORRUPTION)?;
    Ok(Some(receipt))
}

impl MigrationStateStoreV1 for SqliteS2LiteStoreV1<'_> {
    fn transition_for_storage_v1(&self, state: &MigrationStateV1) -> Result<MigrationStateV1> {
        // The frozen strict store rejects noncanonical transient stages. In
        // particular StageAComplete with empty Stage B canonicalizes forward.
        reconcile_migration_state_v1(state)
    }

    fn authority_identity(&self) -> u64 {
        self.conn as *const Mutex<Connection> as usize as u64
    }

    fn claim_or_load(&mut self, candidate: &MigrationStateV1) -> Result<MigrationStateV1> {
        self.require_root(&candidate.root_id)?;
        let reconciled = reconcile_migration_state_v1(candidate)?;
        if reconciled != *candidate {
            return Err(STORE_CORRUPTION);
        }
        let mut conn = self.connection()?;
        let transaction = database(conn.transaction_with_behavior(TransactionBehavior::Immediate))?;
        ensure_root_authority(&transaction, self.root_id)?;
        if let Some(existing) = load_migration_from(&transaction, self.root_id)? {
            database(transaction.commit())?;
            return Ok(existing);
        }
        let safety = load_root_safety_from(&transaction, self.root_id)?.ok_or(STORE_CORRUPTION)?;
        let mut claimed = reconciled;
        inherit_root_fatals_on_claim(&mut claimed, &safety.root_fatal_signals)?;
        insert_migration(&transaction, &claimed)?;
        database(transaction.commit())?;
        Ok(claimed)
    }

    fn load(&mut self, root_id: &str) -> Result<Option<MigrationStateV1>> {
        self.require_root(root_id)?;
        let conn = self.connection()?;
        load_migration_from(&conn, root_id)
    }

    fn compare_and_swap(
        &mut self,
        root_id: &str,
        migration_id: &str,
        expected_generation: u64,
        next: &MigrationStateV1,
    ) -> Result<bool> {
        self.require_root(root_id)?;
        let reconciled = reconcile_migration_state_v1(next)?;
        if reconciled != *next {
            return Err(STORE_CORRUPTION);
        }
        let mut conn = self.connection()?;
        let transaction = database(conn.transaction_with_behavior(TransactionBehavior::Immediate))?;
        let mut safety = load_root_safety_from(&transaction, root_id)?.ok_or(STORE_CORRUPTION)?;
        let current = load_migration_from(&transaction, root_id)?.ok_or(ROOT_MISMATCH)?;
        if current.migration_id != migration_id || current.generation != expected_generation {
            return Ok(false);
        }
        validate_attempt_transition(&current, &reconciled)?;
        if reconciled.generation != expected_generation.checked_add(1).ok_or(STORE_CORRUPTION)? {
            return Err(STORE_CORRUPTION);
        }

        let preserves = |required: &[MigrationRootFatalV1]| {
            required.iter().all(|fatal| {
                reconciled
                    .root_fatal_signals
                    .iter()
                    .any(|next| next.code == fatal.code)
            })
        };
        if (current.status == MigrationStatusV1::RootFrozen
            && reconciled.status != MigrationStatusV1::RootFrozen)
            || !preserves(&current.root_fatal_signals)
            || !preserves(&safety.root_fatal_signals)
            || (reconciled.status == MigrationStatusV1::RootFrozen
                && reconciled.root_fatal_signals.is_empty())
            || (!reconciled.root_fatal_signals.is_empty()
                && reconciled.status != MigrationStatusV1::RootFrozen)
        {
            return Err(STORE_CORRUPTION);
        }

        let mut authoritative_codes = safety
            .root_fatal_signals
            .iter()
            .map(|fatal| fatal.code.clone())
            .chain(
                reconciled
                    .root_fatal_signals
                    .iter()
                    .map(|fatal| fatal.code.clone()),
            )
            .collect::<Vec<_>>();
        authoritative_codes.sort();
        authoritative_codes.dedup();
        let authoritative_fatals = authoritative_codes
            .into_iter()
            .map(|code| MigrationRootFatalV1 { code })
            .collect::<Vec<_>>();
        if safety.root_fatal_signals != authoritative_fatals {
            safety.generation = safety.generation.checked_add(1).ok_or(STORE_CORRUPTION)?;
            safety.root_fatal_signals = authoritative_fatals;
            save_root_safety(&transaction, &safety)?;
        }
        save_migration(&transaction, &reconciled)?;
        database(transaction.commit())?;
        Ok(true)
    }

    fn load_root_safety(&mut self, root_id: &str) -> Result<MigrationRootSafetyStateV1> {
        self.require_root(root_id)?;
        let mut conn = self.connection()?;
        let transaction = database(conn.transaction_with_behavior(TransactionBehavior::Immediate))?;
        ensure_root_authority(&transaction, root_id)?;
        let state = load_root_safety_from(&transaction, root_id)?.ok_or(STORE_CORRUPTION)?;
        database(transaction.commit())?;
        Ok(state)
    }

    fn load_cutover_state(&mut self, root_id: &str) -> Result<Option<ActivationCutoverStateV1>> {
        Ok(Some(self.load_root_safety(root_id)?.cutover_state))
    }

    fn persist_cutover_state(
        &mut self,
        root_id: &str,
        incoming: &ActivationCutoverStateV1,
    ) -> Result<()> {
        self.require_root(root_id)?;
        if incoming.state_version != 1 {
            return Err(STORE_CORRUPTION);
        }
        let mut conn = self.connection()?;
        let transaction = database(conn.transaction_with_behavior(TransactionBehavior::Immediate))?;
        ensure_root_authority(&transaction, root_id)?;
        let mut safety = load_root_safety_from(&transaction, root_id)?.ok_or(STORE_CORRUPTION)?;
        let merged = merge_migration_root_cutover_state_v1(&safety.cutover_state, incoming)?;
        let mut fatal_codes = safety
            .root_fatal_signals
            .iter()
            .map(|fatal| fatal.code.clone())
            .chain(
                merged
                    .root_fatal_signals
                    .iter()
                    .map(|fatal| fatal.code.clone()),
            )
            .collect::<Vec<_>>();
        fatal_codes.sort();
        fatal_codes.dedup();
        let next_fatals = fatal_codes
            .into_iter()
            .map(|code| MigrationRootFatalV1 { code })
            .collect::<Vec<_>>();
        if safety.cutover_state != merged || safety.root_fatal_signals != next_fatals {
            safety.generation = safety.generation.checked_add(1).ok_or(STORE_CORRUPTION)?;
            safety.cutover_state = merged;
            safety.root_fatal_signals = next_fatals;
            save_root_safety(&transaction, &safety)?;
        }
        if let Some(mut migration) = load_migration_from(&transaction, root_id)? {
            let mut changed = false;
            for fatal in &safety.root_fatal_signals {
                changed |= add_fatal(&mut migration, &fatal.code)?;
            }
            if changed {
                save_migration(&transaction, &migration)?;
            }
        }
        database(transaction.commit())?;
        Ok(())
    }

    fn persist_root_fatal(
        &mut self,
        root_id: &str,
        code: &str,
    ) -> Result<Option<MigrationStateV1>> {
        self.require_root(root_id)?;
        if code.is_empty() {
            return Err(STORE_CORRUPTION);
        }
        let mut conn = self.connection()?;
        let transaction = database(conn.transaction_with_behavior(TransactionBehavior::Immediate))?;
        ensure_root_authority(&transaction, root_id)?;
        let mut safety = load_root_safety_from(&transaction, root_id)?.ok_or(STORE_CORRUPTION)?;
        if !safety
            .root_fatal_signals
            .iter()
            .any(|fatal| fatal.code == code)
        {
            safety.generation = safety.generation.checked_add(1).ok_or(STORE_CORRUPTION)?;
            safety.root_fatal_signals.push(MigrationRootFatalV1 {
                code: code.to_string(),
            });
            safety
                .root_fatal_signals
                .sort_by(|left, right| left.code.cmp(&right.code));
            save_root_safety(&transaction, &safety)?;
        }
        let mut migration = load_migration_from(&transaction, root_id)?;
        if let Some(state) = migration.as_mut() {
            if add_fatal(state, code)? {
                save_migration(&transaction, state)?;
            }
        }
        database(transaction.commit())?;
        Ok(migration)
    }

    fn run_publish_exclusive<T, F: FnOnce() -> Result<T>>(
        &mut self,
        root_id: &str,
        migration_id: &str,
        expected_generation: u64,
        expected_execution_identity: Option<&MigrationExecutionIdentityV1>,
        operation: F,
    ) -> Result<PublishExclusiveResultV1<T>> {
        let frozen_rejection = |transaction: &Connection,
                                safety: &MigrationRootSafetyStateV1|
         -> Result<MigrationStateV1> {
            let mut migration = load_migration_from(transaction, root_id)?.ok_or(ROOT_MISMATCH)?;
            let mut changed = false;
            for fatal in &safety.root_fatal_signals {
                changed |= add_fatal(&mut migration, &fatal.code)?;
            }
            if changed {
                save_migration(transaction, &migration)?;
            }
            Ok(migration)
        };
        let admission = |transaction: &Connection,
                         safety: &MigrationRootSafetyStateV1|
         -> Result<Option<MigrationStateV1>> {
            let mut migration = load_migration_from(transaction, root_id)?.ok_or(ROOT_MISMATCH)?;
            let mut changed = false;
            for fatal in &safety.root_fatal_signals {
                changed |= add_fatal(&mut migration, &fatal.code)?;
            }
            if changed {
                save_migration(transaction, &migration)?;
            }
            if migration.migration_id != migration_id
                || migration.generation != expected_generation
                || migration.status == MigrationStatusV1::RootFrozen
                || !migration.root_fatal_signals.is_empty()
            {
                return Ok(Some(migration));
            }
            // Activation publication is admitted only from its durable
            // activation state, with the exact frozen object reloaded inside
            // this same root-authoritative transaction. Bootstrap commit
            // admission intentionally has no corresponding requirement.
            if migration.status == MigrationStatusV1::ActivationPublishing {
                // Rebuild the complete historical authority chain inside this
                // same BEGIN IMMEDIATE admission. Nothing captured by the
                // earlier production preflight is sufficient to authorize an
                // activation PUT after another connection changed the source
                // owner, guard, execution identity, or target/root binding.
                let execution =
                    load_migration_execution_binding_structural_from(transaction, root_id)?
                        .ok_or(STORE_CORRUPTION)?;
                let expected_execution_identity =
                    expected_execution_identity.ok_or(STORE_CORRUPTION)?;
                if migration_execution_identity_v1(&execution) != *expected_execution_identity {
                    return Err(ACTIVATION_EXECUTION_IDENTITY_MISMATCH);
                }
                let owner = load_migration_source_owner(transaction)?.ok_or(STORE_CORRUPTION)?;
                if owner.root_id != root_id || owner.migration_id != migration.migration_id {
                    return Err(STORE_CORRUPTION);
                }
                let expected_fingerprint = migration
                    .snapshot
                    .as_ref()
                    .ok_or(STORE_CORRUPTION)?
                    .legacy_fingerprint
                    .clone();
                if execution.physical_root_id != root_id
                    || execution.migration_id != migration.migration_id
                    || execution.legacy_fingerprint.as_deref()
                        != Some(expected_fingerprint.as_str())
                {
                    return Err(STORE_CORRUPTION);
                }
                let target = load_target_root_binding_from(
                    transaction,
                    &execution.target_id,
                    execution.target_epoch,
                )?
                .ok_or(STORE_CORRUPTION)?;
                if target.target_id != execution.target_id
                    || target.target_epoch != execution.target_epoch
                    || target.physical_root_id != execution.physical_root_id
                    || target.physical_root_id != root_id
                {
                    return Err(STORE_CORRUPTION);
                }
                let intent = migration
                    .activation_intent
                    .as_ref()
                    .ok_or(STORE_CORRUPTION)?;
                let durable =
                    load_activation_intent_from(transaction, root_id, &intent.remote_path)?
                        .ok_or(STORE_CORRUPTION)?;
                if durable != *intent {
                    return Err(STORE_CORRUPTION);
                }
                match &safety.cutover_state.fingerprint_consistency {
                    super::activation_cutover::ActivationFingerprintConsistencyV1::NoEvidence => {}
                    super::activation_cutover::ActivationFingerprintConsistencyV1::Consistent {
                        legacy_fingerprint,
                    } if legacy_fingerprint.as_ref() == Some(&expected_fingerprint) => {
                        // A verified compatible third-party activation already
                        // satisfies publication necessity. Retain the frozen
                        // local intent, but suppress this PUT.
                        return Ok(Some(migration));
                    }
                    super::activation_cutover::ActivationFingerprintConsistencyV1::Consistent {
                        ..
                    }
                    | super::activation_cutover::ActivationFingerprintConsistencyV1::Conflict => {
                        // The exact same authority transaction that would
                        // otherwise admit the PUT latches the root fatal.
                        let code = "S2_ACTIVATION_VERIFIED_FINGERPRINT_MISMATCH";
                        let mut next_safety = safety.clone();
                        if !next_safety
                            .root_fatal_signals
                            .iter()
                            .any(|fatal| fatal.code == code)
                        {
                            next_safety.generation = next_safety
                                .generation
                                .checked_add(1)
                                .ok_or(STORE_CORRUPTION)?;
                            next_safety.root_fatal_signals.push(MigrationRootFatalV1 {
                                code: code.to_string(),
                            });
                            next_safety
                                .root_fatal_signals
                                .sort_by(|left, right| left.code.cmp(&right.code));
                            save_root_safety(transaction, &next_safety)?;
                        }
                        if add_fatal(&mut migration, code)? {
                            save_migration(transaction, &migration)?;
                        }
                        return Ok(Some(migration));
                    }
                }
            }
            Ok(None)
        };
        match self.run_root_publication_admission(
            root_id,
            frozen_rejection,
            admission,
            operation,
        )? {
            Ok(value) => Ok(PublishExclusiveResultV1::Executed(value)),
            Err(migration) => Ok(PublishExclusiveResultV1::Rejected(Box::new(migration))),
        }
    }
}

impl ActivationCutoverStateStoreV1 for SqliteS2LiteStoreV1<'_> {
    fn authority_identity(&self) -> u64 {
        MigrationStateStoreV1::authority_identity(self)
    }

    fn load(&mut self) -> Result<Option<ActivationCutoverStateV1>> {
        MigrationStateStoreV1::load_cutover_state(self, self.root_id)
    }

    fn persist(&mut self, state: &ActivationCutoverStateV1) -> Result<()> {
        let root_id = self.root_id.to_string();
        MigrationStateStoreV1::persist_cutover_state(self, &root_id, state)
    }
}

impl PreparedIntentStoreV1 for SqliteS2LiteStoreV1<'_> {
    fn persist(&mut self, intent: &PreparedIntentV1) -> Result<()> {
        validate_prepared_intent_v1(intent)?;
        let mut metadata = intent.clone();
        let exact_bytes = std::mem::take(&mut metadata.exact_bytes);
        let mut conn = self.connection()?;
        let transaction = database(conn.transaction_with_behavior(TransactionBehavior::Immediate))?;
        persist_intent_parts(
            &transaction,
            self.root_id,
            "commit",
            &intent.remote_path,
            &intent.intent_fingerprint,
            &metadata,
            &exact_bytes,
        )?;
        let retained = load_commit_intent_from(&transaction, self.root_id, &intent.remote_path)?
            .ok_or(STORE_FAILURE)?;
        if retained != *intent {
            return Err(STORE_CORRUPTION);
        }
        database(transaction.commit())?;
        Ok(())
    }
}

impl PreparedActivationIntentStoreV1 for SqliteS2LiteStoreV1<'_> {
    fn persist(&mut self, intent: &PreparedActivationIntentV1) -> Result<()> {
        validate_prepared_activation_intent_v1(intent)?;
        let mut metadata = intent.clone();
        let exact_bytes = std::mem::take(&mut metadata.exact_bytes);
        let mut conn = self.connection()?;
        let transaction = database(conn.transaction_with_behavior(TransactionBehavior::Immediate))?;
        persist_intent_parts(
            &transaction,
            self.root_id,
            "activation",
            &intent.remote_path,
            &intent.intent_fingerprint,
            &metadata,
            &exact_bytes,
        )?;
        let retained =
            load_activation_intent_from(&transaction, self.root_id, &intent.remote_path)?
                .ok_or(STORE_FAILURE)?;
        if retained != *intent {
            return Err(STORE_CORRUPTION);
        }
        database(transaction.commit())?;
        Ok(())
    }
}

impl PublishedReceiptStoreV1 for SqliteS2LiteStoreV1<'_> {
    fn persist(&mut self, receipt: &RemotePublishedReceiptV1) -> Result<()> {
        self.persist_root_bound_receipt(&RootBoundPublishedReceiptV1 {
            physical_root_id: self.root_id.to_string(),
            receipt: receipt.clone(),
        })
    }

    fn load_verified_receipt(
        &mut self,
        remote_path: &str,
    ) -> Result<Option<RemotePublishedReceiptV1>> {
        self.load_published_receipt(remote_path)
    }
}

impl PublishedActivationReceiptStoreV1 for SqliteS2LiteStoreV1<'_> {
    fn persist(&mut self, receipt: &PublishedActivationReceiptV1) -> Result<()> {
        self.persist_root_bound_activation_receipt(&RootBoundPublishedActivationReceiptV1 {
            physical_root_id: self.root_id.to_string(),
            receipt: receipt.clone(),
        })
    }

    fn load_verified_activation_receipt(
        &mut self,
        remote_path: &str,
    ) -> Result<Option<PublishedActivationReceiptV1>> {
        self.load_published_activation_receipt(remote_path)
    }
}

pub fn persistence_schema_version(conn: &Connection) -> rusqlite::Result<Option<String>> {
    conn.query_row(
        "SELECT value FROM settings WHERE key='s2_lite_persistence_schema_version'",
        [],
        |row| row.get(0),
    )
    .optional()
}

pub const S2_LITE_PERSISTENCE_SCHEMA_VERSION: &str = SCHEMA_VERSION;

/// Final S1 business acknowledgement gate, used inside the caller's transaction.
/// The same strict root authority as publication prevents stale S1 responses
/// from changing business tables or clearing staging after cutover.
pub(crate) fn admit_legacy_business_ack_v1(conn: &Connection) -> Result<()> {
    if load_migration_source_owner(conn)?.is_some() {
        return Err(ProtocolError("S2_LEGACY_PUT_BLOCKED"));
    }
    let Some(registry) = crate::sync_targets::registry(conn).map_err(|_| STORE_CORRUPTION)? else {
        return Ok(());
    };
    let Some(target) = registry
        .targets
        .iter()
        .find(|target| Some(target.id.as_str()) == registry.active_target_id.as_deref())
    else {
        return Ok(());
    };
    let root = super::webdav_adapter::webdav_root_v1(&target.normalized_url, &target.username)
        .map_err(|_| ROOT_MISMATCH)?;
    if let Some(safety) = load_root_safety_from(conn, &root.physical_root_id)? {
        if safety.cutover_state.remote_s2_activated || !safety.root_fatal_signals.is_empty() {
            return Err(ProtocolError("S2_LEGACY_PUT_BLOCKED"));
        }
    }
    if let Some(read) =
        super::discovery_persistence::load_read_state_v1(conn, &root.physical_root_id)?
    {
        let cutover = recover_activation_cutover_v1(&read.discovery.state, None)
            .diagnostic_state()
            .ok_or(STORE_CORRUPTION)?;
        if cutover.remote_s2_activated
            || !read.fatal_codes.is_empty()
            || !cutover.root_fatal_signals.is_empty()
        {
            return Err(ProtocolError("S2_LEGACY_PUT_BLOCKED"));
        }
    }
    Ok(())
}
