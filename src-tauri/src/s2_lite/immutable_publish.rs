use super::canonical::{
    jcs_bytes, parse_writer_seq, sha256_hex, validate_canonical_uuid_v4, validate_commit_ref,
    validate_timestamp, ProtocolError, Result,
};
use super::causal::decode_frozen_wire_commit_v1;
use super::types::CommitRef;
use serde::{Deserialize, Serialize};
use serde_json::json;

pub const COMMIT_SEGMENT_SIZE_V1: u64 = 256;
pub const WRITER_SEQ_PATH_WIDTH_V1: usize = 20;
pub const SEGMENT_NAME_WIDTH_V1: usize = 14;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PreparedIntentV1 {
    pub intent_version: u8,
    pub object_kind: String,
    pub remote_path: String,
    pub exact_bytes: Vec<u8>,
    pub content_hash: String,
    pub commit_ref: CommitRef,
    pub intent_fingerprint: String,
    pub created_locally_at_diagnostic: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RemotePublishedReceiptV1 {
    pub receipt_version: u8,
    pub remote_path: String,
    pub content_hash: String,
    pub commit_ref: CommitRef,
    pub prepared_intent_fingerprint: String,
    pub verified_exact_bytes_hash: String,
    pub verified_at_diagnostic: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PreparedActivationIntentV1 {
    pub intent_version: u8,
    pub object_kind: String,
    pub remote_path: String,
    pub exact_bytes: Vec<u8>,
    pub content_hash: String,
    pub activation_id: String,
    pub intent_fingerprint: String,
    pub created_locally_at_diagnostic: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PublishedActivationReceiptV1 {
    pub receipt_version: u8,
    pub remote_path: String,
    pub content_hash: String,
    pub activation_id: String,
    pub prepared_intent_fingerprint: String,
    pub verified_exact_bytes_hash: String,
    pub verified_at_diagnostic: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PublishActivationResultV1 {
    AlreadyPublishedExact(PublishedActivationReceiptV1),
    CorruptionMismatch(ImmutablePathMismatchEventV1),
    RemoteIndeterminate,
    AuthOrCapabilityFailure,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RemoteExactGetResultV1 {
    DefinitelyPresent(Vec<u8>),
    DefinitelyAbsent,
    Indeterminate,
    AuthOrCapabilityFailure,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RemotePutResultV1 {
    Success,
    Indeterminate,
    AuthOrCapabilityFailure,
}

pub trait ImmutableObjectRemoteV1 {
    fn execution_context_identity(&self) -> u64 {
        0
    }

    fn physical_root_id(&self) -> Option<&str> {
        None
    }

    /// Prepares the fixed top-level collection needed by normal S2 discovery.
    /// This is a root-authorized infrastructure mutation, not part of the
    /// read-only `DiscoveryRemoteV1` surface. Production WebDAV remotes create
    /// or strictly verify only the canonical `writers` collection.
    fn prepare_normal_s2_discovery_infrastructure(&mut self) -> RemotePutResultV1 {
        RemotePutResultV1::Success
    }

    /// Publication-only infrastructure admission for one already-validated
    /// immutable object path. Read-only recovery and discovery must never use
    /// this hook. The default keeps deterministic non-WebDAV remotes side
    /// effect free while the production adapter provisions the canonical DAV
    /// parent chain.
    fn prepare_immutable_parent_collections(&mut self, _remote_path: &str) -> RemotePutResultV1 {
        RemotePutResultV1::Success
    }

    fn get_exact(&mut self, remote_path: &str) -> RemoteExactGetResultV1;
    fn put_exact(
        &mut self,
        remote_path: &str,
        exact_bytes: &[u8],
        if_none_match_star: bool,
    ) -> RemotePutResultV1;
}

pub trait PreparedIntentStoreV1 {
    fn persist(&mut self, intent: &PreparedIntentV1) -> Result<()>;
}

pub trait PublishedReceiptStoreV1 {
    fn persist(&mut self, receipt: &RemotePublishedReceiptV1) -> Result<()>;

    /// Returns the durable, root-bound receipt for an already prepared exact
    /// object. Implementations that do not own durable receipt storage retain
    /// the historical recovery behavior; the production SQLite store must
    /// override this so a receipt committed before a later state CAS remains
    /// publication authority after restart.
    fn load_verified_receipt(
        &mut self,
        _remote_path: &str,
    ) -> Result<Option<RemotePublishedReceiptV1>> {
        Ok(None)
    }
}

pub trait PreparedActivationIntentStoreV1 {
    fn persist(&mut self, intent: &PreparedActivationIntentV1) -> Result<()>;
}

pub trait PublishedActivationReceiptStoreV1 {
    fn persist(&mut self, receipt: &PublishedActivationReceiptV1) -> Result<()>;

    /// See `PublishedReceiptStoreV1::load_verified_receipt`. A durable
    /// activation receipt is authoritative after a crash before the
    /// migration-state compare-and-swap attaches it.
    fn load_verified_activation_receipt(
        &mut self,
        _remote_path: &str,
    ) -> Result<Option<PublishedActivationReceiptV1>> {
        Ok(None)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PersistedPreparedIntentV1 {
    intent: PreparedIntentV1,
    persisted_fingerprint: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PersistedPreparedActivationIntentV1 {
    intent: PreparedActivationIntentV1,
    persisted_fingerprint: String,
}

impl PersistedPreparedIntentV1 {
    pub fn intent_fingerprint(&self) -> &str {
        &self.persisted_fingerprint
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImmutablePathMismatchEventV1 {
    pub code: &'static str,
    pub freeze_class: &'static str,
    pub remote_path: String,
    pub expected_content_hash: String,
    pub observed_content_hash: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RecoverPreparedIntentResultV1 {
    AlreadyPublishedExact(RemotePublishedReceiptV1),
    RetryPublishExact,
    CorruptionMismatch(ImmutablePathMismatchEventV1),
    RemoteIndeterminate,
    AuthOrCapabilityFailure,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ImmutablePublishStateV1 {
    Prepared,
    PutAttempted,
    VerifyRemote,
    VerifiedPublished,
    CorruptionMismatch,
    RemoteIndeterminate,
    AuthOrCapabilityFailure,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ImmutablePublishEventV1 {
    PutStarted,
    VerifyStarted,
    RemoteExact,
    RemoteAbsent,
    RemoteMismatch,
    RemoteIndeterminate,
    RemoteAuthOrCapabilityFailure,
}

fn bytes_to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn intent_fingerprint(intent: &PreparedIntentV1) -> Result<String> {
    let core = json!({
        "domain": "watchtracker-s2-lite-prepared-intent-fingerprint-v1",
        "intentVersion": intent.intent_version,
        "objectKind": intent.object_kind,
        "remotePath": intent.remote_path,
        "exactBytesHex": bytes_to_hex(&intent.exact_bytes),
        "contentHash": intent.content_hash,
        "commitRef": intent.commit_ref,
    });
    Ok(sha256_hex(&jcs_bytes(&core)?))
}

pub fn build_commit_remote_path_v1(commit_ref: &CommitRef) -> Result<String> {
    validate_commit_ref(commit_ref)?;
    let writer_seq = parse_writer_seq(&commit_ref.writer_seq)?;
    let segment_index = (writer_seq - 1) / COMMIT_SEGMENT_SIZE_V1;
    let segment_name = format!("{segment_index:0SEGMENT_NAME_WIDTH_V1$x}");
    if segment_name.len() != SEGMENT_NAME_WIDTH_V1 {
        return Err(ProtocolError("segment_name_overflow"));
    }
    let writer_seq_20 = format!("{writer_seq:0WRITER_SEQ_PATH_WIDTH_V1$}");
    if writer_seq_20.len() != WRITER_SEQ_PATH_WIDTH_V1 {
        return Err(ProtocolError("writer_seq_path_overflow"));
    }
    Ok(format!(
        "writers/{}/segments/{}/{}--{}--{}.json",
        commit_ref.writer_id,
        segment_name,
        writer_seq_20,
        commit_ref.commit_id,
        commit_ref.content_hash
    ))
}

pub fn prepare_commit_intent_v1(
    exact_commit_bytes: &[u8],
    created_locally_at_diagnostic: &str,
) -> Result<PreparedIntentV1> {
    validate_timestamp(created_locally_at_diagnostic)?;
    let commit = decode_frozen_wire_commit_v1(exact_commit_bytes)?;
    let commit_ref = commit.commit_ref();
    let mut intent = PreparedIntentV1 {
        intent_version: 1,
        object_kind: "commit".to_string(),
        remote_path: build_commit_remote_path_v1(&commit_ref)?,
        exact_bytes: exact_commit_bytes.to_vec(),
        content_hash: commit_ref.content_hash.clone(),
        commit_ref,
        intent_fingerprint: String::new(),
        created_locally_at_diagnostic: created_locally_at_diagnostic.to_string(),
    };
    intent.intent_fingerprint = intent_fingerprint(&intent)?;
    Ok(intent)
}

pub fn validate_prepared_intent_v1(intent: &PreparedIntentV1) -> Result<()> {
    let validate = || -> Result<()> {
        if intent.intent_version != 1 || intent.object_kind != "commit" {
            return Err(ProtocolError("invalid_prepared_intent"));
        }
        validate_timestamp(&intent.created_locally_at_diagnostic)?;
        validate_commit_ref(&intent.commit_ref)?;
        if intent.remote_path != build_commit_remote_path_v1(&intent.commit_ref)? {
            return Err(ProtocolError("invalid_prepared_intent"));
        }
        let exact_hash = sha256_hex(&intent.exact_bytes);
        if exact_hash != intent.content_hash || exact_hash != intent.commit_ref.content_hash {
            return Err(ProtocolError("invalid_prepared_intent"));
        }
        let decoded = decode_frozen_wire_commit_v1(&intent.exact_bytes)?;
        if decoded.commit_ref() != intent.commit_ref
            || intent.intent_fingerprint != intent_fingerprint(intent)?
        {
            return Err(ProtocolError("invalid_prepared_intent"));
        }
        Ok(())
    };
    validate().map_err(|_| ProtocolError("LOCAL_PREPARED_INTENT_CORRUPTION"))
}

fn make_receipt_v1(
    intent: &PreparedIntentV1,
    verified_at_diagnostic: &str,
) -> Result<RemotePublishedReceiptV1> {
    validate_timestamp(verified_at_diagnostic)?;
    Ok(RemotePublishedReceiptV1 {
        receipt_version: 1,
        remote_path: intent.remote_path.clone(),
        content_hash: intent.content_hash.clone(),
        commit_ref: intent.commit_ref.clone(),
        prepared_intent_fingerprint: intent.intent_fingerprint.clone(),
        verified_exact_bytes_hash: sha256_hex(&intent.exact_bytes),
        verified_at_diagnostic: verified_at_diagnostic.to_string(),
    })
}

pub fn validate_published_receipt_v1(
    receipt: &RemotePublishedReceiptV1,
    intent: &PreparedIntentV1,
) -> Result<()> {
    let validate = || -> Result<()> {
        validate_prepared_intent_v1(intent)?;
        validate_timestamp(&receipt.verified_at_diagnostic)?;
        validate_commit_ref(&receipt.commit_ref)?;
        if receipt.receipt_version != 1
            || receipt.remote_path != intent.remote_path
            || receipt.content_hash != intent.content_hash
            || receipt.verified_exact_bytes_hash != intent.content_hash
            || receipt.prepared_intent_fingerprint != intent.intent_fingerprint
            || receipt.commit_ref != intent.commit_ref
        {
            return Err(ProtocolError("invalid_published_receipt"));
        }
        Ok(())
    };
    validate().map_err(|_| ProtocolError("LOCAL_PUBLISHED_RECEIPT_CORRUPTION"))
}

pub fn advance_immutable_publish_state_v1(
    state: ImmutablePublishStateV1,
    event: ImmutablePublishEventV1,
) -> Result<ImmutablePublishStateV1> {
    use ImmutablePublishEventV1 as Event;
    use ImmutablePublishStateV1 as State;
    match (state, event) {
        (State::Prepared, Event::PutStarted) => Ok(State::PutAttempted),
        (State::Prepared | State::PutAttempted, Event::VerifyStarted) => Ok(State::VerifyRemote),
        (State::VerifyRemote, Event::RemoteExact) => Ok(State::VerifiedPublished),
        (State::VerifyRemote, Event::RemoteAbsent) => Ok(State::Prepared),
        (State::VerifyRemote, Event::RemoteMismatch) => Ok(State::CorruptionMismatch),
        (State::VerifyRemote, Event::RemoteIndeterminate) => Ok(State::RemoteIndeterminate),
        (State::VerifyRemote, Event::RemoteAuthOrCapabilityFailure) => {
            Ok(State::AuthOrCapabilityFailure)
        }
        _ => Err(ProtocolError("invalid_immutable_publish_transition")),
    }
}

fn classify_exact_get_v1(
    intent: &PreparedIntentV1,
    fetched: RemoteExactGetResultV1,
    verified_at_diagnostic: &str,
) -> Result<RecoverPreparedIntentResultV1> {
    match fetched {
        RemoteExactGetResultV1::DefinitelyAbsent => {
            Ok(RecoverPreparedIntentResultV1::RetryPublishExact)
        }
        RemoteExactGetResultV1::Indeterminate => {
            Ok(RecoverPreparedIntentResultV1::RemoteIndeterminate)
        }
        RemoteExactGetResultV1::AuthOrCapabilityFailure => {
            Ok(RecoverPreparedIntentResultV1::AuthOrCapabilityFailure)
        }
        RemoteExactGetResultV1::DefinitelyPresent(bytes) if bytes == intent.exact_bytes => {
            Ok(RecoverPreparedIntentResultV1::AlreadyPublishedExact(
                make_receipt_v1(intent, verified_at_diagnostic)?,
            ))
        }
        RemoteExactGetResultV1::DefinitelyPresent(bytes) => Ok(
            RecoverPreparedIntentResultV1::CorruptionMismatch(ImmutablePathMismatchEventV1 {
                code: "REMOTE_IMMUTABLE_PATH_CONTENT_MISMATCH",
                freeze_class: "SYNC_ROOT_FROZEN_CORRUPTION",
                remote_path: intent.remote_path.clone(),
                expected_content_hash: intent.content_hash.clone(),
                observed_content_hash: sha256_hex(&bytes),
            }),
        ),
    }
}

pub fn recover_prepared_intent_v1<R: ImmutableObjectRemoteV1>(
    intent: &PreparedIntentV1,
    remote: &mut R,
    verified_at_diagnostic: &str,
) -> Result<RecoverPreparedIntentResultV1> {
    validate_prepared_intent_v1(intent)?;
    validate_timestamp(verified_at_diagnostic)?;
    classify_exact_get_v1(
        intent,
        remote.get_exact(&intent.remote_path),
        verified_at_diagnostic,
    )
}

fn publish_prepared_intent_v1<R: ImmutableObjectRemoteV1>(
    intent: &PreparedIntentV1,
    remote: &mut R,
    verified_at_diagnostic: &str,
) -> Result<RecoverPreparedIntentResultV1> {
    match remote.prepare_immutable_parent_collections(&intent.remote_path) {
        RemotePutResultV1::Success => {}
        RemotePutResultV1::Indeterminate => {
            return Ok(RecoverPreparedIntentResultV1::RemoteIndeterminate)
        }
        RemotePutResultV1::AuthOrCapabilityFailure => {
            return Ok(RecoverPreparedIntentResultV1::AuthOrCapabilityFailure)
        }
    }
    let preflight = recover_prepared_intent_v1(intent, remote, verified_at_diagnostic)?;
    if !matches!(preflight, RecoverPreparedIntentResultV1::RetryPublishExact) {
        return Ok(preflight);
    }
    let put_result = remote.put_exact(&intent.remote_path, &intent.exact_bytes, true);
    let verification = classify_exact_get_v1(
        intent,
        remote.get_exact(&intent.remote_path),
        verified_at_diagnostic,
    )?;
    if matches!(
        verification,
        RecoverPreparedIntentResultV1::RetryPublishExact
    ) && put_result == RemotePutResultV1::AuthOrCapabilityFailure
    {
        return Ok(RecoverPreparedIntentResultV1::AuthOrCapabilityFailure);
    }
    Ok(verification)
}

pub fn persist_prepared_intent_before_publish_v1<S: PreparedIntentStoreV1>(
    intent: &PreparedIntentV1,
    store: &mut S,
) -> Result<PersistedPreparedIntentV1> {
    validate_prepared_intent_v1(intent)?;
    store.persist(intent)?;
    Ok(PersistedPreparedIntentV1 {
        intent: intent.clone(),
        persisted_fingerprint: intent.intent_fingerprint.clone(),
    })
}

pub fn publish_persisted_intent_v1<R: ImmutableObjectRemoteV1>(
    persisted: &PersistedPreparedIntentV1,
    remote: &mut R,
    verified_at_diagnostic: &str,
) -> Result<RecoverPreparedIntentResultV1> {
    validate_prepared_intent_v1(&persisted.intent)?;
    if persisted.persisted_fingerprint != persisted.intent.intent_fingerprint {
        return Err(ProtocolError("LOCAL_PREPARED_INTENT_CORRUPTION"));
    }
    publish_prepared_intent_v1(&persisted.intent, remote, verified_at_diagnostic)
}

pub fn publish_admitted_persisted_intent_v1<R: ImmutableObjectRemoteV1>(
    persisted: &PersistedPreparedIntentV1,
    remote: &mut R,
    verified_at_diagnostic: &str,
) -> Result<RecoverPreparedIntentResultV1> {
    validate_prepared_intent_v1(&persisted.intent)?;
    validate_timestamp(verified_at_diagnostic)?;
    if persisted.persisted_fingerprint != persisted.intent.intent_fingerprint {
        return Err(ProtocolError("LOCAL_PREPARED_INTENT_CORRUPTION"));
    }
    publish_prepared_intent_v1(&persisted.intent, remote, verified_at_diagnostic)
}

pub fn restart_durable_publish_v1<R: ImmutableObjectRemoteV1>(
    durable_intent: &PreparedIntentV1,
    durable_receipt: Option<&RemotePublishedReceiptV1>,
    remote: &mut R,
    verified_at_diagnostic: &str,
) -> Result<RecoverPreparedIntentResultV1> {
    validate_prepared_intent_v1(durable_intent)?;
    if let Some(receipt) = durable_receipt {
        validate_published_receipt_v1(receipt, durable_intent)?;
        return Ok(RecoverPreparedIntentResultV1::AlreadyPublishedExact(
            receipt.clone(),
        ));
    }
    recover_prepared_intent_v1(durable_intent, remote, verified_at_diagnostic)
}

/// Validates and reuses an already durable receipt without making any remote
/// observation. Mutating production paths use this before admission; absent
/// receipts must enter admission before their first publication-decision GET.
pub fn reuse_durable_published_receipt_v1(
    intent: &PreparedIntentV1,
    receipt: &RemotePublishedReceiptV1,
) -> Result<RecoverPreparedIntentResultV1> {
    validate_prepared_intent_v1(intent)?;
    validate_published_receipt_v1(receipt, intent)?;
    Ok(RecoverPreparedIntentResultV1::AlreadyPublishedExact(
        receipt.clone(),
    ))
}

pub fn persist_verified_receipt_v1<S: PublishedReceiptStoreV1>(
    result: &RecoverPreparedIntentResultV1,
    intent: &PreparedIntentV1,
    store: &mut S,
) -> Result<()> {
    let RecoverPreparedIntentResultV1::AlreadyPublishedExact(receipt) = result else {
        return Err(ProtocolError("receipt_requires_exact_remote_verification"));
    };
    validate_published_receipt_v1(receipt, intent)?;
    store.persist(receipt)
}

fn activation_intent_fingerprint(intent: &PreparedActivationIntentV1) -> Result<String> {
    let core = json!({
        "domain": "watchtracker-s2-lite-prepared-activation-intent-v1",
        "intentVersion": intent.intent_version,
        "objectKind": intent.object_kind,
        "remotePath": intent.remote_path,
        "exactBytesHex": bytes_to_hex(&intent.exact_bytes),
        "contentHash": intent.content_hash,
        "activationId": intent.activation_id,
    });
    Ok(sha256_hex(&jcs_bytes(&core)?))
}

pub fn prepare_activation_intent_v1(
    activation_id: &str,
    exact_activation_bytes: &[u8],
    created_locally_at_diagnostic: &str,
) -> Result<PreparedActivationIntentV1> {
    validate_canonical_uuid_v4(activation_id)?;
    validate_timestamp(created_locally_at_diagnostic)?;
    let content_hash = sha256_hex(exact_activation_bytes);
    let mut intent = PreparedActivationIntentV1 {
        intent_version: 1,
        object_kind: "activation".to_string(),
        remote_path: format!("activations/{activation_id}--{content_hash}.json"),
        exact_bytes: exact_activation_bytes.to_vec(),
        content_hash,
        activation_id: activation_id.to_string(),
        intent_fingerprint: String::new(),
        created_locally_at_diagnostic: created_locally_at_diagnostic.to_string(),
    };
    intent.intent_fingerprint = activation_intent_fingerprint(&intent)?;
    Ok(intent)
}

pub fn validate_prepared_activation_intent_v1(intent: &PreparedActivationIntentV1) -> Result<()> {
    let validate = || -> Result<()> {
        if intent.intent_version != 1 || intent.object_kind != "activation" {
            return Err(ProtocolError("invalid_activation_intent"));
        }
        validate_canonical_uuid_v4(&intent.activation_id)?;
        validate_timestamp(&intent.created_locally_at_diagnostic)?;
        let hash = sha256_hex(&intent.exact_bytes);
        if hash != intent.content_hash
            || intent.remote_path != format!("activations/{}--{}.json", intent.activation_id, hash)
            || intent.intent_fingerprint != activation_intent_fingerprint(intent)?
        {
            return Err(ProtocolError("invalid_activation_intent"));
        }
        Ok(())
    };
    validate().map_err(|_| ProtocolError("LOCAL_PREPARED_INTENT_CORRUPTION"))
}

pub fn validate_published_activation_receipt_v1(
    receipt: &PublishedActivationReceiptV1,
    intent: &PreparedActivationIntentV1,
) -> Result<()> {
    let validate = || -> Result<()> {
        validate_prepared_activation_intent_v1(intent)?;
        validate_timestamp(&receipt.verified_at_diagnostic)?;
        validate_canonical_uuid_v4(&receipt.activation_id)?;
        if receipt.receipt_version != 1
            || receipt.remote_path != intent.remote_path
            || receipt.content_hash != intent.content_hash
            || receipt.activation_id != intent.activation_id
            || receipt.prepared_intent_fingerprint != intent.intent_fingerprint
            || receipt.verified_exact_bytes_hash != intent.content_hash
        {
            return Err(ProtocolError("invalid_activation_receipt"));
        }
        Ok(())
    };
    validate().map_err(|_| ProtocolError("LOCAL_PUBLISHED_RECEIPT_CORRUPTION"))
}

fn publish_prepared_activation_intent_v1<R: ImmutableObjectRemoteV1>(
    intent: &PreparedActivationIntentV1,
    remote: &mut R,
    verified_at_diagnostic: &str,
) -> Result<PublishActivationResultV1> {
    validate_prepared_activation_intent_v1(intent)?;
    validate_timestamp(verified_at_diagnostic)?;
    match remote.prepare_immutable_parent_collections(&intent.remote_path) {
        RemotePutResultV1::Success => {}
        RemotePutResultV1::Indeterminate => {
            return Ok(PublishActivationResultV1::RemoteIndeterminate)
        }
        RemotePutResultV1::AuthOrCapabilityFailure => {
            return Ok(PublishActivationResultV1::AuthOrCapabilityFailure)
        }
    }
    let mut fetched = remote.get_exact(&intent.remote_path);
    if fetched == RemoteExactGetResultV1::DefinitelyAbsent {
        let put = remote.put_exact(&intent.remote_path, &intent.exact_bytes, true);
        fetched = remote.get_exact(&intent.remote_path);
        if fetched == RemoteExactGetResultV1::DefinitelyAbsent
            && put == RemotePutResultV1::AuthOrCapabilityFailure
        {
            return Ok(PublishActivationResultV1::AuthOrCapabilityFailure);
        }
    }
    match fetched {
        RemoteExactGetResultV1::Indeterminate | RemoteExactGetResultV1::DefinitelyAbsent => {
            Ok(PublishActivationResultV1::RemoteIndeterminate)
        }
        RemoteExactGetResultV1::AuthOrCapabilityFailure => {
            Ok(PublishActivationResultV1::AuthOrCapabilityFailure)
        }
        RemoteExactGetResultV1::DefinitelyPresent(bytes) if bytes == intent.exact_bytes => Ok(
            PublishActivationResultV1::AlreadyPublishedExact(PublishedActivationReceiptV1 {
                receipt_version: 1,
                remote_path: intent.remote_path.clone(),
                content_hash: intent.content_hash.clone(),
                activation_id: intent.activation_id.clone(),
                prepared_intent_fingerprint: intent.intent_fingerprint.clone(),
                verified_exact_bytes_hash: intent.content_hash.clone(),
                verified_at_diagnostic: verified_at_diagnostic.to_string(),
            }),
        ),
        RemoteExactGetResultV1::DefinitelyPresent(bytes) => Ok(
            PublishActivationResultV1::CorruptionMismatch(ImmutablePathMismatchEventV1 {
                code: "REMOTE_IMMUTABLE_PATH_CONTENT_MISMATCH",
                freeze_class: "SYNC_ROOT_FROZEN_CORRUPTION",
                remote_path: intent.remote_path.clone(),
                expected_content_hash: intent.content_hash.clone(),
                observed_content_hash: sha256_hex(&bytes),
            }),
        ),
    }
}

pub fn persist_prepared_activation_intent_before_publish_v1<S: PreparedActivationIntentStoreV1>(
    intent: &PreparedActivationIntentV1,
    store: &mut S,
) -> Result<PersistedPreparedActivationIntentV1> {
    validate_prepared_activation_intent_v1(intent)?;
    store.persist(intent)?;
    validate_prepared_activation_intent_v1(intent)?;
    Ok(PersistedPreparedActivationIntentV1 {
        intent: intent.clone(),
        persisted_fingerprint: intent.intent_fingerprint.clone(),
    })
}

pub fn publish_persisted_activation_intent_v1<R: ImmutableObjectRemoteV1>(
    persisted: &PersistedPreparedActivationIntentV1,
    remote: &mut R,
    verified_at_diagnostic: &str,
) -> Result<PublishActivationResultV1> {
    validate_prepared_activation_intent_v1(&persisted.intent)?;
    if persisted.persisted_fingerprint != persisted.intent.intent_fingerprint {
        return Err(ProtocolError("LOCAL_PREPARED_INTENT_CORRUPTION"));
    }
    publish_prepared_activation_intent_v1(&persisted.intent, remote, verified_at_diagnostic)
}

pub fn publish_admitted_persisted_activation_intent_v1<R: ImmutableObjectRemoteV1>(
    persisted: &PersistedPreparedActivationIntentV1,
    remote: &mut R,
    verified_at_diagnostic: &str,
) -> Result<PublishActivationResultV1> {
    validate_prepared_activation_intent_v1(&persisted.intent)?;
    validate_timestamp(verified_at_diagnostic)?;
    if persisted.persisted_fingerprint != persisted.intent.intent_fingerprint {
        return Err(ProtocolError("LOCAL_PREPARED_INTENT_CORRUPTION"));
    }
    publish_prepared_activation_intent_v1(&persisted.intent, remote, verified_at_diagnostic)
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RecoverActivationIntentResultV1 {
    AlreadyPublishedExact(PublishedActivationReceiptV1),
    RetryPublishExact,
    CorruptionMismatch(ImmutablePathMismatchEventV1),
    RemoteIndeterminate,
    AuthOrCapabilityFailure,
}

pub fn restart_durable_activation_publish_v1<R: ImmutableObjectRemoteV1>(
    intent: &PreparedActivationIntentV1,
    receipt: Option<&PublishedActivationReceiptV1>,
    remote: &mut R,
    verified_at_diagnostic: &str,
) -> Result<RecoverActivationIntentResultV1> {
    validate_prepared_activation_intent_v1(intent)?;
    if let Some(receipt) = receipt {
        validate_published_activation_receipt_v1(receipt, intent)?;
        return Ok(RecoverActivationIntentResultV1::AlreadyPublishedExact(
            receipt.clone(),
        ));
    }
    match remote.get_exact(&intent.remote_path) {
        RemoteExactGetResultV1::DefinitelyAbsent => {
            Ok(RecoverActivationIntentResultV1::RetryPublishExact)
        }
        RemoteExactGetResultV1::Indeterminate => {
            Ok(RecoverActivationIntentResultV1::RemoteIndeterminate)
        }
        RemoteExactGetResultV1::AuthOrCapabilityFailure => {
            Ok(RecoverActivationIntentResultV1::AuthOrCapabilityFailure)
        }
        RemoteExactGetResultV1::DefinitelyPresent(bytes) if bytes == intent.exact_bytes => {
            validate_timestamp(verified_at_diagnostic)?;
            Ok(RecoverActivationIntentResultV1::AlreadyPublishedExact(
                PublishedActivationReceiptV1 {
                    receipt_version: 1,
                    remote_path: intent.remote_path.clone(),
                    content_hash: intent.content_hash.clone(),
                    activation_id: intent.activation_id.clone(),
                    prepared_intent_fingerprint: intent.intent_fingerprint.clone(),
                    verified_exact_bytes_hash: intent.content_hash.clone(),
                    verified_at_diagnostic: verified_at_diagnostic.to_string(),
                },
            ))
        }
        RemoteExactGetResultV1::DefinitelyPresent(bytes) => Ok(
            RecoverActivationIntentResultV1::CorruptionMismatch(ImmutablePathMismatchEventV1 {
                code: "REMOTE_IMMUTABLE_PATH_CONTENT_MISMATCH",
                freeze_class: "SYNC_ROOT_FROZEN_CORRUPTION",
                remote_path: intent.remote_path.clone(),
                expected_content_hash: intent.content_hash.clone(),
                observed_content_hash: sha256_hex(&bytes),
            }),
        ),
    }
}

/// Activation equivalent of `reuse_durable_published_receipt_v1`.
pub fn reuse_durable_published_activation_receipt_v1(
    intent: &PreparedActivationIntentV1,
    receipt: &PublishedActivationReceiptV1,
) -> Result<RecoverActivationIntentResultV1> {
    validate_prepared_activation_intent_v1(intent)?;
    validate_published_activation_receipt_v1(receipt, intent)?;
    Ok(RecoverActivationIntentResultV1::AlreadyPublishedExact(
        receipt.clone(),
    ))
}

pub fn persist_verified_activation_receipt_v1<S: PublishedActivationReceiptStoreV1>(
    receipt: &PublishedActivationReceiptV1,
    intent: &PreparedActivationIntentV1,
    store: &mut S,
) -> Result<()> {
    validate_published_activation_receipt_v1(receipt, intent)?;
    store.persist(receipt)?;
    validate_published_activation_receipt_v1(receipt, intent)
}
