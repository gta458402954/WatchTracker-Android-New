use super::canonical::{sha256_hex, ProtocolError};
use super::immutable_publish::{
    advance_immutable_publish_state_v1, build_commit_remote_path_v1,
    persist_prepared_intent_before_publish_v1, persist_verified_receipt_v1,
    prepare_commit_intent_v1, publish_persisted_intent_v1, recover_prepared_intent_v1,
    restart_durable_publish_v1, validate_prepared_intent_v1, validate_published_receipt_v1,
    ImmutableObjectRemoteV1, ImmutablePublishEventV1, ImmutablePublishStateV1,
    PreparedIntentStoreV1, PreparedIntentV1, PublishedReceiptStoreV1,
    RecoverPreparedIntentResultV1, RemoteExactGetResultV1, RemotePublishedReceiptV1,
    RemotePutResultV1,
};
use super::types::CommitRef;
use base64::Engine;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::fs;

const VERIFIED_AT: &str = "2026-09-08T05:00:00.000Z";
type IntentCorruption = Box<dyn Fn(&mut PreparedIntentV1)>;
type ReceiptCorruption = Box<dyn Fn(&mut RemotePublishedReceiptV1)>;

fn fixture() -> Value {
    serde_json::from_str(
        &fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../contracts/s2-lite/v1/publish-golden-v1.json"
        ))
        .unwrap(),
    )
    .unwrap()
}

fn exact_bytes(fixture: &Value) -> Vec<u8> {
    base64::engine::general_purpose::STANDARD
        .decode(
            fixture["preparedIntent"]["exactBytesBase64"]
                .as_str()
                .unwrap(),
        )
        .unwrap()
}

fn prepared(fixture: &Value) -> PreparedIntentV1 {
    prepare_commit_intent_v1(
        &exact_bytes(fixture),
        fixture["preparedIntent"]["createdLocallyAtDiagnostic"]
            .as_str()
            .unwrap(),
    )
    .unwrap()
}

fn path_ref(fixture: &Value, writer_seq: &str) -> CommitRef {
    CommitRef {
        writer_id: fixture["pathIdentity"]["writerId"]
            .as_str()
            .unwrap()
            .to_string(),
        writer_seq: writer_seq.to_string(),
        commit_id: fixture["pathIdentity"]["commitId"]
            .as_str()
            .unwrap()
            .to_string(),
        content_hash: fixture["pathIdentity"]["contentHash"]
            .as_str()
            .unwrap()
            .to_string(),
    }
}

#[derive(Default)]
struct AdversarialRemote {
    objects: HashMap<String, Vec<u8>>,
    put_calls: Vec<(String, Vec<u8>, bool)>,
    get_calls: Vec<String>,
    put_mode: &'static str,
    get_mode: &'static str,
    prepare_mode: &'static str,
    operations: Vec<&'static str>,
    delayed_reads: usize,
}

impl ImmutableObjectRemoteV1 for AdversarialRemote {
    fn get_exact(&mut self, remote_path: &str) -> RemoteExactGetResultV1 {
        self.operations.push("GET");
        self.get_calls.push(remote_path.to_string());
        if self.get_mode == "indeterminate" {
            return RemoteExactGetResultV1::Indeterminate;
        }
        if self.get_mode == "auth" {
            return RemoteExactGetResultV1::AuthOrCapabilityFailure;
        }
        if self.delayed_reads > 0 {
            self.delayed_reads -= 1;
            return RemoteExactGetResultV1::Indeterminate;
        }
        self.objects
            .get(remote_path)
            .map_or(RemoteExactGetResultV1::DefinitelyAbsent, |bytes| {
                RemoteExactGetResultV1::DefinitelyPresent(bytes.clone())
            })
    }

