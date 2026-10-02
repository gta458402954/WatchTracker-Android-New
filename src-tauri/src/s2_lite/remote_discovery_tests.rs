use std::collections::{BTreeMap, BTreeSet};
use std::fs;

use base64::Engine;
use serde_json::{json, Value};

use super::canonical::sha256_hex;
use super::causal::{decode_frozen_wire_commit_v1, replay_verified_history_v1};
use super::immutable_publish::build_commit_remote_path_v1;
use super::remote_discovery::{
    canonical_discovery_projection_v1, choose_historical_audit_target_v1,
    classify_candidate_path_v1, create_discovery_state_v1, observe_candidate_listing_v1,
    observe_segment_listing_v1, observe_writer_listing_v1, parse_activation_candidate_path_v1,
    parse_writer_candidate_path_v1, retained_verified_commit_bytes_v1, run_discovery_round_v1,
    validate_production_activation_body_v1, verify_activation_candidate_v1,
    verify_commit_candidate_v1, ActivationValidatorErrorV1, ActivationVerificationV1,
    CandidatePathClassificationV1, DirectoryListResultV1, DiscoveryBudgetsV1,
    DiscoveryExactGetResultV1, DiscoveryRemoteV1, DiscoveryStateV1, HistoricalAuditCursorV1,
    ObservedCandidateV1, VerifiedFingerprintEvidenceV1, VerifiedRemoteObjectV1,
};
use super::types::{CommitRef, CommitV1};

fn fixture() -> Value {
    serde_json::from_str(
        &fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../contracts/s2-lite/v1/discovery-golden-v1.json"
        ))
        .unwrap(),
    )
    .unwrap()
}

fn raw_template() -> Value {
    let fixture: Value = serde_json::from_str(
        &fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../contracts/s2-lite/v1/raw-wire-json-v1.json"
        ))
        .unwrap(),
    )
    .unwrap();
    let encoded = fixture["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|case| case["name"] == "canonical-mutation-resolves-absent")
        .unwrap()["utf8Base64"]
        .as_str()
        .unwrap();
    serde_json::from_slice(
        &base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .unwrap(),
    )
    .unwrap()
}

fn uuid(prefix: char, value: u64) -> String {
    format!("{prefix}0000000-0000-4000-8000-{value:012}")
}

fn dummy_ref(writer_id: &str, writer_seq: u64, salt: u64) -> CommitRef {
    CommitRef {
        writer_id: writer_id.to_string(),
        writer_seq: writer_seq.to_string(),
        commit_id: uuid('2', salt),
        content_hash: (salt % 10).to_string().repeat(64),
    }
}

struct MadeCommit {
    bytes: Vec<u8>,
    commit_ref: CommitRef,
    path: String,
    candidate: ObservedCandidateV1,
    commit: CommitV1,
}

fn make_commit(writer_id: &str, seq: u64, salt: u64, previous: Option<CommitRef>) -> MadeCommit {
    let mut wire = raw_template();
    wire["writerId"] = json!(writer_id);
    wire["writerSeq"] = json!(seq.to_string());
    wire["commitId"] = json!(uuid('2', salt));
    let prior = if seq == 1 {
        None
    } else {
        Some(previous.unwrap_or_else(|| dummy_ref(writer_id, seq - 1, salt + 1000)))
    };
    wire["previousWriterCommit"] = serde_json::to_value(&prior).unwrap();
    wire["basisClock"] = serde_json::to_value(prior.iter().collect::<Vec<_>>()).unwrap();
    wire["mutations"][0]["localMutationId"] = json!(uuid('4', salt));
    wire["mutations"][0]["entityKey"] = json!(["collection", format!("discovery-{salt}")]);
    wire["mutations"][0]["value"]["id"] = json!(format!("discovery-{salt}"));
    wire["mutations"][0]["value"]["name"] = json!(format!("Discovery {salt}"));
    wire["mutations"][0]["value"]["normalizedName"] = json!(format!("discovery {salt}"));
    let bytes = serde_json::to_vec(&wire).unwrap();
    let commit = decode_frozen_wire_commit_v1(&bytes).unwrap();
    let commit_ref = commit.commit_ref();
    let path = build_commit_remote_path_v1(&commit_ref).unwrap();
    let candidate = parse_writer_candidate_path_v1(&path).unwrap();
    MadeCommit {
        bytes,
        commit_ref,
        path,
        candidate,
        commit,
    }
}

