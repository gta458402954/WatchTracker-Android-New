use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::activation_cutover::{
    begin_activation_cutover_recovery_v1, create_activation_cutover_state_v1,
    evaluate_activation_cutover_v1, recover_activation_cutover_v1, ActivationCutoverFatalV1,
    ActivationCutoverRecoveryV1, ActivationCutoverStateV1,
};
use super::bootstrap::build_bootstrap_plan_v1;
use super::canonical::{
    jcs_bytes, sha256_hex, validate_canonical_uuid_v4, validate_timestamp, ProtocolError, Result,
};
use super::immutable_publish::{
    persist_prepared_activation_intent_before_publish_v1,
    persist_prepared_intent_before_publish_v1, persist_verified_activation_receipt_v1,
    persist_verified_receipt_v1, prepare_activation_intent_v1, prepare_commit_intent_v1,
    publish_admitted_persisted_activation_intent_v1, publish_admitted_persisted_intent_v1,
    reuse_durable_published_activation_receipt_v1, reuse_durable_published_receipt_v1,
    validate_prepared_activation_intent_v1, validate_prepared_intent_v1,
    validate_published_activation_receipt_v1, validate_published_receipt_v1,
    ImmutableObjectRemoteV1, PreparedActivationIntentStoreV1, PreparedActivationIntentV1,
    PreparedIntentStoreV1, PreparedIntentV1, PublishActivationResultV1,
    PublishedActivationReceiptStoreV1, PublishedActivationReceiptV1, PublishedReceiptStoreV1,
    RecoverActivationIntentResultV1, RecoverPreparedIntentResultV1, RemotePublishedReceiptV1,
};
use super::remote_discovery::{
    create_discovery_state_v1, VerifiedFingerprintEvidenceV1, VerifiedRemoteObjectV1,
};
use super::semantic::business_field_order;
use super::types::{BootstrapEntity, CommitRef, LegacySemanticAdapterV1};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum MigrationStatusV1 {
    NotStarted,
    LegacySnapshotCaptured,
    BootstrapPlanned,
    StageAPublishing,
    StageAComplete,
    StageBPublishing,
    StageBComplete,
    ActivationPublishing,
    ActivationVerified,
    MigrationComplete,
    RootFrozen,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CapturedLegacySnapshotV1 {
    pub snapshot_version: u8,
    pub legacy_fingerprint: String,
    pub canonical_entities: Vec<BootstrapEntity>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MigrationCommitTaskV1 {
    pub stage: String,
    pub chunk_index: usize,
    pub root_id: String,
    pub intent: PreparedIntentV1,
    pub receipt: Option<RemotePublishedReceiptV1>,
    pub receipt_root_id: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MigrationRootFatalV1 {
    pub code: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MigrationRootSafetyStateV1 {
    pub state_version: u8,
    pub root_id: String,
    pub generation: u64,
    pub root_fatal_signals: Vec<MigrationRootFatalV1>,
    pub cutover_state: ActivationCutoverStateV1,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PreservationHandoffV1 {
    pub old_root_id: String,
    pub fatal_codes: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MigrationStateV1 {
    pub state_version: u8,
    pub generation: u64,
    pub migration_id: String,
    pub root_id: String,
    pub source_type: String,
    pub writer_id: String,
    pub created_at: String,
    pub status: MigrationStatusV1,
    pub snapshot: Option<CapturedLegacySnapshotV1>,
    pub stage_a: Vec<MigrationCommitTaskV1>,
    pub stage_b: Vec<MigrationCommitTaskV1>,
    pub activation_intent: Option<PreparedActivationIntentV1>,
    pub activation_intent_root_id: Option<String>,
    pub activation_receipt: Option<PublishedActivationReceiptV1>,
    pub activation_receipt_root_id: Option<String>,
    pub root_fatal_signals: Vec<MigrationRootFatalV1>,
    pub preservation_handoff: Option<PreservationHandoffV1>,
}

pub trait MigrationStateStoreV1 {
    /// A strict durable store can require the frozen canonical recovery form.
    /// In-memory pure stores retain the frozen executor's explicit boundaries.
    fn transition_for_storage_v1(&self, state: &MigrationStateV1) -> Result<MigrationStateV1> {
        Ok(state.clone())
    }

    fn authority_identity(&self) -> u64;
    fn claim_or_load(&mut self, candidate: &MigrationStateV1) -> Result<MigrationStateV1>;
    fn load(&mut self, root_id: &str) -> Result<Option<MigrationStateV1>>;
    fn compare_and_swap(
        &mut self,
        root_id: &str,
        migration_id: &str,
        expected_generation: u64,
        next: &MigrationStateV1,
    ) -> Result<bool>;
    fn load_root_safety(&mut self, root_id: &str) -> Result<MigrationRootSafetyStateV1>;
    fn load_cutover_state(&mut self, root_id: &str) -> Result<Option<ActivationCutoverStateV1>>;
    /// Monotonic reconcile inside the root authority; an owned fatal cannot be
    /// replaced by a later, safer caller snapshot.
    fn persist_cutover_state(
        &mut self,
        root_id: &str,
        state: &ActivationCutoverStateV1,
    ) -> Result<()>;
    fn persist_root_fatal(&mut self, root_id: &str, code: &str)
        -> Result<Option<MigrationStateV1>>;
    /// Serializes root-fatal persistence with admission of exactly one PUT.
    /// A fatal that wins this authority first rejects `operation`; after a
    /// safe read, `operation` is invoked synchronously while exclusion is held.
    fn run_publish_exclusive<T, F: FnOnce() -> Result<T>>(
        &mut self,
        root_id: &str,
        migration_id: &str,
        expected_generation: u64,
        expected_execution_identity: Option<&MigrationExecutionIdentityV1>,
        operation: F,
    ) -> Result<PublishExclusiveResultV1<T>>;
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PublishExclusiveResultV1<T> {
    Executed(T),
    Rejected(Box<MigrationStateV1>),
}

/// Immutable historical execution identity captured before an activation
/// attempt. It deliberately contains every durable execution-binding field
/// that can select a target/root or change the frozen migration source.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MigrationExecutionIdentityV1 {
    pub physical_root_id: String,
    pub target_id: String,
    pub target_epoch: u64,
    pub captured_records_generation: i64,
    pub legacy_fingerprint: Option<String>,
    pub migration_id: String,
}

#[derive(Clone, Debug)]
pub struct MigrationRootExecutionCapabilityV1 {
    root_id: String,
    migration_id: String,
    remote_identity: u64,
    migration_authority_identity: u64,
    activation_publication_only: bool,
    expected_execution_identity: Option<MigrationExecutionIdentityV1>,
}

#[derive(Clone, Debug)]
pub struct MigrationAttemptAttachmentV1 {
    pub state: MigrationStateV1,
    root_id: String,
    migration_id: String,
    migration_authority_identity: u64,
}

pub fn create_migration_root_execution_capability_v1<
    R: ImmutableObjectRemoteV1,
    M: MigrationStateStoreV1,
>(
    attachment: &MigrationAttemptAttachmentV1,
    remote: &R,
    migration_store: &M,
) -> Result<MigrationRootExecutionCapabilityV1> {
    if attachment.migration_authority_identity != migration_store.authority_identity()
        || remote.physical_root_id() != Some(attachment.root_id.as_str())
    {
        return Err(ProtocolError("MIGRATION_ROOT_BINDING_MISMATCH"));
    }
    Ok(MigrationRootExecutionCapabilityV1 {
        root_id: attachment.root_id.clone(),
        migration_id: attachment.migration_id.clone(),
        remote_identity: remote.execution_context_identity(),
        migration_authority_identity: migration_store.authority_identity(),
        activation_publication_only: false,
        expected_execution_identity: None,
    })
}

/// Opaque capability for the 4B2.1 activation-publication phase. It shares
/// all root and remote identity validation with normal migration execution,
/// but never permits the executor to enter the later cutover/completion phase.
pub fn create_activation_publication_execution_capability_v1<
    R: ImmutableObjectRemoteV1,
    M: MigrationStateStoreV1,
>(
    attachment: &MigrationAttemptAttachmentV1,
    remote: &R,
    migration_store: &M,
    expected_execution_identity: MigrationExecutionIdentityV1,
) -> Result<MigrationRootExecutionCapabilityV1> {
    let mut capability =
        create_migration_root_execution_capability_v1(attachment, remote, migration_store)?;
    capability.activation_publication_only = true;
    if expected_execution_identity.physical_root_id != capability.root_id
        || expected_execution_identity.migration_id != capability.migration_id
    {
        return Err(ProtocolError("MIGRATION_ROOT_BINDING_MISMATCH"));
    }
    capability.expected_execution_identity = Some(expected_execution_identity);
    Ok(capability)
}

pub trait ActivationCutoverStateStoreV1 {
    fn authority_identity(&self) -> u64;
    fn load(&mut self) -> Result<Option<ActivationCutoverStateV1>>;
    fn persist(&mut self, state: &ActivationCutoverStateV1) -> Result<()>;
}

pub fn create_migration_root_safety_state_v1(root_id: &str) -> MigrationRootSafetyStateV1 {
    MigrationRootSafetyStateV1 {
        state_version: 1,
        root_id: root_id.to_string(),
        generation: 0,
        root_fatal_signals: vec![],
        cutover_state: create_activation_cutover_state_v1(),
    }
}

pub fn merge_migration_root_cutover_state_v1(
    existing: &ActivationCutoverStateV1,
    incoming: &ActivationCutoverStateV1,
) -> Result<ActivationCutoverStateV1> {
    let mut discovery = create_discovery_state_v1();
    discovery.verified_objects = incoming
        .verified_activation_evidence
        .iter()
        .map(|value| VerifiedRemoteObjectV1 {
            path: value.path.clone(),
            kind: "activation".to_string(),
            exact_bytes_hash: value.exact_bytes_hash.clone(),
            exact_bytes_hex: String::new(),
            content_hash: value.content_hash.clone(),
            commit_ref: None,
            activation_id: Some(value.activation_id.clone()),
            fingerprint_evidence: value.legacy_fingerprint.as_ref().map_or(
                VerifiedFingerprintEvidenceV1::Null,
                |fingerprint| VerifiedFingerprintEvidenceV1::Value {
                    value: fingerprint.clone(),
                },
            ),
        })
        .collect();
    let mut merged = evaluate_activation_cutover_v1(existing, &discovery)?;
    let mut fatal_codes = existing
        .root_fatal_signals
        .iter()
        .chain(&incoming.root_fatal_signals)
        .chain(&merged.root_fatal_signals)
        .map(|fatal| fatal.code.clone())
        .collect::<Vec<_>>();
    fatal_codes.sort();
    fatal_codes.dedup();
    merged.root_fatal_signals = fatal_codes
        .into_iter()
        .map(|code| ActivationCutoverFatalV1 { code })
        .collect();
    Ok(merged)
}

#[derive(Clone, Debug)]
pub struct LegacySnapshotEntryV1 {
    pub entity_type: String,
    pub value: Value,
}

pub fn validate_migration_activation_body_v1(bytes: &[u8]) -> Result<Value> {
    let value: Value =
        serde_json::from_slice(bytes).map_err(|_| ProtocolError("INVALID_ACTIVATION_BODY"))?;
    let object = value
        .as_object()
        .ok_or(ProtocolError("INVALID_ACTIVATION_BODY"))?;
    let expected = [
        "activationId",
        "legacyFingerprint",
        "protocol",
        "protocolVersion",
        "requiredFeatures",
        "s2SemanticProfileVersion",
    ];
    if object.len() != expected.len()
        || expected.iter().any(|field| !object.contains_key(*field))
        || object["protocol"] != "watchtracker-s2-lite"
        || object["protocolVersion"] != 1
        || object["s2SemanticProfileVersion"] != 1
        || object["requiredFeatures"] != json!([])
    {
        return Err(ProtocolError("INVALID_ACTIVATION_BODY"));
    }
    let activation_id = object["activationId"]
        .as_str()
        .ok_or(ProtocolError("INVALID_ACTIVATION_BODY"))?;
    validate_canonical_uuid_v4(activation_id)
        .map_err(|_| ProtocolError("INVALID_ACTIVATION_BODY"))?;
    let fingerprint = object["legacyFingerprint"]
        .as_str()
        .ok_or(ProtocolError("INVALID_ACTIVATION_BODY"))?;
    if fingerprint.len() != 64
        || !fingerprint
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err(ProtocolError("INVALID_ACTIVATION_BODY"));
    }
    Ok(value)
}

fn uuid_from_hash(hash: &str) -> String {
    let mut value = hash.as_bytes()[..32].to_vec();
    value[12] = b'4';
    value[16] = match value[16] {
        b'0' | b'4' | b'8' | b'c' => b'8',
        b'1' | b'5' | b'9' | b'd' => b'9',
        b'2' | b'6' | b'a' | b'e' => b'a',
        _ => b'b',
    };
    let compact = String::from_utf8(value).expect("sha256 is ascii");
    format!(
        "{}-{}-{}-{}-{}",
        &compact[..8],
        &compact[8..12],
        &compact[12..16],
        &compact[16..20],
        &compact[20..]
    )
}

fn deterministic_uuid_v4(migration_id: &str, domain: &str, index: usize) -> String {
    uuid_from_hash(&sha256_hex(
        format!("{migration_id}\0{domain}\0{index}").as_bytes(),
    ))
}

pub fn create_migration_state_v1(
    migration_id: &str,
    root_id: &str,
    writer_id: &str,
    created_at: &str,
    source_type: &str,
) -> Result<MigrationStateV1> {
    validate_canonical_uuid_v4(migration_id)?;
    validate_canonical_uuid_v4(writer_id)?;
    validate_timestamp(created_at)?;
    if root_id.is_empty() || !matches!(source_type, "legacy-bootstrap" | "new-root-bootstrap") {
        return Err(ProtocolError("invalid_migration_root"));
    }
    Ok(MigrationStateV1 {
        state_version: 1,
        generation: 0,
        migration_id: migration_id.to_string(),
        root_id: root_id.to_string(),
        source_type: source_type.to_string(),
        writer_id: writer_id.to_string(),
        created_at: created_at.to_string(),
        status: MigrationStatusV1::NotStarted,
        snapshot: None,
        stage_a: vec![],
        stage_b: vec![],
        activation_intent: None,
        activation_intent_root_id: None,
        activation_receipt: None,
        activation_receipt_root_id: None,
        root_fatal_signals: vec![],
        preservation_handoff: None,
    })
}

pub fn capture_legacy_snapshot_v1<A: LegacySemanticAdapterV1>(
    input: &[LegacySnapshotEntryV1],
    adapter: &A,
) -> Result<CapturedLegacySnapshotV1> {
    let owned_input = input.to_vec();
    let entities = owned_input
        .iter()
        .map(|entry| {
            adapter
                .adapt_live_entity(&entry.entity_type, &entry.value)
                .map_err(|_| ProtocolError("LEGACY_SNAPSHOT_INVALID"))
        })
        .collect::<Result<Vec<_>>>()?;
    let plan =
        build_bootstrap_plan_v1(&entities).map_err(|_| ProtocolError("LEGACY_SNAPSHOT_INVALID"))?;
    let canonical_entities = plan
        .stage_a_ordered_mutations
        .into_iter()
        .chain(plan.stage_b_ordered_mutations)
        .collect::<Vec<_>>();
    let legacy_fingerprint = sha256_hex(&jcs_bytes(&json!({
        "domain": "watchtracker-s2-lite-legacy-snapshot-v1",
        "canonicalEntities": canonical_entities,
    }))?);
    Ok(CapturedLegacySnapshotV1 {
        snapshot_version: 1,
        legacy_fingerprint,
        canonical_entities,
    })
}

pub fn retain_captured_snapshot_v1(
    prior: &MigrationStateV1,
    snapshot: &CapturedLegacySnapshotV1,
) -> Result<MigrationStateV1> {
    if prior.status != MigrationStatusV1::NotStarted || prior.snapshot.is_some() {
        return Err(ProtocolError("invalid_migration_transition"));
    }
    let mut state = prior.clone();
    state.snapshot = Some(snapshot.clone());
    state.status = MigrationStatusV1::LegacySnapshotCaptured;
    Ok(state)
}

fn commit_wire(
    state: &MigrationStateV1,
    chunk: &[BootstrapEntity],
    writer_seq: usize,
    commit_id: &str,
    previous: Option<&CommitRef>,
    mutation_ids: &[String],
) -> Value {
    json!({
        "protocol": "watchtracker-s2-lite",
        "protocolVersion": 1,
        "s2SemanticProfileVersion": 1,
        "requiredFeatures": [],
        "writerId": state.writer_id,
        "writerSeq": writer_seq.to_string(),
        "commitId": commit_id,
        "previousWriterCommit": previous,
        "basisClock": previous.into_iter().collect::<Vec<_>>(),
        "commitKind": "bootstrap",
        "createdAt": state.created_at,
        "source": { "type": state.source_type },
        "mutations": chunk.iter().zip(mutation_ids).map(|(entity, mutation_id)| json!({
            "localMutationId": mutation_id,
            "entityType": entity.entity_type,
            "entityKey": entity.entity_key,
            "operation": "upsert",
            "value": entity.value,
            "baseFrontier": [],
            "changedFields": business_field_order(&entity.entity_type).expect("planned entity type"),
        })).collect::<Vec<_>>(),
    })
}

pub fn plan_captured_migration_v1(prior: &MigrationStateV1) -> Result<MigrationStateV1> {
    if prior.status != MigrationStatusV1::LegacySnapshotCaptured || prior.snapshot.is_none() {
        return Err(ProtocolError("invalid_migration_transition"));
    }
    let mut state = prior.clone();
    let plan = build_bootstrap_plan_v1(&state.snapshot.as_ref().unwrap().canonical_entities)?;
    let planning_state = state.clone();
    let mut previous: Option<CommitRef> = None;
    let mut writer_seq = 1usize;
    let mut build =
        |stage: &str, chunks: &[Vec<BootstrapEntity>]| -> Result<Vec<MigrationCommitTaskV1>> {
            let mut tasks = Vec::new();
            for (chunk_index, chunk) in chunks.iter().enumerate() {
                let commit_id =
                    deterministic_uuid_v4(&planning_state.migration_id, "commit", writer_seq);
                let mutation_ids = (0..chunk.len())
                    .map(|index| {
                        deterministic_uuid_v4(
                            &planning_state.migration_id,
                            &format!("mutation-{writer_seq}"),
                            index,
                        )
                    })
                    .collect::<Vec<_>>();
                let bytes = jcs_bytes(&commit_wire(
                    &planning_state,
                    chunk,
                    writer_seq,
                    &commit_id,
                    previous.as_ref(),
                    &mutation_ids,
                ))?;
                let intent = prepare_commit_intent_v1(&bytes, &planning_state.created_at)?;
                previous = Some(intent.commit_ref.clone());
                tasks.push(MigrationCommitTaskV1 {
                    stage: stage.to_string(),
                    chunk_index,
                    root_id: planning_state.root_id.clone(),
                    intent,
                    receipt: None,
                    receipt_root_id: None,
                });
                writer_seq += 1;
            }
            Ok(tasks)
        };
    state.stage_a = build("A", &plan.stage_a_chunks)?;
    state.stage_b = build("B", &plan.stage_b_chunks)?;
    let activation_id = deterministic_uuid_v4(&state.migration_id, "activation", 0);
    let activation_bytes = jcs_bytes(&json!({
        "protocol": "watchtracker-s2-lite",
        "protocolVersion": 1,
        "s2SemanticProfileVersion": 1,
        "requiredFeatures": [],
        "activationId": activation_id,
        "legacyFingerprint": state.snapshot.as_ref().unwrap().legacy_fingerprint,
    }))?;
    validate_migration_activation_body_v1(&activation_bytes)?;
    state.activation_intent = Some(prepare_activation_intent_v1(
        &activation_id,
        &activation_bytes,
        &state.created_at,
    )?);
    state.activation_intent_root_id = Some(state.root_id.clone());
    state.status = MigrationStatusV1::BootstrapPlanned;
    Ok(state)
}

fn all_receipted(tasks: &[MigrationCommitTaskV1]) -> bool {
    tasks.iter().all(|task| task.receipt.is_some())
}

pub fn reconcile_migration_state_v1(input: &MigrationStateV1) -> Result<MigrationStateV1> {
    let mut state = input.clone();
    if state.state_version != 1 || state.root_id.is_empty() {
        return Err(ProtocolError("LOCAL_MIGRATION_STATE_CORRUPTION"));
    }
    validate_canonical_uuid_v4(&state.migration_id)
        .and_then(|_| validate_canonical_uuid_v4(&state.writer_id))
        .and_then(|_| validate_timestamp(&state.created_at))
        .map_err(|_| ProtocolError("LOCAL_MIGRATION_STATE_CORRUPTION"))?;
    if let Some(snapshot) = &state.snapshot {
        let plan = build_bootstrap_plan_v1(&snapshot.canonical_entities)
            .map_err(|_| ProtocolError("LOCAL_MIGRATION_STATE_CORRUPTION"))?;
        let canonical_entities = plan
            .stage_a_ordered_mutations
            .into_iter()
            .chain(plan.stage_b_ordered_mutations)
            .collect::<Vec<_>>();
        let fingerprint = sha256_hex(&jcs_bytes(&json!({
            "domain": "watchtracker-s2-lite-legacy-snapshot-v1",
            "canonicalEntities": canonical_entities,
        }))?);
        if fingerprint != snapshot.legacy_fingerprint {
            return Err(ProtocolError("LOCAL_MIGRATION_STATE_CORRUPTION"));
        }
    }
    if !state.root_fatal_signals.is_empty() || state.status == MigrationStatusV1::RootFrozen {
        state.status = MigrationStatusV1::RootFrozen;
        return Ok(state);
    }
    if matches!(
        state.status,
        MigrationStatusV1::NotStarted | MigrationStatusV1::LegacySnapshotCaptured
    ) {
        return Ok(state);
    }
    if state.snapshot.is_none() || state.activation_intent.is_none() {
        return Err(ProtocolError("LOCAL_MIGRATION_STATE_CORRUPTION"));
    }
    let mut expected_seed = state.clone();
    expected_seed.status = MigrationStatusV1::LegacySnapshotCaptured;
    expected_seed.stage_a.clear();
    expected_seed.stage_b.clear();
    expected_seed.activation_intent = None;
    expected_seed.activation_intent_root_id = None;
    expected_seed.activation_receipt = None;
    expected_seed.activation_receipt_root_id = None;
    let expected_plan = plan_captured_migration_v1(&expected_seed)
        .map_err(|_| ProtocolError("LOCAL_MIGRATION_STATE_CORRUPTION"))?;
    let actual_tasks = state
        .stage_a
        .iter()
        .chain(&state.stage_b)
        .collect::<Vec<_>>();
    let expected_tasks = expected_plan
        .stage_a
        .iter()
        .chain(&expected_plan.stage_b)
        .collect::<Vec<_>>();
    if actual_tasks.len() != expected_tasks.len()
        || actual_tasks
            .iter()
            .zip(expected_tasks)
            .any(|(actual, expected)| {
                actual.stage != expected.stage
                    || actual.chunk_index != expected.chunk_index
                    || actual.root_id != state.root_id
                    || actual.root_id != expected.root_id
                    || actual.intent.intent_fingerprint != expected.intent.intent_fingerprint
            })
        || state.activation_intent_root_id.as_deref() != Some(state.root_id.as_str())
        || state.activation_intent.as_ref().unwrap().intent_fingerprint
            != expected_plan
                .activation_intent
                .as_ref()
                .unwrap()
                .intent_fingerprint
    {
        return Err(ProtocolError("LOCAL_MIGRATION_STATE_CORRUPTION"));
    }
    for task in state.stage_a.iter().chain(&state.stage_b) {
        if task.receipt.is_some() != task.receipt_root_id.is_some()
            || (task.receipt.is_some()
                && task.receipt_root_id.as_deref() != Some(state.root_id.as_str()))
        {
            return Err(ProtocolError("LOCAL_MIGRATION_STATE_CORRUPTION"));
        }
        validate_prepared_intent_v1(&task.intent)
            .map_err(|_| ProtocolError("LOCAL_MIGRATION_STATE_CORRUPTION"))?;
        if let Some(receipt) = &task.receipt {
            validate_published_receipt_v1(receipt, &task.intent)
                .map_err(|_| ProtocolError("LOCAL_MIGRATION_STATE_CORRUPTION"))?;
        }
    }
    let activation_intent = state.activation_intent.as_ref().unwrap();
    validate_prepared_activation_intent_v1(activation_intent)
        .map_err(|_| ProtocolError("LOCAL_MIGRATION_STATE_CORRUPTION"))?;
    let body = validate_migration_activation_body_v1(&activation_intent.exact_bytes)
        .map_err(|_| ProtocolError("LOCAL_MIGRATION_STATE_CORRUPTION"))?;
    if body["activationId"] != activation_intent.activation_id
        || body["legacyFingerprint"] != state.snapshot.as_ref().unwrap().legacy_fingerprint
    {
        return Err(ProtocolError("LOCAL_MIGRATION_STATE_CORRUPTION"));
    }
    if let Some(receipt) = &state.activation_receipt {
        if state.activation_receipt_root_id.as_deref() != Some(state.root_id.as_str()) {
            return Err(ProtocolError("LOCAL_MIGRATION_STATE_CORRUPTION"));
        }
        validate_published_activation_receipt_v1(receipt, activation_intent)
            .map_err(|_| ProtocolError("LOCAL_MIGRATION_STATE_CORRUPTION"))?;
    } else if state.activation_receipt_root_id.is_some() {
        return Err(ProtocolError("LOCAL_MIGRATION_STATE_CORRUPTION"));
    }
    if !all_receipted(&state.stage_a) {
        state.status = if state.status == MigrationStatusV1::BootstrapPlanned {
            MigrationStatusV1::BootstrapPlanned
        } else {
            MigrationStatusV1::StageAPublishing
        };
    } else if !all_receipted(&state.stage_b) {
        state.status = if state.status == MigrationStatusV1::StageAComplete {
            MigrationStatusV1::StageAComplete
        } else {
            MigrationStatusV1::StageBPublishing
        };
    } else if state.activation_receipt.is_none()
        && !matches!(
            state.status,
            MigrationStatusV1::ActivationVerified | MigrationStatusV1::MigrationComplete
        )
    {
        // Stage boundaries are durable crash-recovery points. In particular,
        // a completed Stage B task must not enter activation before the
        // executor explicitly advances it. Stage A is retained only while
        // there is actual Stage B work left to admit.
        state.status = match state.status {
            MigrationStatusV1::StageAComplete if !state.stage_b.is_empty() => {
                MigrationStatusV1::StageAComplete
            }
            MigrationStatusV1::StageBComplete => MigrationStatusV1::StageBComplete,
            _ => MigrationStatusV1::ActivationPublishing,
        };
    } else {
        // Completion is a durable terminal lifecycle fact.  Reconciliation
        // may derive activation verification from local receipts or preserve
        // compatible remote-activation adoption, but it must not erase a
        // previously committed verification/completion on restart.
        state.status = if state.status == MigrationStatusV1::MigrationComplete {
            MigrationStatusV1::MigrationComplete
        } else {
            MigrationStatusV1::ActivationVerified
        };
    }
    Ok(state)
}

fn push_fatal(state: &mut MigrationStateV1, code: &str) {
    if !state
        .root_fatal_signals
        .iter()
        .any(|value| value.code == code)
    {
        state.root_fatal_signals.push(MigrationRootFatalV1 {
            code: code.to_string(),
        });
    }
}

pub fn start_or_attach_migration_v1<M: MigrationStateStoreV1>(
    candidate: &MigrationStateV1,
    store: &mut M,
) -> Result<MigrationAttemptAttachmentV1> {
    let candidate = reconcile_migration_state_v1(candidate)?;
    let existing = store.load(&candidate.root_id)?;
    if existing.is_none()
        && (candidate.status != MigrationStatusV1::NotStarted || candidate.generation != 0)
    {
        return Err(ProtocolError("invalid_migration_start"));
    }
    let attached = reconcile_migration_state_v1(&store.claim_or_load(&candidate)?)?;
    if attached.root_id != candidate.root_id {
        return Err(ProtocolError("MIGRATION_ROOT_BINDING_MISMATCH"));
    }
    Ok(MigrationAttemptAttachmentV1 {
        root_id: attached.root_id.clone(),
        migration_id: attached.migration_id.clone(),
        migration_authority_identity: store.authority_identity(),
        state: attached,
    })
}

pub(crate) fn validate_attempt_transition(
    prior: &MigrationStateV1,
    requested: &MigrationStateV1,
) -> Result<()> {
    if prior.root_id != requested.root_id || prior.migration_id != requested.migration_id {
        return Err(ProtocolError("MIGRATION_ROOT_BINDING_MISMATCH"));
    }
    if prior.writer_id != requested.writer_id
        || prior.source_type != requested.source_type
        || prior.created_at != requested.created_at
    {
        return Err(ProtocolError("LOCAL_MIGRATION_STATE_CORRUPTION"));
    }
    if let Some(prior_snapshot) = &prior.snapshot {
        let Some(requested_snapshot) = &requested.snapshot else {
            return Err(ProtocolError("LOCAL_MIGRATION_STATE_CORRUPTION"));
        };
        if prior_snapshot.legacy_fingerprint != requested_snapshot.legacy_fingerprint
            || jcs_bytes(&json!(prior_snapshot.canonical_entities))?
                != jcs_bytes(&json!(requested_snapshot.canonical_entities))?
        {
            return Err(ProtocolError("LOCAL_MIGRATION_STATE_CORRUPTION"));
        }
    }
    let rank = |status: MigrationStatusV1| match status {
        MigrationStatusV1::NotStarted => 0,
        MigrationStatusV1::LegacySnapshotCaptured => 1,
        MigrationStatusV1::BootstrapPlanned => 2,
        MigrationStatusV1::StageAPublishing => 3,
        MigrationStatusV1::StageAComplete => 4,
        MigrationStatusV1::StageBPublishing => 5,
        MigrationStatusV1::StageBComplete => 6,
        MigrationStatusV1::ActivationPublishing => 7,
        MigrationStatusV1::ActivationVerified => 8,
        MigrationStatusV1::MigrationComplete => 9,
        MigrationStatusV1::RootFrozen => 10,
    };
    if rank(prior.status) >= rank(MigrationStatusV1::BootstrapPlanned) {
        let task_identity = |task: &MigrationCommitTaskV1| {
            (
                task.stage.clone(),
                task.chunk_index,
                task.root_id.clone(),
                task.intent.intent_fingerprint.clone(),
            )
        };
        if prior
            .snapshot
            .as_ref()
            .map(|value| value.legacy_fingerprint.as_str())
            != requested
                .snapshot
                .as_ref()
                .map(|value| value.legacy_fingerprint.as_str())
            || prior.stage_a.iter().map(task_identity).collect::<Vec<_>>()
                != requested
                    .stage_a
                    .iter()
                    .map(task_identity)
                    .collect::<Vec<_>>()
            || prior.stage_b.iter().map(task_identity).collect::<Vec<_>>()
                != requested
                    .stage_b
                    .iter()
                    .map(task_identity)
                    .collect::<Vec<_>>()
            || prior
                .activation_intent
                .as_ref()
                .map(|value| value.intent_fingerprint.as_str())
                != requested
                    .activation_intent
                    .as_ref()
                    .map(|value| value.intent_fingerprint.as_str())
            || prior.activation_intent_root_id != requested.activation_intent_root_id
            || prior.preservation_handoff != requested.preservation_handoff
        {
            return Err(ProtocolError("LOCAL_MIGRATION_STATE_CORRUPTION"));
        }
    }
    for (old, new) in prior
        .stage_a
        .iter()
        .chain(&prior.stage_b)
        .zip(requested.stage_a.iter().chain(&requested.stage_b))
    {
        if old.receipt.is_some()
            && (old.receipt != new.receipt || old.receipt_root_id != new.receipt_root_id)
        {
            return Err(ProtocolError("LOCAL_MIGRATION_STATE_CORRUPTION"));
        }
    }
    if prior.activation_receipt.is_some()
        && (prior.activation_receipt != requested.activation_receipt
            || prior.activation_receipt_root_id != requested.activation_receipt_root_id)
    {
        return Err(ProtocolError("LOCAL_MIGRATION_STATE_CORRUPTION"));
    }
    if prior.status != MigrationStatusV1::RootFrozen
        && requested.status != MigrationStatusV1::RootFrozen
        && rank(requested.status) < rank(prior.status)
    {
        return Err(ProtocolError("LOCAL_MIGRATION_STATE_CORRUPTION"));
    }
    Ok(())
}

fn persist_transition<M: MigrationStateStoreV1>(
    prior: &MigrationStateV1,
    requested: &MigrationStateV1,
    store: &mut M,
) -> Result<MigrationStateV1> {
    validate_attempt_transition(prior, requested)?;
    let mut next = requested.clone();
    for fatal in &prior.root_fatal_signals {
        push_fatal(&mut next, &fatal.code);
    }
    next.root_fatal_signals.sort_by(|a, b| a.code.cmp(&b.code));
    if !next.root_fatal_signals.is_empty() || prior.status == MigrationStatusV1::RootFrozen {
        next.status = MigrationStatusV1::RootFrozen;
    }
    next.generation = prior
        .generation
        .checked_add(1)
        .ok_or(ProtocolError("LOCAL_MIGRATION_STATE_CORRUPTION"))?;
    next = store.transition_for_storage_v1(&next)?;
    if store.compare_and_swap(&prior.root_id, &prior.migration_id, prior.generation, &next)? {
        return Ok(next);
    }
    let current = store
        .load(&prior.root_id)?
        .ok_or(ProtocolError("MIGRATION_ROOT_BINDING_MISMATCH"))?;
    if current.migration_id != prior.migration_id {
        return Err(ProtocolError("MIGRATION_ROOT_BINDING_MISMATCH"));
    }
    reconcile_migration_state_v1(&current)
}

fn activation_discovery(state: &MigrationStateV1) -> super::remote_discovery::DiscoveryStateV1 {
    let intent = state.activation_intent.as_ref().unwrap();
    let mut discovery = create_discovery_state_v1();
    discovery.verified_objects.push(VerifiedRemoteObjectV1 {
        path: intent.remote_path.clone(),
        kind: "activation".to_string(),
        exact_bytes_hash: intent.content_hash.clone(),
        exact_bytes_hex: intent
            .exact_bytes
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect(),
        content_hash: intent.content_hash.clone(),
        commit_ref: None,
        activation_id: Some(intent.activation_id.clone()),
        fingerprint_evidence: VerifiedFingerprintEvidenceV1::Value {
            value: state.snapshot.as_ref().unwrap().legacy_fingerprint.clone(),
        },
    });
    discovery
}

pub fn recover_migration_activation_cutover_v1<C: ActivationCutoverStateStoreV1>(
    input: &MigrationStateV1,
    store: &mut C,
) -> Result<ActivationCutoverRecoveryV1> {
    let state = reconcile_migration_state_v1(input)?;
    if state.activation_receipt.is_some() {
        let discovery = activation_discovery(&state);
        let persisted = store.load()?;
        return Ok(recover_activation_cutover_v1(
            &discovery,
            persisted.as_ref(),
        ));
    }
    if matches!(
        state.status,
        MigrationStatusV1::ActivationPublishing
            | MigrationStatusV1::ActivationVerified
            | MigrationStatusV1::MigrationComplete
    ) {
        return Ok(begin_activation_cutover_recovery_v1());
    }
    let persisted = store.load()?;
    Ok(recover_activation_cutover_v1(
        &create_discovery_state_v1(),
        persisted.as_ref(),
    ))
}

#[allow(clippy::too_many_arguments)]
pub fn execute_migration_step_v1<
    R: ImmutableObjectRemoteV1,
    M: MigrationStateStoreV1,
    I: PreparedIntentStoreV1,
    P: PublishedReceiptStoreV1,
    AI: PreparedActivationIntentStoreV1,
    AP: PublishedActivationReceiptStoreV1,
>(
    input: &MigrationStateV1,
    capability: &MigrationRootExecutionCapabilityV1,
    remote: &mut R,
    migration_store: &mut M,
    intent_store: &mut I,
    receipt_store: &mut P,
    activation_intent_store: &mut AI,
    activation_receipt_store: &mut AP,
    verified_at_diagnostic: &str,
) -> Result<MigrationStateV1> {
    if capability.root_id != input.root_id
        || capability.migration_id != input.migration_id
        || remote.physical_root_id() != Some(capability.root_id.as_str())
        || remote.execution_context_identity() != capability.remote_identity
        || migration_store.authority_identity() != capability.migration_authority_identity
    {
        return Err(ProtocolError("MIGRATION_ROOT_BINDING_MISMATCH"));
    }
    let mut durable = migration_store
        .load(&capability.root_id)?
        .ok_or(ProtocolError("MIGRATION_ROOT_BINDING_MISMATCH"))?;
    if durable.migration_id != capability.migration_id {
        return Err(ProtocolError("MIGRATION_ROOT_BINDING_MISMATCH"));
    }
    durable = reconcile_migration_state_v1(&durable)?;
    if durable.status == MigrationStatusV1::RootFrozen {
        return Ok(durable);
    }
    // This fence is deliberately after the authoritative durable reload. A
    // stale caller that began at ActivationPublishing must not turn a later
    // ActivationVerified state into MigrationComplete.
    if capability.activation_publication_only
        && matches!(
            durable.status,
            MigrationStatusV1::ActivationVerified | MigrationStatusV1::MigrationComplete
        )
    {
        return Ok(durable);
    }
    let proposed = reconcile_migration_state_v1(input)?;
    if proposed.generation == durable.generation {
        validate_attempt_transition(&durable, &proposed)?;
        if proposed.status != durable.status {
            durable = persist_transition(&durable, &proposed, migration_store)?;
        }
    }
    let mut state = durable.clone();
    if matches!(
        state.status,
        MigrationStatusV1::NotStarted | MigrationStatusV1::LegacySnapshotCaptured
    ) {
        return Err(ProtocolError("migration_not_planned"));
    }
    if state.status == MigrationStatusV1::BootstrapPlanned {
        state.status = if state.stage_a.is_empty() {
            MigrationStatusV1::StageAComplete
        } else {
            MigrationStatusV1::StageAPublishing
        };
        state = persist_transition(&durable, &state, migration_store)?;
        if state.status == MigrationStatusV1::StageAComplete {
            return Ok(state);
        }
        durable = state.clone();
    }
    if state.status == MigrationStatusV1::StageAComplete {
        state.status = if state.stage_b.is_empty() {
            MigrationStatusV1::StageBComplete
        } else {
            MigrationStatusV1::StageBPublishing
        };
        state = persist_transition(&durable, &state, migration_store)?;
        return Ok(state);
    }
    if state.status == MigrationStatusV1::StageBComplete {
        state.status = MigrationStatusV1::ActivationPublishing;
        state = persist_transition(&durable, &state, migration_store)?;
        return Ok(state);
    }
    let task_location = match state.status {
        MigrationStatusV1::StageAPublishing => state
            .stage_a
            .iter()
            .position(|task| task.receipt.is_none())
            .map(|index| (true, index)),
        MigrationStatusV1::StageBPublishing => state
            .stage_b
            .iter()
            .position(|task| task.receipt.is_none())
            .map(|index| (false, index)),
        _ => None,
    };
    if let Some((stage_a, index)) = task_location {
        if !stage_a && !all_receipted(&state.stage_a) {
            return Err(ProtocolError("stage_b_before_stage_a"));
        }
        let task = if stage_a {
            &state.stage_a[index]
        } else {
            &state.stage_b[index]
        };
        // The exact bootstrap intent is durable authority even when recovery
        // finds the immutable object already present.  Receipt persistence is
        // intentionally bound to that durable object; never let an exact GET
        // bypass the local prepared-intent boundary.
        let persisted = persist_prepared_intent_before_publish_v1(&task.intent, intent_store)?;
        // A verified durable receipt survives a crash between receipt
        // persistence and the migration-state CAS. It is stronger authority
        // than another remote observation, so validate and reuse it before
        // restart recovery can issue any GET or PUT.
        let durable_receipt = receipt_store.load_verified_receipt(&task.intent.remote_path)?;
        if let (Some(task_receipt), Some(durable_receipt)) =
            (task.receipt.as_ref(), durable_receipt.as_ref())
        {
            if task_receipt != durable_receipt {
                return Err(ProtocolError("LOCAL_PUBLISHED_RECEIPT_CORRUPTION"));
            }
        }
        let result = if let Some(receipt) = task.receipt.as_ref().or(durable_receipt.as_ref()) {
            reuse_durable_published_receipt_v1(&task.intent, receipt)?
        } else {
            match migration_store.run_publish_exclusive(
                &state.root_id,
                &state.migration_id,
                state.generation,
                None,
                || publish_admitted_persisted_intent_v1(&persisted, remote, verified_at_diagnostic),
            )? {
                PublishExclusiveResultV1::Executed(value) => value,
                PublishExclusiveResultV1::Rejected(current) => {
                    return reconcile_migration_state_v1(&current)
                }
            }
        };
        match result {
            RecoverPreparedIntentResultV1::CorruptionMismatch(event) => {
                let frozen = migration_store
                    .persist_root_fatal(&state.root_id, event.freeze_class)?
                    .ok_or(ProtocolError("MIGRATION_ROOT_BINDING_MISMATCH"))?;
                return reconcile_migration_state_v1(&frozen);
            }
            RecoverPreparedIntentResultV1::AlreadyPublishedExact(receipt) => {
                persist_verified_receipt_v1(
                    &RecoverPreparedIntentResultV1::AlreadyPublishedExact(receipt.clone()),
                    &task.intent,
                    receipt_store,
                )?;
                if stage_a {
                    state.stage_a[index].receipt = Some(receipt);
                    state.stage_a[index].receipt_root_id = Some(state.root_id.clone());
                } else {
                    state.stage_b[index].receipt = Some(receipt);
                    state.stage_b[index].receipt_root_id = Some(state.root_id.clone());
                }
                let completed = if stage_a {
                    all_receipted(&state.stage_a)
                } else {
                    all_receipted(&state.stage_b)
                };
                if completed {
                    state.status = if stage_a {
                        MigrationStatusV1::StageAComplete
                    } else {
                        MigrationStatusV1::StageBComplete
                    };
                }
            }
            _ => {}
        }
        return persist_transition(&durable, &state, migration_store);
    }
    if state.status == MigrationStatusV1::ActivationPublishing {
        let intent = state.activation_intent.as_ref().unwrap().clone();
        // The frozen activation object is durable authority before any remote
        // observation. This also makes a crash before its first GET restart
        // with precisely the same activation id, path, bytes, and hash.
        let persisted =
            persist_prepared_activation_intent_before_publish_v1(&intent, activation_intent_store)?;
        // Receipt persistence precedes migration-state CAS. Reuse the
        // root-bound durable receipt before recovery can make a GET or PUT.
        let durable_receipt =
            activation_receipt_store.load_verified_activation_receipt(&intent.remote_path)?;
        if let (Some(state_receipt), Some(durable_receipt)) =
            (state.activation_receipt.as_ref(), durable_receipt.as_ref())
        {
            if state_receipt != durable_receipt {
                return Err(ProtocolError(
                    "LOCAL_PUBLISHED_ACTIVATION_RECEIPT_CORRUPTION",
                ));
            }
        }
        let result = if let Some(receipt) = state
            .activation_receipt
            .as_ref()
            .or(durable_receipt.as_ref())
        {
            reuse_durable_published_activation_receipt_v1(&intent, receipt)?
        } else {
            match migration_store.run_publish_exclusive(
                &state.root_id,
                &state.migration_id,
                state.generation,
                capability.expected_execution_identity.as_ref(),
                || {
                    publish_admitted_persisted_activation_intent_v1(
                        &persisted,
                        remote,
                        verified_at_diagnostic,
                    )
                },
            )? {
                PublishExclusiveResultV1::Executed(value) => match value {
                    PublishActivationResultV1::AlreadyPublishedExact(receipt) => {
                        RecoverActivationIntentResultV1::AlreadyPublishedExact(receipt)
                    }
                    PublishActivationResultV1::CorruptionMismatch(event) => {
                        RecoverActivationIntentResultV1::CorruptionMismatch(event)
                    }
                    PublishActivationResultV1::RemoteIndeterminate => {
                        RecoverActivationIntentResultV1::RemoteIndeterminate
                    }
                    PublishActivationResultV1::AuthOrCapabilityFailure => {
                        RecoverActivationIntentResultV1::AuthOrCapabilityFailure
                    }
                },
                PublishExclusiveResultV1::Rejected(current) => {
                    return reconcile_migration_state_v1(&current)
                }
            }
        };
        match result {
            RecoverActivationIntentResultV1::CorruptionMismatch(event) => {
                let frozen = migration_store
                    .persist_root_fatal(&state.root_id, event.freeze_class)?
                    .ok_or(ProtocolError("MIGRATION_ROOT_BINDING_MISMATCH"))?;
                return reconcile_migration_state_v1(&frozen);
            }
            RecoverActivationIntentResultV1::AlreadyPublishedExact(receipt) => {
                persist_verified_activation_receipt_v1(
                    &receipt,
                    &intent,
                    activation_receipt_store,
                )?;
                state.activation_receipt = Some(receipt);
                state.activation_receipt_root_id = Some(state.root_id.clone());
                state.status = MigrationStatusV1::ActivationVerified;
            }
            _ => {}
        }
        return persist_transition(&durable, &state, migration_store);
    }
    if state.status == MigrationStatusV1::ActivationVerified {
        let discovery = activation_discovery(&state);
        let persisted = migration_store.load_cutover_state(&state.root_id)?;
        let recovery = recover_activation_cutover_v1(&discovery, persisted.as_ref());
        let cutover = recovery
            .diagnostic_state()
            .ok_or(ProtocolError("activation_cutover_not_ready"))?;
        if !cutover.remote_s2_activated {
            return Err(ProtocolError("activation_cutover_not_ready"));
        }
        migration_store.persist_cutover_state(&state.root_id, &cutover)?;
        // The executor is intentionally publication/recovery-only.  The
        // later local writer handoff and source-guard retirement must happen
        // in the dedicated one-transaction durable finalizer.
    }
    Ok(state)
}

pub fn freeze_old_root_for_new_root_handoff_v1(
    state: &MigrationStateV1,
    fatal_codes: &[String],
) -> Result<MigrationStateV1> {
    if state.snapshot.is_none() || fatal_codes.is_empty() {
        return Err(ProtocolError("invalid_frozen_root_handoff"));
    }
    let mut frozen = state.clone();
    frozen.status = MigrationStatusV1::RootFrozen;
    let mut codes = frozen
        .root_fatal_signals
        .iter()
        .map(|value| value.code.clone())
        .chain(fatal_codes.iter().cloned())
        .collect::<Vec<_>>();
    codes.sort();
    codes.dedup();
    frozen.root_fatal_signals = codes
        .into_iter()
        .map(|code| MigrationRootFatalV1 { code })
        .collect();
    Ok(frozen)
}

pub fn create_new_root_migration_handoff_v1(
    frozen: &MigrationStateV1,
    migration_id: &str,
    new_root_id: &str,
    writer_id: &str,
    created_at: &str,
) -> Result<MigrationStateV1> {
    if frozen.status != MigrationStatusV1::RootFrozen
        || frozen.snapshot.is_none()
        || frozen.root_id == new_root_id
    {
        return Err(ProtocolError("invalid_frozen_root_handoff"));
    }
    let mut next = create_migration_state_v1(
        migration_id,
        new_root_id,
        writer_id,
        created_at,
        "new-root-bootstrap",
    )?;
    next.snapshot = frozen.snapshot.clone();
    next.status = MigrationStatusV1::LegacySnapshotCaptured;
    next.preservation_handoff = Some(PreservationHandoffV1 {
        old_root_id: frozen.root_id.clone(),
        fatal_codes: frozen
            .root_fatal_signals
            .iter()
            .map(|value| value.code.clone())
            .collect(),
    });
    Ok(next)
}

pub fn migration_projection_v1(state: &MigrationStateV1) -> Value {
    let project = |task: &MigrationCommitTaskV1| {
        let wire: Value =
            serde_json::from_slice(&task.intent.exact_bytes).expect("validated intent");
        json!({
            "writerSeq": task.intent.commit_ref.writer_seq,
            "commitId": task.intent.commit_ref.commit_id,
            "contentHash": task.intent.content_hash,
            "intentFingerprint": task.intent.intent_fingerprint,
            "mutationCount": wire["mutations"].as_array().unwrap().len(),
            "receipted": task.receipt.is_some(),
        })
    };
    json!({
        "rootId": state.root_id,
        "generation": state.generation,
        "status": state.status,
        "legacyFingerprint": state.snapshot.as_ref().map(|value| &value.legacy_fingerprint),
        "stageA": state.stage_a.iter().map(project).collect::<Vec<_>>(),
        "stageB": state.stage_b.iter().map(project).collect::<Vec<_>>(),
        "activationId": state.activation_intent.as_ref().map(|value| &value.activation_id),
        "activationContentHash": state.activation_intent.as_ref().map(|value| &value.content_hash),
        "activationIntentFingerprint": state.activation_intent.as_ref().map(|value| &value.intent_fingerprint),
        "activationVerified": state.activation_receipt.is_some(),
        "fatalCodes": state.root_fatal_signals.iter().map(|value| &value.code).collect::<Vec<_>>(),
        "sourceType": state.source_type,
        "preservationHandoff": state.preservation_handoff,
    })
}