    fn prepare_immutable_parent_collections(&mut self, _remote_path: &str) -> RemotePutResultV1 {
        self.operations.push("PREPARE");
        match self.prepare_mode {
            "indeterminate" => RemotePutResultV1::Indeterminate,
            "auth" => RemotePutResultV1::AuthOrCapabilityFailure,
            _ => RemotePutResultV1::Success,
        }
    }

    fn put_exact(
        &mut self,
        remote_path: &str,
        exact_bytes: &[u8],
        if_none_match_star: bool,
    ) -> RemotePutResultV1 {
        self.operations.push("PUT");
        self.put_calls.push((
            remote_path.to_string(),
            exact_bytes.to_vec(),
            if_none_match_star,
        ));
        if self.put_mode == "timeout-before-store" {
            return RemotePutResultV1::Indeterminate;
        }
        if self.put_mode == "auth" {
            return RemotePutResultV1::AuthOrCapabilityFailure;
        }
        // Deliberately ignores the condition and overwrites the same path.
        self.objects
            .insert(remote_path.to_string(), exact_bytes.to_vec());
        match self.put_mode {
            "store-then-timeout" => RemotePutResultV1::Indeterminate,
            _ => RemotePutResultV1::Success,
        }
    }
}

#[test]
fn publication_prepares_before_fresh_exact_preflight_and_fails_closed() {
    let fixture = fixture();
    let intent = prepared(&fixture);

    let mut fresh = AdversarialRemote::default();
    assert_eq!(
        outcome_name(&publish_persisted(&intent, &mut fresh)),
        "AlreadyPublishedExact"
    );
    assert_eq!(fresh.operations, ["PREPARE", "GET", "PUT", "GET"]);
    assert_eq!(fresh.put_calls.len(), 1);

    let mut exact = AdversarialRemote::default();
    exact
        .objects
        .insert(intent.remote_path.clone(), intent.exact_bytes.clone());
    assert_eq!(
        outcome_name(&publish_persisted(&intent, &mut exact)),
        "AlreadyPublishedExact"
    );
    assert_eq!(exact.operations, ["PREPARE", "GET"]);
    assert!(exact.put_calls.is_empty());

    for (mode, outcome) in [
        ("indeterminate", "RemoteIndeterminate"),
        ("auth", "AuthOrCapabilityFailure"),
    ] {
        let mut blocked = AdversarialRemote {
            prepare_mode: mode,
            ..Default::default()
        };
        assert_eq!(
            outcome_name(&publish_persisted(&intent, &mut blocked)),
            outcome
        );
        assert_eq!(blocked.operations, ["PREPARE"]);
        assert!(blocked.put_calls.is_empty());
    }
}

fn outcome_name(result: &RecoverPreparedIntentResultV1) -> &'static str {
    match result {
        RecoverPreparedIntentResultV1::AlreadyPublishedExact(_) => "AlreadyPublishedExact",
        RecoverPreparedIntentResultV1::RetryPublishExact => "RetryPublishExact",
        RecoverPreparedIntentResultV1::CorruptionMismatch(_) => "CorruptionMismatch",
        RecoverPreparedIntentResultV1::RemoteIndeterminate => "RemoteIndeterminate",
        RecoverPreparedIntentResultV1::AuthOrCapabilityFailure => "AuthOrCapabilityFailure",
    }
}

fn publish_persisted(
    intent: &PreparedIntentV1,
    remote: &mut AdversarialRemote,
) -> RecoverPreparedIntentResultV1 {
    let mut store = MemoryIntentStore::default();
    let persisted = persist_prepared_intent_before_publish_v1(intent, &mut store).unwrap();
    publish_persisted_intent_v1(&persisted, remote, VERIFIED_AT).unwrap()
}