#[derive(Default)]
struct FakeDiscoveryRemote {
    listings: BTreeMap<String, Vec<String>>,
    objects: BTreeMap<String, Vec<u8>>,
    get_calls: Vec<String>,
    list_failures: BTreeSet<String>,
    get_failures: BTreeSet<String>,
}

impl DiscoveryRemoteV1 for FakeDiscoveryRemote {
    fn list_directory(&mut self, path: &str) -> DirectoryListResultV1 {
        if self.list_failures.contains(path) {
            return DirectoryListResultV1::Indeterminate;
        }
        DirectoryListResultV1::Entries(self.listings.get(path).cloned().unwrap_or_default())
    }

    fn get_exact(&mut self, path: &str) -> DiscoveryExactGetResultV1 {
        self.get_calls.push(path.to_string());
        if self.get_failures.contains(path) {
            return DiscoveryExactGetResultV1::Indeterminate;
        }
        self.objects
            .get(path)
            .cloned()
            .map(DiscoveryExactGetResultV1::DefinitelyPresent)
            .unwrap_or(DiscoveryExactGetResultV1::DefinitelyAbsent)
    }
}

fn activation_validator(
    bytes: &[u8],
) -> std::result::Result<ActivationVerificationV1, ActivationValidatorErrorV1> {
    let value: Value = serde_json::from_slice(bytes)
        .map_err(|_| ActivationValidatorErrorV1::ProtocolValidation)?;
    Ok(ActivationVerificationV1 {
        activation_id: value["activationId"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        legacy_fingerprint: value["legacyFingerprint"].as_str().map(str::to_string),
        semantic_profile_supported: value["semanticProfileSupported"].as_bool().unwrap_or(false),
        required_features_supported: value["requiredFeaturesSupported"]
            .as_bool()
            .unwrap_or(false),
    })
}

#[test]
fn production_activation_validator_is_exact_and_preserves_nullable_fingerprint() {
    let activation_id = "10000000-0000-4000-8000-000000000001";
    let valid = |fingerprint: Value| {
        serde_json::to_vec(&json!({
            "activationId": activation_id,
            "legacyFingerprint": fingerprint,
            "protocol": "watchtracker-s2-lite",
            "protocolVersion": 1,
            "requiredFeatures": [],
            "s2SemanticProfileVersion": 1,
        }))
        .unwrap()
    };
    assert_eq!(
        validate_production_activation_body_v1(&valid(Value::Null)).unwrap(),
        ActivationVerificationV1 {
            activation_id: activation_id.to_string(),
            legacy_fingerprint: None,
            semantic_profile_supported: true,
            required_features_supported: true,
        }
    );
    let fingerprint = "a".repeat(64);
    assert_eq!(
        validate_production_activation_body_v1(&valid(json!(fingerprint)))
            .unwrap()
            .legacy_fingerprint,
        Some(fingerprint)
    );
    for invalid in [
        br#"{}"#.to_vec(),
        valid(json!("")),
        serde_json::to_vec(&json!({
            "activationId": activation_id,
            "legacyFingerprint": null,
            "protocol": "watchtracker-s2-lite",
            "protocolVersion": 2,
            "requiredFeatures": [],
            "s2SemanticProfileVersion": 1,
        }))
        .unwrap(),
        serde_json::to_vec(&json!({
            "activationId": activation_id,
            "legacyFingerprint": null,
            "protocol": "watchtracker-s2-lite",
            "protocolVersion": 1,
            "requiredFeatures": ["unknown"],
            "s2SemanticProfileVersion": 1,
        }))
        .unwrap(),
    ] {
        assert_eq!(
            validate_production_activation_body_v1(&invalid),
            Err(ActivationValidatorErrorV1::ProtocolValidation)
        );
    }
}

fn ref_key_for_projection(value: &CommitRef) -> String {
    format!(
        "{}/{}/{}/{}",
        value.writer_id, value.writer_seq, value.commit_id, value.content_hash
    )
}

fn canonical_trace_projection(state: &DiscoveryStateV1) -> Value {
    let mut verified = state
        .verified_objects
        .iter()
        .filter(|value| value.commit_ref.is_some())
        .collect::<Vec<_>>();
    verified.sort_by(|a, b| a.path.cmp(&b.path));
    let mut groups = BTreeMap::<String, Vec<&VerifiedRemoteObjectV1>>::new();
    for value in &verified {
        let commit_ref = value.commit_ref.as_ref().unwrap();
        groups
            .entry(format!(
                "{}/{}",
                commit_ref.writer_id, commit_ref.writer_seq
            ))
            .or_default()
            .push(value);
    }
    let fork_state = groups
        .values()
        .filter(|values| values.len() > 1)
        .map(|values| {
            let commit_ref = values[0].commit_ref.as_ref().unwrap();
            let mut paths = values.iter().map(|value| value.path.clone()).collect::<Vec<_>>();
            paths.sort();
            json!({
                "writerId": commit_ref.writer_id,
                "writerSeq": commit_ref.writer_seq,
                "safeWriterFrontier": (commit_ref.writer_seq.parse::<u64>().unwrap() - 1).to_string(),
                "paths": paths,
            })
        })
        .collect::<Vec<_>>();
    json!({
        "observedCandidates": state.observed_candidates.iter().map(|value| value.path()).collect::<Vec<_>>(),
        "verifiedRefs": verified.iter().map(|value| ref_key_for_projection(value.commit_ref.as_ref().unwrap())).collect::<Vec<_>>(),
        "gaps": state.known_gaps,
        "dependencyQueue": state.targeted_queue.iter().map(ref_key_for_projection).collect::<Vec<_>>(),
        "dependencyProgress": state.exact_work_scheduler,
        "auditCursor": state.historical_audit_cursor,
        "scheduledLists": state.last_round_scheduled_lists,
        "scheduledGets": state.last_round_scheduled_gets,
        "fatalSignals": state.root_fatal_signals,
        "forkState": fork_state,
    })
}

#[test]
fn shared_fixture_path_and_audit_oracle_match_rust() {
    let fixture = fixture();
    assert_eq!(fixture["scenarioCoverage"].as_array().unwrap().len(), 14);
    for case in fixture["pathCases"].as_array().unwrap() {
        let path = case["path"].as_str().unwrap();
        let parsed = if case["kind"] == "activation" {
            parse_activation_candidate_path_v1(path)
        } else {
            parse_writer_candidate_path_v1(path)
        };
        assert_eq!(
            parsed.is_some(),
            case["accepted"].as_bool().unwrap(),
            "{}",
            case["name"]
        );
    }
    let mut state = create_discovery_state_v1();
    state.historical_closed_segments = fixture["audit"]["closedSegments"]
        .as_array()
        .unwrap()
        .iter()
        .map(|value| value.as_str().unwrap().to_string())
        .collect();
    state.historical_audit_cursor =
        serde_json::from_value(fixture["audit"]["initialCursor"].clone()).unwrap();
    let actual = (0..fixture["audit"]["rounds"].as_u64().unwrap())
        .map(|_| choose_historical_audit_target_v1(&mut state).unwrap())
        .collect::<Vec<_>>();
    let expected = fixture["audit"]["expectedSequence"]
        .as_array()
        .unwrap()
        .iter()
        .map(|value| value.as_str().unwrap().to_string())
        .collect::<Vec<_>>();
    assert_eq!(actual, expected);
    let mut corrupt = create_discovery_state_v1();
    let inconsistent = fixture["pathCases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|case| case["name"] == "segment-seq-mismatch")
        .unwrap()["path"]
        .as_str()
        .unwrap()
        .to_string();
    observe_candidate_listing_v1(&mut corrupt, &[inconsistent]);
    assert!(corrupt
        .root_fatal_signals
        .iter()
        .any(|signal| signal.code == "REMOTE_S2_PATH_IDENTITY_MISMATCH"));
}

#[test]
fn shared_junk_corpus_matches_candidate_and_fatal_classification() {
    let fixture = fixture();
    for item in fixture["junkCorpus"].as_array().unwrap() {
        let expected = match item["classification"].as_str().unwrap() {
            "Candidate" => CandidatePathClassificationV1::Candidate,
            "CanonicalIdentityMismatch" => CandidatePathClassificationV1::CanonicalIdentityMismatch,
            "UnrelatedJunk" => CandidatePathClassificationV1::UnrelatedJunk,
            _ => unreachable!(),
        };
        let path = item["path"].as_str().unwrap();
        assert_eq!(
            classify_candidate_path_v1(path),
            expected,
            "{}",
            item["name"]
        );
        let mut state = create_discovery_state_v1();
        observe_candidate_listing_v1(&mut state, &[path.to_string()]);
        assert_eq!(
            !state.root_fatal_signals.is_empty(),
            item["fatal"].as_bool().unwrap(),
            "{}",
            item["name"]
        );
    }
}

#[test]
fn rust_executes_shared_multi_round_remote_trace() {
    let fixture = fixture();
    let trace = &fixture["executableTrace"];
    let budgets = DiscoveryBudgetsV1 {
        max_exact_fetches_per_sync: trace["budgets"]["maxExactFetchesPerSync"].as_u64().unwrap()
            as usize,
        max_segments_per_writer_per_sync: trace["budgets"]["maxSegmentsPerWriterPerSync"]
            .as_u64()
            .unwrap() as usize,
        max_dependency_targets_per_sync: trace["budgets"]["maxDependencyTargetsPerSync"]
            .as_u64()
            .unwrap() as usize,
        max_listing_entries_per_directory: trace["budgets"]["maxListingEntriesPerDirectory"]
            .as_u64()
            .unwrap() as usize,
    };
    let objects = trace["objects"]
        .as_array()
        .unwrap()
        .iter()
        .map(|object| {
            (
                object["path"].as_str().unwrap().to_string(),
                base64::engine::general_purpose::STANDARD
                    .decode(object["utf8Base64"].as_str().unwrap())
                    .unwrap(),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let mut state = create_discovery_state_v1();
    for round in trace["rounds"].as_array().unwrap() {
        let listings = round["listings"]
            .as_object()
            .unwrap()
            .iter()
            .map(|(path, entries)| {
                (
                    path.clone(),
                    entries
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|value| value.as_str().unwrap().to_string())
                        .collect(),
                )
            })
            .collect();
        let mut remote = FakeDiscoveryRemote {
            listings,
            objects: objects.clone(),
            ..FakeDiscoveryRemote::default()
        };
        state = run_discovery_round_v1(&state, &mut remote, &mut activation_validator, &budgets)
            .unwrap();
        assert_eq!(canonical_trace_projection(&state), round["expected"]);
    }
}

#[test]
fn invalid_activation_is_retained_fatal_while_valid_commit_continues_and_restart_omits() {
    let fixture = fixture();
    let trace = &fixture["activationFailureTrace"];
    let budgets = DiscoveryBudgetsV1 {
        max_exact_fetches_per_sync: trace["budgets"]["maxExactFetchesPerSync"].as_u64().unwrap()
            as usize,
        max_segments_per_writer_per_sync: trace["budgets"]["maxSegmentsPerWriterPerSync"]
            .as_u64()
            .unwrap() as usize,
        max_dependency_targets_per_sync: trace["budgets"]["maxDependencyTargetsPerSync"]
            .as_u64()
            .unwrap() as usize,
        max_listing_entries_per_directory: trace["budgets"]["maxListingEntriesPerDirectory"]
            .as_u64()
            .unwrap() as usize,
    };
    let listings = trace["listing"]
        .as_object()
        .unwrap()
        .iter()
        .map(|(path, entries)| {
            (
                path.clone(),
                entries
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|value| value.as_str().unwrap().to_string())
                    .collect(),
            )
        })
        .collect();
    let objects = trace["objects"]
        .as_array()
        .unwrap()
        .iter()
        .map(|object| {
            (
                object["path"].as_str().unwrap().to_string(),
                base64::engine::general_purpose::STANDARD
                    .decode(object["utf8Base64"].as_str().unwrap())
                    .unwrap(),
            )
        })
        .collect();
    let mut remote = FakeDiscoveryRemote {
        listings,
        objects,
        ..FakeDiscoveryRemote::default()
    };
    let mut state = run_discovery_round_v1(
        &create_discovery_state_v1(),
        &mut remote,
        &mut activation_validator,
        &budgets,
    )
    .unwrap();
    assert_eq!(canonical_trace_projection(&state), trace["expected"]);
    let bad_path = trace["objects"][0]["path"].as_str().unwrap();
    assert!(state
        .observed_candidates
        .iter()
        .any(|value| value.path() == bad_path));
    assert_eq!(
        state
            .verified_objects
            .iter()
            .filter(|value| value.kind == "commit")
            .count(),
        1
    );

    state = serde_json::from_slice(&serde_json::to_vec(&state).unwrap()).unwrap();
    let fatal_before = state.root_fatal_signals.clone();
    let mut omitted = FakeDiscoveryRemote::default();
    state =
        run_discovery_round_v1(&state, &mut omitted, &mut activation_validator, &budgets).unwrap();
    assert_eq!(state.root_fatal_signals, fatal_before);
    assert!(state
        .observed_candidates
        .iter()
        .any(|value| value.path() == bad_path));
}

#[test]
fn unknown_activation_validator_error_propagates() {
    let fixture = fixture();
    let trace = &fixture["activationFailureTrace"];
    let listings = trace["listing"]
        .as_object()
        .unwrap()
        .iter()
        .map(|(path, entries)| {
            (
                path.clone(),
                entries
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|value| value.as_str().unwrap().to_string())
                    .collect(),
            )
        })
        .collect();
    let objects = trace["objects"]
        .as_array()
        .unwrap()
        .iter()
        .map(|object| {
            (
                object["path"].as_str().unwrap().to_string(),
                base64::engine::general_purpose::STANDARD
                    .decode(object["utf8Base64"].as_str().unwrap())
                    .unwrap(),
            )
        })
        .collect();
    let mut remote = FakeDiscoveryRemote {
        listings,
        objects,
        ..FakeDiscoveryRemote::default()
    };
    let error = run_discovery_round_v1(
        &create_discovery_state_v1(),
        &mut remote,
        &mut |_| {
            Err(ActivationValidatorErrorV1::Internal(
                super::canonical::ProtocolError("validator_programming_bug"),
            ))
        },
        &DiscoveryBudgetsV1::default(),
    )
    .unwrap_err();
    assert_eq!(
        error,
        super::canonical::ProtocolError("validator_programming_bug")
    );
}

#[test]
fn rust_round_executor_retains_work_across_remote_failures() {
    let fixture = fixture();
    let writer = fixture["identities"]["writerA"].as_str().unwrap();
    let commit = make_commit(writer, 1, 7201, None);
    let mut state = create_discovery_state_v1();
    observe_candidate_listing_v1(&mut state, std::slice::from_ref(&commit.path));
    let mut failing = FakeDiscoveryRemote::default();
    failing.list_failures.insert("writers/".to_string());
    failing.get_failures.insert(commit.path.clone());
    state = run_discovery_round_v1(
        &state,
        &mut failing,
        &mut activation_validator,
        &DiscoveryBudgetsV1::default(),
    )
    .unwrap();
    assert!(state.last_round_indeterminate);
    assert!(state
        .observed_candidates
        .iter()
        .any(|value| value.path() == commit.path));
    let mut recovered = FakeDiscoveryRemote::default();
    recovered.objects.insert(commit.path.clone(), commit.bytes);
    state = run_discovery_round_v1(
        &state,
        &mut recovered,
        &mut activation_validator,
        &DiscoveryBudgetsV1::default(),
    )
    .unwrap();
    assert!(state
        .verified_objects
        .iter()
        .any(|value| value.path == commit.path));
}

#[test]
fn fair_scheduler_closes_verified_prefix_and_dependency_restart_counterexamples() {
    let fixture = fixture();
    let writer_a = fixture["identities"]["writerA"].as_str().unwrap();
    let writer_b = fixture["identities"]["writerB"].as_str().unwrap();
    let original = make_commit(writer_b, 1, 7001, None);
    let alternate = make_commit(writer_b, 1, 7002, None);
    let mut state = create_discovery_state_v1();
    observe_candidate_listing_v1(&mut state, &[original.path.clone(), alternate.path.clone()]);
    verify_commit_candidate_v1(&mut state, &original.candidate, &original.bytes).unwrap();
    for index in 1..=64_u64 {
        let commit_ref = dummy_ref(writer_a, index, 8000 + index);
        let prefix_path = build_commit_remote_path_v1(&commit_ref).unwrap();
        state.verified_objects.push(VerifiedRemoteObjectV1 {
            path: prefix_path.clone(),
            kind: "commit".to_string(),
            exact_bytes_hash: commit_ref.content_hash.clone(),
            exact_bytes_hex: String::new(),
            content_hash: commit_ref.content_hash.clone(),
            commit_ref: Some(commit_ref),
            activation_id: None,
            fingerprint_evidence: VerifiedFingerprintEvidenceV1::Missing,
        });
        state.reverification_queue.push(prefix_path);
    }
    let mut remote = FakeDiscoveryRemote::default();
    remote
        .objects
        .insert(alternate.path.clone(), alternate.bytes.clone());
    state = run_discovery_round_v1(
        &state,
        &mut remote,
        &mut activation_validator,
        &DiscoveryBudgetsV1::default(),
    )
    .unwrap();
    assert_eq!(state.last_round_scheduled_gets[0], alternate.path);
    assert!(state
        .root_fatal_signals
        .iter()
        .any(|value| value.code == "WRITER_FORK" && value.writer_id.as_deref() == Some(writer_b)));

    let mut dependencies = create_discovery_state_v1();
    dependencies.targeted_queue = (1..=65_u64)
        .map(|index| dummy_ref(writer_a, index, 9000 + index))
        .collect();
    let expected_65 = build_commit_remote_path_v1(&dependencies.targeted_queue[64]).unwrap();
    let mut absent = FakeDiscoveryRemote::default();
    let first = run_discovery_round_v1(
        &dependencies,
        &mut absent,
        &mut activation_validator,
        &DiscoveryBudgetsV1::default(),
    )
    .unwrap();
    assert_eq!(first.last_round_scheduled_gets.len(), 64);
    assert!(!first.last_round_scheduled_gets.contains(&expected_65));
    let restarted: DiscoveryStateV1 =
        serde_json::from_slice(&serde_json::to_vec(&first).unwrap()).unwrap();
    let second = run_discovery_round_v1(
        &restarted,
        &mut absent,
        &mut activation_validator,
        &DiscoveryBudgetsV1::default(),
    )
    .unwrap();
    assert_eq!(second.last_round_scheduled_gets[0], expected_65);
    assert!(second
        .targeted_queue
        .iter()
        .any(|value| build_commit_remote_path_v1(value).unwrap() == expected_65));
}

#[test]
fn retained_knowledge_gaps_and_cursor_survive_serialized_restart() {
    let fixture = fixture();
    let writer = fixture["identities"]["writerA"].as_str().unwrap();
    let one = make_commit(writer, 1, 1, None);
    let four = make_commit(writer, 4, 4, None);
    let mut state = create_discovery_state_v1();
    observe_writer_listing_v1(&mut state, &[writer.to_string(), writer.to_string()]);
    observe_segment_listing_v1(&mut state, writer, &["00000000000000".to_string()]);
    observe_candidate_listing_v1(
        &mut state,
        &[four.path.clone(), one.path.clone(), one.path.clone()],
    );
    verify_commit_candidate_v1(&mut state, &one.candidate, &one.bytes).unwrap();
    verify_commit_candidate_v1(&mut state, &four.candidate, &four.bytes).unwrap();
    assert_eq!(
        state.known_gaps,
        vec![format!("{writer}/2"), format!("{writer}/3")]
    );
    observe_writer_listing_v1(&mut state, &[]);
    observe_segment_listing_v1(&mut state, writer, &[]);
    observe_candidate_listing_v1(&mut state, &[]);
    let encoded = serde_json::to_vec(&state).unwrap();
    let restarted: DiscoveryStateV1 = serde_json::from_slice(&encoded).unwrap();
    assert_eq!(
        canonical_discovery_projection_v1(&state),
        canonical_discovery_projection_v1(&restarted)
    );
    assert!(restarted
        .observed_candidates
        .iter()
        .any(|candidate| candidate.path() == one.path));
    let retained = retained_verified_commit_bytes_v1(&restarted).unwrap();
    assert_eq!(retained.len(), 2);
    let decoded = retained
        .iter()
        .map(|bytes| decode_frozen_wire_commit_v1(bytes).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        replay_verified_history_v1(&decoded).unwrap().validity.len(),
        2
    );
}

#[test]
fn audit_is_fair_for_ten_thousand_rounds_and_restart_does_not_starve() {
    let fixture = fixture();
    let mut state = create_discovery_state_v1();
    state.historical_closed_segments = fixture["audit"]["closedSegments"]
        .as_array()
        .unwrap()
        .iter()
        .map(|value| value.as_str().unwrap().to_string())
        .collect();
    let mut counts = BTreeMap::<String, usize>::new();
    for round in 0..10_000 {
        let selected = choose_historical_audit_target_v1(&mut state).unwrap();
        *counts.entry(selected).or_default() += 1;
        if round == 4_999 {
            state = serde_json::from_slice(&serde_json::to_vec(&state).unwrap()).unwrap();
        }
    }
    for segment in &state.historical_closed_segments {
        assert!(
            counts.get(segment).copied().unwrap_or_default() > 500,
            "{segment}"
        );
    }
}

#[test]
fn late_historical_fork_freezes_and_phase_one_quarantines_both_alternatives() {
    let fixture = fixture();
    let writer = fixture["identities"]["writerA"].as_str().unwrap();
    let previous = dummy_ref(writer, 199, 9000);
    let original = make_commit(writer, 200, 200, Some(previous.clone()));
    let alternate = make_commit(writer, 200, 201, Some(previous));
    let mut state = create_discovery_state_v1();
    observe_candidate_listing_v1(&mut state, std::slice::from_ref(&original.path));
    verify_commit_candidate_v1(&mut state, &original.candidate, &original.bytes).unwrap();
    assert!(state.root_fatal_signals.is_empty());
    observe_candidate_listing_v1(&mut state, std::slice::from_ref(&alternate.path));
    verify_commit_candidate_v1(&mut state, &alternate.candidate, &alternate.bytes).unwrap();
    let signal = state
        .root_fatal_signals
        .iter()
        .find(|value| value.code == "WRITER_FORK")
        .unwrap();
    assert_eq!(signal.safe_writer_frontier.as_deref(), Some("199"));
    observe_candidate_listing_v1(&mut state, &[]);
    assert!(state
        .root_fatal_signals
        .iter()
        .any(|value| value.code == "WRITER_FORK"));
    let replay = replay_verified_history_v1(&[original.commit, alternate.commit]).unwrap();
    assert_eq!(replay.forks.len(), 1);
    assert_eq!(replay.forks[0].safe_writer_frontier, "199");
    assert_eq!(replay.unsafe_commit_refs.len(), 2);
}

#[test]
fn immutable_path_and_path_body_mismatch_fail_closed() {
    let fixture = fixture();
    let writer_a = fixture["identities"]["writerA"].as_str().unwrap();
    let writer_b = fixture["identities"]["writerB"].as_str().unwrap();
    let original = make_commit(writer_a, 1, 11, None);
    let mut state = create_discovery_state_v1();
    observe_candidate_listing_v1(&mut state, std::slice::from_ref(&original.path));
    verify_commit_candidate_v1(&mut state, &original.candidate, &original.bytes).unwrap();
    verify_commit_candidate_v1(&mut state, &original.candidate, b"changed").unwrap();
    assert!(state
        .root_fatal_signals
        .iter()
        .any(|value| value.code == "REMOTE_IMMUTABLE_PATH_CONTENT_MISMATCH"));

    let other = make_commit(writer_b, 1, 12, None);
    let mut wrong_ref = other.commit_ref.clone();
    wrong_ref.writer_id = writer_a.to_string();
    let wrong_path = build_commit_remote_path_v1(&wrong_ref).unwrap();
    let wrong_candidate = parse_writer_candidate_path_v1(&wrong_path).unwrap();
    verify_commit_candidate_v1(&mut state, &wrong_candidate, &other.bytes).unwrap();
    assert!(state
        .root_fatal_signals
        .iter()
        .any(|value| value.code == "REMOTE_S2_PATH_BODY_IDENTITY_MISMATCH"));
    assert_eq!(sha256_hex(&other.bytes), other.commit_ref.content_hash);
}

#[test]
fn candidate_order_and_duplicates_are_idempotent() {
    let fixture = fixture();
    let writer = fixture["identities"]["writerA"].as_str().unwrap();
    let commits = (1..=4)
        .map(|seq| make_commit(writer, seq, seq, None))
        .collect::<Vec<_>>();
    let mut forward = create_discovery_state_v1();
    let paths = commits
        .iter()
        .map(|value| value.path.clone())
        .collect::<Vec<_>>();
    observe_candidate_listing_v1(&mut forward, &paths);
    for commit in &commits {
        verify_commit_candidate_v1(&mut forward, &commit.candidate, &commit.bytes).unwrap();
    }
    let expected = canonical_discovery_projection_v1(&forward);
    let mut reverse = create_discovery_state_v1();
    let duplicate_reverse = paths
        .iter()
        .rev()
        .chain(paths.iter().rev())
        .cloned()
        .collect::<Vec<_>>();
    observe_candidate_listing_v1(&mut reverse, &duplicate_reverse);
    for commit in commits.iter().rev() {
        verify_commit_candidate_v1(&mut reverse, &commit.candidate, &commit.bytes).unwrap();
    }
    assert_eq!(canonical_discovery_projection_v1(&reverse), expected);
}

#[test]
fn cursor_fixture_deserializes_exact_cross_language_shape() {
    let fixture = fixture();
    let cursor: HistoricalAuditCursorV1 =
        serde_json::from_value(fixture["audit"]["initialCursor"].clone()).unwrap();
    assert_eq!(cursor.last_writer_id, None);
    assert!(cursor.last_segment_by_writer.is_empty());
}

#[test]
fn activation_candidate_is_retained_verified_and_never_interpreted_as_cutover() {
    let fixture = fixture();
    let activation_id = fixture["identities"]["activationId"].as_str().unwrap();
    let bytes = serde_json::to_vec(&json!({
        "activationId": activation_id,
        "semanticProfileSupported": true,
        "requiredFeaturesSupported": true
    }))
    .unwrap();
    let path = format!("activations/{activation_id}--{}.json", sha256_hex(&bytes));
    let candidate = parse_activation_candidate_path_v1(&path).unwrap();
    let mut state = create_discovery_state_v1();
    observe_candidate_listing_v1(&mut state, std::slice::from_ref(&path));
    verify_activation_candidate_v1(
        &mut state,
        &candidate,
        &bytes,
        &ActivationVerificationV1 {
            activation_id: activation_id.to_string(),
            legacy_fingerprint: None,
            semantic_profile_supported: true,
            required_features_supported: true,
        },
    )
    .unwrap();
    observe_candidate_listing_v1(&mut state, &[]);
    assert_eq!(state.observed_activations, vec![path.clone()]);
    assert!(state
        .verified_objects
        .iter()
        .any(|object| object.path == path && object.kind == "activation"));
    assert!(state.root_fatal_signals.is_empty());
}