#[test]
fn shared_fixture_freezes_segment_boundaries_and_prepared_identity() {
    let fixture = fixture();
    assert_eq!(fixture["schema"], "watchtracker-s2-lite-publish-golden-v1");
    for vector in fixture["pathCases"].as_array().unwrap() {
        let writer_seq = vector["writerSeq"].as_str().unwrap();
        assert_eq!(
            build_commit_remote_path_v1(&path_ref(&fixture, writer_seq)).unwrap(),
            vector["expectedPath"].as_str().unwrap()
        );
    }
    for vector in fixture["invalidWriterSeqCases"].as_array().unwrap() {
        assert_eq!(
            build_commit_remote_path_v1(&path_ref(&fixture, vector["writerSeq"].as_str().unwrap()))
                .unwrap_err()
                .0,
            vector["expectedError"].as_str().unwrap()
        );
    }
    let intent = prepared(&fixture);
    assert_eq!(
        intent.content_hash,
        fixture["preparedIntent"]["expectedContentHash"]
            .as_str()
            .unwrap()
    );
    assert_eq!(
        serde_json::to_value(&intent.commit_ref).unwrap(),
        fixture["preparedIntent"]["expectedCommitRef"]
    );
    assert_eq!(
        intent.remote_path,
        fixture["preparedIntent"]["expectedRemotePath"]
            .as_str()
            .unwrap()
    );
    assert_eq!(
        intent.intent_fingerprint,
        fixture["preparedIntent"]["expectedIntentFingerprint"]
            .as_str()
            .unwrap()
    );
    assert_eq!(sha256_hex(&intent.exact_bytes), intent.content_hash);
    validate_prepared_intent_v1(&intent).unwrap();
}

#[test]
fn local_intent_and_receipt_corruption_fail_closed() {
    let fixture = fixture();
    let intent = prepared(&fixture);
    let mut corruptions: Vec<IntentCorruption> = vec![
        Box::new(|value| value.remote_path.push_str(".other")),
        Box::new(|value| value.content_hash = "0".repeat(64)),
        Box::new(|value| value.commit_ref.content_hash = "0".repeat(64)),
        Box::new(|value| value.exact_bytes[0] ^= 1),
        Box::new(|value| value.intent_fingerprint = "0".repeat(64)),
    ];
    for mutate in corruptions.drain(..) {
        let mut corrupt = intent.clone();
        mutate(&mut corrupt);
        assert_eq!(
            validate_prepared_intent_v1(&corrupt),
            Err(ProtocolError("LOCAL_PREPARED_INTENT_CORRUPTION"))
        );
    }
    let mut unknown_intent = serde_json::to_value(&intent).unwrap();
    unknown_intent["unexpected"] = json!(true);
    assert!(serde_json::from_value::<PreparedIntentV1>(unknown_intent).is_err());

    let mut remote = AdversarialRemote::default();
    remote
        .objects
        .insert(intent.remote_path.clone(), intent.exact_bytes.clone());
    let RecoverPreparedIntentResultV1::AlreadyPublishedExact(receipt) =
        recover_prepared_intent_v1(&intent, &mut remote, VERIFIED_AT).unwrap()
    else {
        panic!("expected exact receipt")
    };
    validate_published_receipt_v1(&receipt, &intent).unwrap();
    let mut unknown_receipt = serde_json::to_value(&receipt).unwrap();
    unknown_receipt["unexpected"] = json!(true);
    assert!(serde_json::from_value::<RemotePublishedReceiptV1>(unknown_receipt).is_err());
    let mut corrupted_receipts: Vec<ReceiptCorruption> = vec![
        Box::new(|value| value.remote_path.push_str(".other")),
        Box::new(|value| value.content_hash = "0".repeat(64)),
        Box::new(|value| {
            value.commit_ref.commit_id = "20000000-0000-4000-8000-000000000099".to_string()
        }),
        Box::new(|value| value.prepared_intent_fingerprint = "0".repeat(64)),
        Box::new(|value| value.verified_exact_bytes_hash = "0".repeat(64)),
    ];
    for mutate in corrupted_receipts.drain(..) {
        let mut corrupt = receipt.clone();
        mutate(&mut corrupt);
        assert_eq!(
            validate_published_receipt_v1(&corrupt, &intent),
            Err(ProtocolError("LOCAL_PUBLISHED_RECEIPT_CORRUPTION"))
        );
    }
}

#[test]
fn shared_recovery_vectors_match_exact_taxonomy() {
    let fixture = fixture();
    let intent = prepared(&fixture);
    for vector in fixture["recoveryCases"].as_array().unwrap() {
        let mut remote = AdversarialRemote::default();
        match vector["remote"].as_str().unwrap() {
            "exact" => {
                remote
                    .objects
                    .insert(intent.remote_path.clone(), intent.exact_bytes.clone());
            }
            "different" => {
                remote.objects.insert(
                    intent.remote_path.clone(),
                    base64::engine::general_purpose::STANDARD
                        .decode(fixture["differentRemoteBytesBase64"].as_str().unwrap())
                        .unwrap(),
                );
            }
            "indeterminate" => remote.get_mode = "indeterminate",
            "auth" => remote.get_mode = "auth",
            "absent" => {}
            _ => panic!("unknown vector"),
        }
        let result = recover_prepared_intent_v1(&intent, &mut remote, VERIFIED_AT).unwrap();
        assert_eq!(
            outcome_name(&result),
            vector["expectedOutcome"].as_str().unwrap()
        );
        assert!(remote.put_calls.is_empty());
        if let RecoverPreparedIntentResultV1::CorruptionMismatch(event) = result {
            assert_eq!(event.code, "REMOTE_IMMUTABLE_PATH_CONTENT_MISMATCH");
            assert_eq!(event.freeze_class, "SYNC_ROOT_FROZEN_CORRUPTION");
            assert_eq!(
                event.observed_content_hash,
                fixture["differentRemoteBytesHash"].as_str().unwrap()
            );
        }
    }
}

#[test]
fn success_and_lost_put_responses_always_verify_exact_bytes() {
    let fixture = fixture();
    let intent = prepared(&fixture);
    for mode in ["success", "store-then-timeout"] {
        let mut remote = AdversarialRemote {
            put_mode: mode,
            ..Default::default()
        };
        let result = publish_persisted(&intent, &mut remote);
        assert_eq!(outcome_name(&result), "AlreadyPublishedExact");
        assert_eq!(remote.put_calls.len(), 1);
        assert_eq!(remote.get_calls.len(), 2);
        assert!(remote.put_calls[0].2);
        assert_eq!(remote.put_calls[0].1, intent.exact_bytes);
    }

    let mut no_store = AdversarialRemote {
        put_mode: "timeout-before-store",
        ..Default::default()
    };
    assert_eq!(
        outcome_name(&publish_persisted(&intent, &mut no_store)),
        "RetryPublishExact"
    );
    no_store.put_mode = "success";
    assert_eq!(
        outcome_name(&publish_persisted(&intent, &mut no_store)),
        "AlreadyPublishedExact"
    );
    assert_eq!(no_store.put_calls.len(), 2);
    assert!(no_store
        .put_calls
        .iter()
        .all(|(path, bytes, _)| path == &intent.remote_path && bytes == &intent.exact_bytes));
    let mut auth_put = AdversarialRemote {
        put_mode: "auth",
        ..Default::default()
    };
    assert_eq!(
        outcome_name(&publish_persisted(&intent, &mut auth_put)),
        "AuthOrCapabilityFailure"
    );
    let mut unavailable = AdversarialRemote {
        get_mode: "indeterminate",
        ..Default::default()
    };
    assert_eq!(
        outcome_name(&publish_persisted(&intent, &mut unavailable)),
        "RemoteIndeterminate"
    );
    assert!(unavailable.put_calls.is_empty());
}

#[test]
fn mismatch_is_not_overwritten_and_retries_are_idempotent() {
    let fixture = fixture();
    let intent = prepared(&fixture);
    let different = b"different".to_vec();
    let mut mismatch = AdversarialRemote::default();
    mismatch
        .objects
        .insert(intent.remote_path.clone(), different.clone());
    assert_eq!(
        outcome_name(&publish_persisted(&intent, &mut mismatch)),
        "CorruptionMismatch"
    );
    assert!(mismatch.put_calls.is_empty());
    assert_eq!(mismatch.objects[&intent.remote_path], different);

    let mut remote = AdversarialRemote::default();
    for _ in 0..8 {
        assert_eq!(
            outcome_name(&publish_persisted(&intent, &mut remote)),
            "AlreadyPublishedExact"
        );
    }
    assert_eq!(remote.objects.len(), 1);
    assert_eq!(remote.objects[&intent.remote_path], intent.exact_bytes);

    let mut second_wire: Value = serde_json::from_slice(&exact_bytes(&fixture)).unwrap();
    second_wire["commitId"] = json!("20000000-0000-4000-8000-000000000099");
    let second_bytes = serde_json::to_vec(&second_wire).unwrap();
    let second = prepare_commit_intent_v1(
        &second_bytes,
        fixture["preparedIntent"]["createdLocallyAtDiagnostic"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    assert_ne!(second.remote_path, intent.remote_path);
    assert_eq!(
        outcome_name(&publish_persisted(&second, &mut remote)),
        "AlreadyPublishedExact"
    );
    assert_eq!(remote.objects.len(), 2);
    assert_eq!(remote.objects[&intent.remote_path], intent.exact_bytes);
    assert_eq!(remote.objects[&second.remote_path], second.exact_bytes);
}

#[derive(Default)]
struct MemoryIntentStore {
    saved: Option<PreparedIntentV1>,
    fail: bool,
}

impl MemoryIntentStore {
    fn load(&self) -> PreparedIntentV1 {
        self.saved.clone().unwrap()
    }
}

impl PreparedIntentStoreV1 for MemoryIntentStore {
    fn persist(&mut self, intent: &PreparedIntentV1) -> super::canonical::Result<()> {
        if self.fail {
            return Err(ProtocolError("intent_persist_failed"));
        }
        self.saved = Some(intent.clone());
        Ok(())
    }
}

#[derive(Default)]
struct MemoryReceiptStore {
    saved: Option<RemotePublishedReceiptV1>,
    fail: bool,
    load_count: usize,
}

impl MemoryReceiptStore {
    fn load(&mut self) -> RemotePublishedReceiptV1 {
        self.load_count += 1;
        self.saved.clone().unwrap()
    }
}

impl PublishedReceiptStoreV1 for MemoryReceiptStore {
    fn persist(&mut self, receipt: &RemotePublishedReceiptV1) -> super::canonical::Result<()> {
        if self.fail {
            return Err(ProtocolError("receipt_persist_failed"));
        }
        self.saved = Some(receipt.clone());
        Ok(())
    }
}

#[test]
fn durable_crash_states_distinguish_new_reused_and_corrupt_receipts() {
    let fixture = fixture();
    let intent = prepared(&fixture);
    let mut failing = MemoryIntentStore {
        fail: true,
        ..Default::default()
    };
    assert_eq!(
        persist_prepared_intent_before_publish_v1(&intent, &mut failing),
        Err(ProtocolError("intent_persist_failed"))
    );

    let mut store = MemoryIntentStore::default();
    let persisted = persist_prepared_intent_before_publish_v1(&intent, &mut store).unwrap();
    assert_eq!(persisted.intent_fingerprint(), intent.intent_fingerprint);

    let mut receipt_remote = AdversarialRemote::default();
    receipt_remote
        .objects
        .insert(intent.remote_path.clone(), intent.exact_bytes.clone());
    let receipt_result =
        recover_prepared_intent_v1(&store.load(), &mut receipt_remote, VERIFIED_AT).unwrap();
    let mut receipt_store = MemoryReceiptStore::default();
    persist_verified_receipt_v1(&receipt_result, &intent, &mut receipt_store).unwrap();

    for cut in fixture["crashCutPoints"].as_array().unwrap() {
        let mut remote = AdversarialRemote::default();
        if !cut["durableIntentPresent"].as_bool().unwrap() {
            assert_eq!(cut["expectedOutcome"], "NoNetworkWithoutIntent");
            assert_eq!(
                remote.get_calls.len() as u64,
                cut["expectedGetCount"].as_u64().unwrap()
            );
            assert_eq!(
                remote.put_calls.len() as u64,
                cut["expectedPutCount"].as_u64().unwrap()
            );
            continue;
        }
        let durable_intent = store.load();
        if cut["remoteObjectState"] == "exact" {
            remote.objects.insert(
                durable_intent.remote_path.clone(),
                durable_intent.exact_bytes.clone(),
            );
        }
        let receipt_loads_before = receipt_store.load_count;
        let mut durable_receipt = cut["durableReceiptPresent"]
            .as_bool()
            .unwrap()
            .then(|| receipt_store.load());
        assert_eq!(
            receipt_store.load_count - receipt_loads_before,
            usize::from(cut["durableReceiptPresent"].as_bool().unwrap())
        );
        if cut["durableReceiptCorrupted"].as_bool() == Some(true) {
            durable_receipt
                .as_mut()
                .unwrap()
                .remote_path
                .push_str(".corrupt");
        }
        if cut["expectedOutcome"] == "LOCAL_PUBLISHED_RECEIPT_CORRUPTION" {
            assert_eq!(
                restart_durable_publish_v1(
                    &durable_intent,
                    durable_receipt.as_ref(),
                    &mut remote,
                    VERIFIED_AT
                ),
                Err(ProtocolError("LOCAL_PUBLISHED_RECEIPT_CORRUPTION"))
            );
        } else {
            let result = restart_durable_publish_v1(
                &durable_intent,
                durable_receipt.as_ref(),
                &mut remote,
                VERIFIED_AT,
            )
            .unwrap();
            assert_eq!(
                outcome_name(&result),
                cut["expectedOutcome"].as_str().unwrap()
            );
            if cut["expectedReceiptAction"] == "reuse-existing" {
                let RecoverPreparedIntentResultV1::AlreadyPublishedExact(receipt) = result else {
                    panic!("expected existing receipt")
                };
                assert_eq!(receipt, durable_receipt.unwrap());
            }
        }
        assert_eq!(
            remote.get_calls.len() as u64,
            cut["expectedGetCount"].as_u64().unwrap()
        );
        assert_eq!(
            remote.put_calls.len() as u64,
            cut["expectedPutCount"].as_u64().unwrap()
        );
    }

    let mut remote = AdversarialRemote {
        put_mode: "store-then-timeout",
        ..Default::default()
    };
    let result = publish_persisted_intent_v1(&persisted, &mut remote, VERIFIED_AT).unwrap();
    assert_eq!(
        persist_verified_receipt_v1(
            &RecoverPreparedIntentResultV1::RetryPublishExact,
            &intent,
            &mut receipt_store
        ),
        Err(ProtocolError("receipt_requires_exact_remote_verification"))
    );
    persist_verified_receipt_v1(&result, &intent, &mut receipt_store).unwrap();
    validate_published_receipt_v1(receipt_store.saved.as_ref().unwrap(), &intent).unwrap();
}

#[test]
fn receipt_loss_and_delayed_visibility_recover_without_new_identity() {
    let fixture = fixture();
    let intent = prepared(&fixture);
    let mut remote = AdversarialRemote {
        delayed_reads: 1,
        ..Default::default()
    };
    remote
        .objects
        .insert(intent.remote_path.clone(), intent.exact_bytes.clone());
    assert_eq!(
        outcome_name(&publish_persisted(&intent, &mut remote)),
        "RemoteIndeterminate"
    );
    let recovered = recover_prepared_intent_v1(&intent, &mut remote, VERIFIED_AT).unwrap();
    assert_eq!(outcome_name(&recovered), "AlreadyPublishedExact");
    let mut failing_receipt = MemoryReceiptStore {
        fail: true,
        ..Default::default()
    };
    assert_eq!(
        persist_verified_receipt_v1(&recovered, &intent, &mut failing_receipt),
        Err(ProtocolError("receipt_persist_failed"))
    );
    let after_restart = recover_prepared_intent_v1(&intent, &mut remote, VERIFIED_AT).unwrap();
    assert_eq!(recovered, after_restart);
}

#[test]
fn pure_state_machine_requires_verification() {
    let state = advance_immutable_publish_state_v1(
        ImmutablePublishStateV1::Prepared,
        ImmutablePublishEventV1::PutStarted,
    )
    .unwrap();
    assert_eq!(state, ImmutablePublishStateV1::PutAttempted);
    assert_eq!(
        advance_immutable_publish_state_v1(state, ImmutablePublishEventV1::RemoteExact),
        Err(ProtocolError("invalid_immutable_publish_transition"))
    );
    let verifying =
        advance_immutable_publish_state_v1(state, ImmutablePublishEventV1::VerifyStarted).unwrap();
    assert_eq!(
        advance_immutable_publish_state_v1(verifying, ImmutablePublishEventV1::RemoteExact)
            .unwrap(),
        ImmutablePublishStateV1::VerifiedPublished
    );
    assert_eq!(
        advance_immutable_publish_state_v1(verifying, ImmutablePublishEventV1::RemoteAbsent)
            .unwrap(),
        ImmutablePublishStateV1::Prepared
    );
}

#[test]
fn generated_path_and_preparation_properties_hold_for_512_commits() {
    let fixture = fixture();
    let template: Value = serde_json::from_slice(&exact_bytes(&fixture)).unwrap();
    let mut state = 0x9e37_79b9_7f4a_7c15_u64;
    let mut property_paths = Vec::new();
    for index in 1..=512_u64 {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        let sequence = state.max(1).to_string();
        let suffix = format!("{index:012}");
        let mut wire = template.clone();
        wire["writerId"] = json!(format!("10000000-0000-4000-8000-{suffix}"));
        wire["writerSeq"] = json!(sequence.clone());
        wire["commitId"] = json!(format!("20000000-0000-4000-8000-{suffix}"));
        property_paths.push(
            build_commit_remote_path_v1(&CommitRef {
                writer_id: wire["writerId"].as_str().unwrap().to_string(),
                writer_seq: sequence.clone(),
                commit_id: wire["commitId"].as_str().unwrap().to_string(),
                content_hash: fixture["pathIdentity"]["contentHash"]
                    .as_str()
                    .unwrap()
                    .to_string(),
            })
            .unwrap(),
        );
        let bytes = serde_json::to_vec(&wire).unwrap();
        let intent = prepare_commit_intent_v1(
            &bytes,
            fixture["preparedIntent"]["createdLocallyAtDiagnostic"]
                .as_str()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(intent.content_hash, sha256_hex(&bytes));
        assert_eq!(intent.commit_ref.content_hash, intent.content_hash);
        assert_eq!(
            build_commit_remote_path_v1(&intent.commit_ref).unwrap(),
            intent.remote_path
        );
        assert!(intent.remote_path.contains(&format!("/{sequence:0>20}--")));
    }
    assert_eq!(
        property_paths.len(),
        fixture["pathProperty"]["count"].as_u64().unwrap() as usize
    );
    assert_eq!(
        sha256_hex(property_paths.join("\n").as_bytes()),
        fixture["pathProperty"]["expectedPathsSha256"]
            .as_str()
            .unwrap()
    );
}
