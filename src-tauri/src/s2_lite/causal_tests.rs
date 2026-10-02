use super::canonical::{compare_commit_ref_v1, compare_entity_key_v1, jcs_bytes, sha256_hex};
use super::causal::{
    causally_covers_v1, compute_entity_frontier_v1, decode_frozen_wire_commit_v1,
    detect_duplicate_diagnostics_v1, detect_watch_tracker_relations_v1, detect_writer_forks_v1,
    replay_verified_history_v1, validate_commit_envelope_v1, validate_writer_chain_link_v1,
};
use super::semantic::{business_field_order, canonical_semantic_value};
use super::types::{
    CommitMutationV1, CommitRef, CommitSourceV1, CommitV1, HistoricalValidityState,
    MaterializedEntityV1, MaterializedEntryV1, VerifiedReplayV1,
};
use base64::Engine;
use serde_json::{json, Value};
use std::collections::{BTreeSet, HashMap};

fn causal_fixture() -> Value {
    serde_json::from_str(include_str!(
        "../../../contracts/s2-lite/v1/causal-golden-v1.json"
    ))
    .expect("valid shared causal fixture")
}

fn raw_wire_fixture() -> Value {
    serde_json::from_str(include_str!(
        "../../../contracts/s2-lite/v1/raw-wire-json-v1.json"
    ))
    .expect("valid shared raw wire fixture")
}

#[test]
fn shared_causal_fixture_lists_all_eighteen_mandatory_scenarios() {
    let fixture = causal_fixture();
    let names = fixture["cases"]
        .as_array()
        .unwrap()
        .iter()
        .take(18)
        .map(|case| case["name"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        names,
        [
            "single-writer-seq-1-3",
            "two-concurrent-writers",
            "one-writer-observes-other",
            "semantic-equivalent-concurrent-values",
            "same-base-disjoint-merge",
            "overlapping-field",
            "different-base",
            "live-tombstone",
            "locked-concurrent",
            "derived-domain",
            "parent-delete-member-create",
            "episode-total-shrink",
            "explicit-resolution",
            "stale-resolution",
            "resolution-late-alternative",
            "malformed-previous-ref",
            "missing-dependency-pending",
            "writer-fork-classification",
        ]
    );
}

#[test]
fn every_shared_causal_case_replays_to_explicit_observables() {
    let fixture = causal_fixture();
    for case in fixture["cases"].as_array().unwrap() {
        let commits: Vec<CommitV1> = serde_json::from_value(case["objects"].clone()).unwrap();
        let replay = replay_verified_history_v1(&commits).unwrap();
        let serialized = serde_json::to_value(&replay).unwrap();
        let entity_conflicts = serialized["materialized"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|item| item["value"]["state"] == "Conflict")
            .map(|item| item["value"]["conflictId"].clone())
            .collect::<Vec<_>>();
        let actual = json!({
            "validity": serialized["validity"],
            "frontiers": serialized["frontiers"],
            "materialized": serialized["materialized"],
            "entityConflicts": entity_conflicts,
            "relationConflicts": serialized["relations"]["conflicts"],
            "diagnostics": serialized["duplicateDiagnostics"],
            "forks": serialized["forks"],
            "unsafeCommitRefs": serialized["unsafeCommitRefs"]
        });
        assert_eq!(
            jcs_bytes(&actual).unwrap(),
            jcs_bytes(&case["expected"]).unwrap(),
            "{}",
            case["name"]
        );
        let validity_counts = ["VALID", "PENDING", "INVALID"]
            .iter()
            .map(|state| {
                serialized["validity"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .filter(|entry| entry["validity"]["state"] == *state)
                    .count()
            })
            .collect::<Vec<_>>();
        let assertions = &case["semanticAssertions"];
        let materialized_assertions = assertions["materialized"].as_array().unwrap();
        let materialized = serialized["materialized"]
            .as_array()
            .unwrap()
            .iter()
            .enumerate()
            .map(|(index, item)| {
                let frontier_count = serialized["frontiers"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|frontier| frontier["entityKey"] == item["entityKey"])
                    .unwrap()["frontier"]
                    .as_array()
                    .unwrap()
                    .len();
                if item["value"]["state"] == "Resolved" {
                    json!({
                        "state": "Resolved",
                        "frontierCount": frontier_count,
                        "provenanceCount": item["value"]["provenanceFrontier"].as_array().unwrap().len()
                    })
                } else {
                    let mut value = json!({
                        "state": "Conflict",
                        "conflictKind": item["value"]["conflictKind"],
                        "frontierCount": frontier_count,
                        "provenanceCount": item["value"]["semanticAlternatives"].as_array().unwrap().len()
                    });
                    if materialized_assertions[index]
                        .as_object()
                        .unwrap()
                        .contains_key("exactConflictId")
                    {
                        value["exactConflictId"] = item["value"]["conflictId"].clone();
                        value["exactFrontierCommitIds"] = Value::Array(
                            item["value"]["frontier"]
                                .as_array()
                                .unwrap()
                                .iter()
                                .map(|reference| reference["commitId"].clone())
                                .collect(),
                        );
                    }
                    value
                }
            })
            .collect::<Vec<_>>();
        let mut semantic_guard = json!({
            "validityCounts": validity_counts,
            "materialized": materialized
        });
        if !replay.relations.conflicts.is_empty() {
            semantic_guard["relationConflictKinds"] = Value::Array(
                replay
                    .relations
                    .conflicts
                    .iter()
                    .map(|conflict| json!(conflict.core.relation_kind))
                    .collect(),
            );
        }
        if !replay.forks.is_empty() {
            semantic_guard["forkCount"] = json!(replay.forks.len());
        }
        if !replay.unsafe_commit_refs.is_empty() {
            semantic_guard["unsafeCommitCount"] = json!(replay.unsafe_commit_refs.len());
        }
        assert_eq!(
            jcs_bytes(&semantic_guard).unwrap(),
            jcs_bytes(assertions).unwrap(),
            "{}: manual semantic guard",
            case["name"]
        );
    }
}

#[test]
fn rust_replay_matches_shared_exact_causal_parity_digest() {
    let fixture = causal_fixture();
    let commits: Vec<CommitV1> =
        serde_json::from_value(fixture["parityScenario"]["commits"].clone()).unwrap();
    let replay = replay_verified_history_v1(&commits).unwrap();
    assert_eq!(
        sha256_hex(&jcs_bytes(&replay).unwrap()),
        fixture["parityScenario"]["expectedReplaySha256"]
    );
}

#[test]
fn frozen_raw_wire_decoder_enforces_presence_numeric_versions_and_uuid_v4() {
    let fixture = causal_fixture();
    let wire = fixture["rawWireCommit"].clone();
    let decoded = decode_frozen_wire_commit_v1(&serde_json::to_vec(&wire).unwrap()).unwrap();
    assert_eq!(decoded.protocol_version, 1);
    assert_eq!(decoded.s2_semantic_profile_version, 1);
    assert_eq!(decoded.source.r#type, "native");
    assert_eq!(decoded.mutations[0].entity_type, "collection");

    let mut missing_previous = wire.clone();
    missing_previous
        .as_object_mut()
        .unwrap()
        .remove("previousWriterCommit");
    assert_eq!(
        decode_frozen_wire_commit_v1(&serde_json::to_vec(&missing_previous).unwrap())
            .unwrap_err()
            .0,
        "invalid_commit_envelope"
    );
    let mut string_version = wire.clone();
    string_version["protocolVersion"] = json!("1");
    assert_eq!(
        decode_frozen_wire_commit_v1(&serde_json::to_vec(&string_version).unwrap())
            .unwrap_err()
            .0,
        "unsupported_protocol_version"
    );
    let mut bootstrap = wire;
    bootstrap["commitKind"] = json!("bootstrap");
    bootstrap["source"]["type"] = json!("legacy-bootstrap");
    assert_eq!(
        decode_frozen_wire_commit_v1(&serde_json::to_vec(&bootstrap).unwrap())
            .unwrap()
            .source
            .r#type,
        "legacy-bootstrap"
    );

    let valid = record_create(6, 6, &[]);
    for bad in [
        "10000000-0000-1000-8000-000000000006",
        "10000000-0000-7000-8000-000000000006",
        "10000000-0000-4000-8000-00000000000A",
        "not-a-uuid",
    ] {
        let mut candidate = valid.clone();
        candidate.writer_id = bad.to_string();
        assert!(validate_commit_envelope_v1(&candidate).is_err());
    }
    validate_commit_envelope_v1(&valid).unwrap();
}

#[test]
fn strict_raw_byte_fixture_has_identical_accept_reject_and_exact_hash_semantics() {
    let fixture = raw_wire_fixture();
    let cases = fixture["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 20);
    for vector in cases {
        let name = vector["name"].as_str().unwrap();
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(vector["utf8Base64"].as_str().unwrap())
            .unwrap();
        assert_eq!(
            sha256_hex(&bytes),
            vector["expectedContentHash"].as_str().unwrap(),
            "{name}: raw hash oracle"
        );
        let result = decode_frozen_wire_commit_v1(&bytes);
        if vector["expected"] == "accept" {
            let decoded = result.unwrap_or_else(|error| panic!("{name}: {}", error.0));
            assert_eq!(
                decoded.content_hash,
                vector["expectedContentHash"].as_str().unwrap(),
                "{name}"
            );
            assert_eq!(
                jcs_bytes(&serde_json::to_value(&decoded).unwrap()).unwrap(),
                jcs_bytes(&vector["expectedNormalizedCommit"]).unwrap(),
                "{name}: normalized commit"
            );
            match name {
                "canonical-mutation-resolves-absent" => {
                    assert_eq!(decoded.commit_kind, "mutation");
                    assert!(decoded.resolves.is_empty());
                }
                "canonical-resolution" => {
                    assert_eq!(decoded.commit_kind, "resolution");
                    assert_eq!(decoded.source.r#type, "manual-resolution");
                }
                "legacy-bootstrap" | "new-root-bootstrap" => {
                    assert_eq!(decoded.commit_kind, "bootstrap");
                    assert!(decoded.resolves.is_empty());
                    assert_eq!(
                        status(
                            &replay_verified_history_v1(std::slice::from_ref(&decoded)).unwrap(),
                            &decoded
                        )
                        .state,
                        HistoricalValidityState::Valid
                    );
                }
                "safe-integer-lexical-float" => {
                    assert_eq!(decoded.mutations[0].entity_key[2], json!(5.0));
                    assert_eq!(decoded.mutations[0].value["episodeNumber"], json!(5.0));
                }
                _ => {}
            }
        } else {
            assert!(result.is_err(), "{name}");
        }
    }
    let bom = cases
        .iter()
        .find(|vector| vector["name"] == "utf8-bom")
        .unwrap();
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(bom["utf8Base64"].as_str().unwrap())
        .unwrap();
    assert_eq!(
        decode_frozen_wire_commit_v1(&bytes).unwrap_err().0,
        "invalid_commit_json_bytes"
    );
}

fn phase0_fixture() -> Value {
    serde_json::from_str(include_str!(
        "../../../contracts/s2-lite/v1/conflict-golden-v1.json"
    ))
    .unwrap()
}

fn writer(number: u32) -> String {
    format!("10000000-0000-4000-8000-{number:012}")
}

fn uuid(number: u32) -> String {
    format!("20000000-0000-4000-8000-{number:012}")
}

fn hash(number: u32) -> String {
    format!("{number:064x}")
}

fn full_record(id: &str, patch: &[(&str, Value)]) -> Value {
    let fixture = phase0_fixture();
    let mut value = fixture["semanticValues"]["recordChanged"]
        .as_object()
        .unwrap()
        .clone();
    value.insert("id".to_string(), json!(id));
    value.insert("notes".to_string(), json!(""));
    value.insert("isLocked".to_string(), json!(false));
    value.insert("createdAt".to_string(), json!("2026-09-06T10:00:00.000Z"));
    value.insert("updatedAt".to_string(), json!("2026-09-06T10:00:00.000Z"));
    value.insert("rev".to_string(), json!("0"));
    value.insert("revActor".to_string(), json!(""));
    for (field, item) in patch {
        value.insert((*field).to_string(), item.clone());
    }
    Value::Object(value)
}

fn full_collection(id: &str, patch: &[(&str, Value)]) -> Value {
    let fixture = phase0_fixture();
    let mut value = fixture["semanticValues"]["collectionOne"]
        .as_object()
        .unwrap()
        .clone();
    value.insert("id".to_string(), json!(id));
    value.insert("name".to_string(), json!("Collection"));
    value.insert("normalizedName".to_string(), json!("collection"));
    value.insert("createdAt".to_string(), json!("2026-09-06T10:00:00.000Z"));
    value.insert("updatedAt".to_string(), json!("2026-09-06T10:00:00.000Z"));
    value.insert("rev".to_string(), json!("0"));
    value.insert("revActor".to_string(), json!(""));
    for (field, item) in patch {
        value.insert((*field).to_string(), item.clone());
    }
    Value::Object(value)
}

fn deterministic_id(domain: &str, components: &[String]) -> String {
    let mut bytes = domain.as_bytes().to_vec();
    for component in components {
        bytes.push(0);
        bytes.extend_from_slice(component.as_bytes());
    }
    sha256_hex(&bytes)
}

fn tombstone(id: &str) -> Value {
    json!({
        "id": id,
        "deletedAt": "2026-09-06T10:00:00.000Z",
        "rev": "1",
        "revActor": "test"
    })
}

fn reference(commit: &CommitV1) -> CommitRef {
    commit.commit_ref()
}

fn sorted_refs(commits: &[&CommitV1]) -> Vec<CommitRef> {
    let mut refs = commits
        .iter()
        .map(|commit| reference(commit))
        .collect::<Vec<_>>();
    refs.sort_by(compare_commit_ref_v1);
    refs
}

fn mutation(
    number: u32,
    entity_key: Value,
    operation: &str,
    value: Value,
    base_frontier: Vec<CommitRef>,
    changed_fields: &[&str],
) -> CommitMutationV1 {
    CommitMutationV1 {
        local_mutation_id: uuid(10_000 + number),
        entity_type: entity_key[0].as_str().unwrap().to_string(),
        entity_key,
        operation: operation.to_string(),
        value,
        base_frontier,
        changed_fields: changed_fields
            .iter()
            .map(|field| (*field).to_string())
            .collect(),
    }
}

fn make_commit(
    number: u32,
    writer_number: u32,
    seq: u64,
    previous: Option<&CommitV1>,
    basis: &[&CommitV1],
    mutations: Vec<CommitMutationV1>,
    resolution: Option<Vec<String>>,
) -> CommitV1 {
    CommitV1 {
        protocol: "watchtracker-s2-lite".to_string(),
        protocol_version: 1,
        s2_semantic_profile_version: 1,
        required_features: vec![],
        writer_id: writer(writer_number),
        writer_seq: seq.to_string(),
        commit_id: uuid(number),
        content_hash: hash(number),
        previous_writer_commit: previous.map(reference),
        basis_clock: sorted_refs(basis),
        commit_kind: if resolution.is_some() {
            "resolution".to_string()
        } else {
            "mutation".to_string()
        },
        created_at: "2026-09-06T10:00:00.000Z".to_string(),
        source: CommitSourceV1 {
            r#type: if resolution.is_some() {
                "manual-resolution".to_string()
            } else {
                "native".to_string()
            },
        },
        resolves: resolution.unwrap_or_default(),
        mutations,
    }
}

fn record_create(number: u32, writer_number: u32, patch: &[(&str, Value)]) -> CommitV1 {
    make_commit(
        number,
        writer_number,
        1,
        None,
        &[],
        vec![mutation(
            number,
            json!(["record", "r1"]),
            "upsert",
            full_record("r1", patch),
            vec![],
            business_field_order("record").unwrap(),
        )],
        None,
    )
}

fn record_update(
    number: u32,
    writer_number: u32,
    basis: &[&CommitV1],
    patch: &[(&str, Value)],
    changed_fields: &[&str],
) -> CommitV1 {
    make_commit(
        number,
        writer_number,
        1,
        None,
        basis,
        vec![mutation(
            number,
            json!(["record", "r1"]),
            "upsert",
            full_record("r1", patch),
            sorted_refs(basis),
            changed_fields,
        )],
        None,
    )
}

fn status<'a>(
    replay: &'a VerifiedReplayV1,
    commit: &CommitV1,
) -> &'a super::types::HistoricalValidity {
    replay
        .validity
        .iter()
        .find(|entry| compare_commit_ref_v1(&entry.commit_ref, &commit.commit_ref()).is_eq())
        .map(|entry| &entry.validity)
        .unwrap()
}

fn materialized<'a>(replay: &'a VerifiedReplayV1, key: &Value) -> &'a MaterializedEntityV1 {
    replay
        .materialized
        .iter()
        .find(|entry| compare_entity_key_v1(&entry.entity_key, key).is_eq())
        .map(|entry| &entry.value)
        .unwrap()
}

fn conflict_kind(value: &MaterializedEntityV1) -> Option<&str> {
    match value {
        MaterializedEntityV1::Conflict { conflict_kind, .. } => Some(conflict_kind),
        _ => None,
    }
}

fn resolved_state(
    state: &str,
    reference: CommitRef,
    business: Option<Value>,
) -> MaterializedEntityV1 {
    MaterializedEntityV1::Resolved {
        semantic_state: if state == "tombstone" {
            json!({"state": "tombstone"})
        } else {
            json!({"state": "live", "value": business.clone().unwrap()})
        },
        business_value: business,
        provenance_frontier: vec![reference],
        metadata_variants: vec![],
    }
}

#[test]
fn commit_chain_pending_and_fork_classification_are_fail_closed() {
    let one = record_create(1, 1, &[]);
    let two = make_commit(
        2,
        1,
        2,
        Some(&one),
        &[&one],
        vec![mutation(
            2,
            json!(["record", "r1"]),
            "upsert",
            full_record("r1", &[("notes", json!("two"))]),
            vec![reference(&one)],
            &["notes"],
        )],
        None,
    );
    let three = make_commit(
        3,
        1,
        3,
        Some(&two),
        &[&two],
        vec![mutation(
            3,
            json!(["record", "r1"]),
            "upsert",
            full_record("r1", &[("notes", json!("three"))]),
            vec![reference(&two)],
            &["notes"],
        )],
        None,
    );
    let replay = replay_verified_history_v1(&[three.clone(), one.clone(), two.clone()]).unwrap();
    assert!(replay
        .validity
        .iter()
        .all(|entry| entry.validity.state == HistoricalValidityState::Valid));

    let mut malformed = two.clone();
    malformed.basis_clock.clear();
    let malformed_replay = replay_verified_history_v1(&[one.clone(), malformed.clone()]).unwrap();
    assert_eq!(
        status(&malformed_replay, &malformed).error.as_deref(),
        Some("invalid_writer_causal_chain")
    );
    let missing = record_update(4, 4, &[&one], &[("notes", json!("pending"))], &["notes"]);
    assert_eq!(
        status(
            &replay_verified_history_v1(std::slice::from_ref(&missing)).unwrap(),
            &missing
        )
        .state,
        HistoricalValidityState::Pending
    );
    let fork_a = record_create(5, 5, &[("notes", json!("A"))]);
    let fork_b = record_create(6, 5, &[("notes", json!("B"))]);
    let forks = detect_writer_forks_v1(&[fork_b.clone(), fork_a.clone()]);
    assert_eq!(forks.len(), 1);
    assert_eq!(forks[0].safe_writer_frontier, "0");
    assert_eq!(forks[0].alternatives, sorted_refs(&[&fork_a, &fork_b]));

    let verified = [(
        format!(
            "{}\0{}\0{}\0{}",
            one.writer_id, one.writer_seq, one.commit_id, one.content_hash
        ),
        one.clone(),
    )]
    .into_iter()
    .collect::<HashMap<_, _>>();
    let mut gap = two.clone();
    gap.writer_seq = "3".to_string();
    let mut mismatched = two.clone();
    mismatched.basis_clock[0].commit_id = uuid(999);
    let mut mismatched_hash = two.clone();
    mismatched_hash
        .previous_writer_commit
        .as_mut()
        .unwrap()
        .content_hash = "f".repeat(64);
    let mut seq_one_own = one.clone();
    seq_one_own.basis_clock = vec![one.commit_ref()];
    for candidate in [gap, mismatched, mismatched_hash, seq_one_own] {
        assert_eq!(
            validate_writer_chain_link_v1(&candidate, &verified)
                .unwrap_err()
                .0,
            "invalid_writer_causal_chain"
        );
    }
}

#[test]
fn generated_commit_corruptions_have_stable_validity_outcomes() {
    let base = record_create(7, 7, &[]);
    let proper = make_commit(
        8,
        7,
        2,
        Some(&base),
        &[&base],
        vec![mutation(
            8,
            json!(["record", "r1"]),
            "upsert",
            full_record("r1", &[("notes", json!("next"))]),
            vec![reference(&base)],
            &["notes"],
        )],
        None,
    );
    let mut state = 0xc0ff_ee12_u32;
    for _ in 0..50 {
        state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        let mode = state % 5;
        let mut candidate = proper.clone();
        let mut inputs = vec![base.clone(), candidate.clone()];
        let expected = match mode {
            0 => {
                candidate
                    .previous_writer_commit
                    .as_mut()
                    .unwrap()
                    .content_hash = "f".repeat(64);
                inputs = vec![base.clone(), candidate.clone()];
                (
                    HistoricalValidityState::Invalid,
                    Some("invalid_writer_causal_chain"),
                )
            }
            1 => {
                let mut alternate_base = base.clone();
                alternate_base.commit_id = uuid(997);
                candidate.basis_clock = vec![reference(&alternate_base)];
                inputs = vec![alternate_base, candidate.clone()];
                (
                    HistoricalValidityState::Invalid,
                    Some("invalid_writer_causal_chain"),
                )
            }
            2 => {
                inputs = vec![candidate.clone()];
                (HistoricalValidityState::Pending, None)
            }
            3 => {
                candidate.mutations[0].base_frontier.clear();
                inputs = vec![base.clone(), candidate.clone()];
                (
                    HistoricalValidityState::Invalid,
                    Some("invalid_entity_base_frontier"),
                )
            }
            _ => {
                let mut copy = candidate.mutations[0].clone();
                copy.local_mutation_id = uuid(996);
                candidate.mutations.push(copy);
                inputs = vec![base.clone(), candidate.clone()];
                (
                    HistoricalValidityState::Invalid,
                    Some("duplicate_entity_key"),
                )
            }
        };
        for ordered in [inputs.clone(), permute(&inputs, state)] {
            let replay = replay_verified_history_v1(&ordered).unwrap();
            let actual = status(&replay, &candidate);
            assert_eq!(actual.state, expected.0);
            assert_eq!(actual.error.as_deref(), expected.1);
        }
    }
}

#[test]
fn dependency_graph_preserves_transitive_pending_and_marks_only_real_cycles() {
    let a = record_create(70, 70, &[]);
    let b = record_update(71, 71, &[&a], &[("notes", json!("B"))], &["notes"]);
    let c = record_update(72, 72, &[&b], &[("notes", json!("C"))], &["notes"]);
    let mut replay = replay_verified_history_v1(&[c.clone(), b.clone()]).unwrap();
    assert_eq!(status(&replay, &b).state, HistoricalValidityState::Pending);
    assert_eq!(status(&replay, &c).state, HistoricalValidityState::Pending);
    replay = replay_verified_history_v1(&[c.clone(), a, b]).unwrap();
    assert_eq!(status(&replay, &c).state, HistoricalValidityState::Valid);

    let mut invalid_dependency = record_create(73, 73, &[]);
    invalid_dependency.protocol = "invalid".to_string();
    let missing = record_create(74, 74, &[]);
    let mixed = make_commit(
        75,
        75,
        1,
        None,
        &[&invalid_dependency, &missing],
        vec![mutation(
            75,
            json!(["collection", "mixed"]),
            "upsert",
            full_collection("mixed", &[]),
            vec![],
            business_field_order("collection").unwrap(),
        )],
        None,
    );
    replay = replay_verified_history_v1(&[mixed.clone(), invalid_dependency]).unwrap();
    assert_eq!(
        status(&replay, &mixed).error.as_deref(),
        Some("invalid_causal_dependency")
    );

    let external = make_commit(
        751,
        751,
        1,
        None,
        &[],
        vec![mutation(
            751,
            json!(["collection", "external"]),
            "upsert",
            full_collection("external", &[]),
            vec![],
            business_field_order("collection").unwrap(),
        )],
        None,
    );
    let mut external_cycle_a = make_commit(
        752,
        752,
        1,
        None,
        &[],
        vec![mutation(
            752,
            json!(["collection", "external-cycle-a"]),
            "upsert",
            full_collection("external-cycle-a", &[]),
            vec![],
            business_field_order("collection").unwrap(),
        )],
        None,
    );
    let mut external_cycle_b = make_commit(
        753,
        753,
        1,
        None,
        &[],
        vec![mutation(
            753,
            json!(["collection", "external-cycle-b"]),
            "upsert",
            full_collection("external-cycle-b", &[]),
            vec![],
            business_field_order("collection").unwrap(),
        )],
        None,
    );
    let mut external_cycle_child = make_commit(
        754,
        754,
        1,
        None,
        &[],
        vec![mutation(
            754,
            json!(["collection", "external-cycle-child"]),
            "upsert",
            full_collection("external-cycle-child", &[]),
            vec![],
            business_field_order("collection").unwrap(),
        )],
        None,
    );
    external_cycle_a.basis_clock = sorted_refs(&[&external_cycle_b, &external]);
    external_cycle_b.basis_clock = vec![reference(&external_cycle_a)];
    external_cycle_child.basis_clock = vec![reference(&external_cycle_a)];
    for inputs in [
        vec![
            external_cycle_a.clone(),
            external_cycle_b.clone(),
            external_cycle_child.clone(),
        ],
        vec![
            external_cycle_a.clone(),
            external_cycle_b.clone(),
            external_cycle_child.clone(),
            external.clone(),
        ],
    ] {
        replay = replay_verified_history_v1(&inputs).unwrap();
        assert_eq!(
            status(&replay, &external_cycle_a).error.as_deref(),
            Some("causal_cycle")
        );
        assert_eq!(
            status(&replay, &external_cycle_b).error.as_deref(),
            Some("causal_cycle")
        );
        assert_eq!(
            status(&replay, &external_cycle_child).error.as_deref(),
            Some("invalid_causal_dependency")
        );
    }
    assert_eq!(
        status(&replay, &external).state,
        HistoricalValidityState::Valid
    );

    let mut external_self = make_commit(
        755,
        755,
        1,
        None,
        &[],
        vec![mutation(
            755,
            json!(["collection", "external-self"]),
            "upsert",
            full_collection("external-self", &[]),
            vec![],
            business_field_order("collection").unwrap(),
        )],
        None,
    );
    external_self.basis_clock = sorted_refs(&[&external_self.clone(), &external]);
    replay = replay_verified_history_v1(std::slice::from_ref(&external_self)).unwrap();
    assert_eq!(
        status(&replay, &external_self).error.as_deref(),
        Some("causal_cycle")
    );

    let mut cycle_a = record_create(76, 76, &[]);
    let mut cycle_b = record_create(77, 77, &[]);
    cycle_a.basis_clock = vec![reference(&cycle_b)];
    cycle_b.basis_clock = vec![reference(&cycle_a)];
    let descendant = record_update(
        78,
        78,
        &[&cycle_a],
        &[("notes", json!("descendant"))],
        &["notes"],
    );
    replay = replay_verified_history_v1(&[descendant.clone(), cycle_b.clone(), cycle_a.clone()])
        .unwrap();
    assert_eq!(
        status(&replay, &cycle_a).error.as_deref(),
        Some("causal_cycle")
    );
    assert_eq!(
        status(&replay, &cycle_b).error.as_deref(),
        Some("causal_cycle")
    );
    assert_eq!(
        status(&replay, &descendant).error.as_deref(),
        Some("invalid_causal_dependency")
    );

    let mut cycle_c = record_create(79, 79, &[]);
    cycle_b.basis_clock = vec![reference(&cycle_c)];
    cycle_c.basis_clock = vec![reference(&cycle_a)];
    replay =
        replay_verified_history_v1(&[cycle_c.clone(), cycle_a.clone(), cycle_b.clone()]).unwrap();
    for item in [&cycle_a, &cycle_b, &cycle_c] {
        assert_eq!(status(&replay, item).error.as_deref(), Some("causal_cycle"));
    }

    let diamond_d = make_commit(
        790,
        790,
        1,
        None,
        &[],
        vec![mutation(
            790,
            json!(["collection", "diamond-d"]),
            "upsert",
            full_collection("diamond-d", &[]),
            vec![],
            business_field_order("collection").unwrap(),
        )],
        None,
    );
    let diamond_c = make_commit(
        791,
        791,
        1,
        None,
        &[&diamond_d],
        vec![mutation(
            791,
            json!(["collection", "diamond-c"]),
            "upsert",
            full_collection("diamond-c", &[]),
            vec![],
            business_field_order("collection").unwrap(),
        )],
        None,
    );
    let diamond_b = make_commit(
        792,
        792,
        1,
        None,
        &[&diamond_c],
        vec![mutation(
            792,
            json!(["collection", "diamond-b"]),
            "upsert",
            full_collection("diamond-b", &[]),
            vec![],
            business_field_order("collection").unwrap(),
        )],
        None,
    );
    let diamond_a = make_commit(
        793,
        793,
        1,
        None,
        &[&diamond_b, &diamond_c],
        vec![mutation(
            793,
            json!(["collection", "diamond-a"]),
            "upsert",
            full_collection("diamond-a", &[]),
            vec![],
            business_field_order("collection").unwrap(),
        )],
        None,
    );
    replay = replay_verified_history_v1(&[diamond_a.clone(), diamond_b.clone(), diamond_c.clone()])
        .unwrap();
    for item in [&diamond_a, &diamond_b, &diamond_c] {
        assert_eq!(
            status(&replay, item).state,
            HistoricalValidityState::Pending
        );
    }
    replay = replay_verified_history_v1(&[
        diamond_a.clone(),
        diamond_b.clone(),
        diamond_c.clone(),
        diamond_d.clone(),
    ])
    .unwrap();
    for item in [&diamond_a, &diamond_b, &diamond_c, &diamond_d] {
        assert_eq!(status(&replay, item).state, HistoricalValidityState::Valid);
    }
}

#[test]
fn tarjan_classification_matches_independent_exhaustive_three_node_oracle() {
    const NODE_COUNT: usize = 3;
    let edge_slots = (0..NODE_COUNT)
        .flat_map(|from| {
            (0..NODE_COUNT)
                .filter(move |to| *to != from)
                .map(move |to| (from, to))
        })
        .collect::<Vec<_>>();
    for mask in 0_u32..(1_u32 << edge_slots.len()) {
        let mut nodes = (0..NODE_COUNT)
            .map(|index| {
                let number = 8_000 + mask * 10 + index as u32;
                let id = format!("graph-{mask}-{index}");
                make_commit(
                    number,
                    number,
                    1,
                    None,
                    &[],
                    vec![mutation(
                        number,
                        json!(["collection", id]),
                        "upsert",
                        full_collection(&id, &[]),
                        vec![],
                        business_field_order("collection").unwrap(),
                    )],
                    None,
                )
            })
            .collect::<Vec<_>>();
        let mut reach = [[false; NODE_COUNT]; NODE_COUNT];
        for (bit, (from, to)) in edge_slots.iter().enumerate() {
            if mask & (1 << bit) != 0 {
                reach[*from][*to] = true;
            }
        }
        let references = nodes.iter().map(reference).collect::<Vec<_>>();
        for from in 0..NODE_COUNT {
            nodes[from].basis_clock = (0..NODE_COUNT)
                .filter(|to| reach[from][*to])
                .map(|to| references[to].clone())
                .collect();
            nodes[from].basis_clock.sort_by(compare_commit_ref_v1);
        }
        for via in 0..NODE_COUNT {
            for from in 0..NODE_COUNT {
                for to in 0..NODE_COUNT {
                    reach[from][to] |= reach[from][via] && reach[via][to];
                }
            }
        }
        let cycle_nodes = (0..NODE_COUNT)
            .filter(|index| reach[*index][*index])
            .collect::<BTreeSet<_>>();
        let replay = replay_verified_history_v1(&permute(&nodes, mask)).unwrap();
        for index in 0..NODE_COUNT {
            let actual = status(&replay, &nodes[index]);
            if cycle_nodes.contains(&index) {
                assert_eq!(actual.error.as_deref(), Some("causal_cycle"), "mask {mask}");
            } else if cycle_nodes.iter().any(|cycle| reach[index][*cycle]) {
                assert_eq!(
                    actual.error.as_deref(),
                    Some("invalid_causal_dependency"),
                    "mask {mask}"
                );
            } else {
                assert_eq!(actual.state, HistoricalValidityState::Valid, "mask {mask}");
            }
        }
    }

    let mut self_loop = make_commit(
        9_000,
        9_000,
        1,
        None,
        &[],
        vec![mutation(
            9_000,
            json!(["collection", "self-loop"]),
            "upsert",
            full_collection("self-loop", &[]),
            vec![],
            business_field_order("collection").unwrap(),
        )],
        None,
    );
    self_loop.basis_clock = vec![reference(&self_loop)];
    let replay = replay_verified_history_v1(std::slice::from_ref(&self_loop)).unwrap();
    assert_eq!(
        status(&replay, &self_loop).error.as_deref(),
        Some("causal_cycle")
    );
}

#[test]
fn scc_classification_matches_448_independent_external_missing_graph_variants() {
    const NODE_COUNT: usize = 3;
    let edge_slots = (0..NODE_COUNT)
        .flat_map(|from| {
            (0..NODE_COUNT)
                .filter(move |to| *to != from)
                .map(move |to| (from, to))
        })
        .collect::<Vec<_>>();
    for mask in 0_u32..(1_u32 << edge_slots.len()) {
        for missing_mask in 1_u32..(1_u32 << NODE_COUNT) {
            let base_number = 20_000 + mask * 100 + missing_mask * 10;
            let mut nodes = (0..NODE_COUNT)
                .map(|index| {
                    let number = base_number + index as u32;
                    let id = format!("missing-{mask}-{missing_mask}-{index}");
                    make_commit(
                        number,
                        number,
                        1,
                        None,
                        &[],
                        vec![mutation(
                            number,
                            json!(["collection", id]),
                            "upsert",
                            full_collection(&id, &[]),
                            vec![],
                            business_field_order("collection").unwrap(),
                        )],
                        None,
                    )
                })
                .collect::<Vec<_>>();
            let missing = (0..NODE_COUNT)
                .map(|index| {
                    let number = base_number + NODE_COUNT as u32 + index as u32;
                    let id = format!("absent-{mask}-{missing_mask}-{index}");
                    make_commit(
                        number,
                        number,
                        1,
                        None,
                        &[],
                        vec![mutation(
                            number,
                            json!(["collection", id]),
                            "upsert",
                            full_collection(&id, &[]),
                            vec![],
                            business_field_order("collection").unwrap(),
                        )],
                        None,
                    )
                })
                .collect::<Vec<_>>();
            let mut reach = [[false; NODE_COUNT]; NODE_COUNT];
            for (bit, (from, to)) in edge_slots.iter().enumerate() {
                if mask & (1 << bit) != 0 {
                    reach[*from][*to] = true;
                }
            }
            let internal_references = nodes.iter().map(reference).collect::<Vec<_>>();
            let missing_references = missing.iter().map(reference).collect::<Vec<_>>();
            for from in 0..NODE_COUNT {
                nodes[from].basis_clock = (0..NODE_COUNT)
                    .filter(|to| reach[from][*to])
                    .map(|to| internal_references[to].clone())
                    .collect();
                if missing_mask & (1 << from) != 0 {
                    nodes[from]
                        .basis_clock
                        .push(missing_references[from].clone());
                }
                nodes[from].basis_clock.sort_by(compare_commit_ref_v1);
            }
            for via in 0..NODE_COUNT {
                for from in 0..NODE_COUNT {
                    for to in 0..NODE_COUNT {
                        reach[from][to] |= reach[from][via] && reach[via][to];
                    }
                }
            }
            let cycle_nodes = (0..NODE_COUNT)
                .filter(|index| reach[*index][*index])
                .collect::<BTreeSet<_>>();
            let replay = replay_verified_history_v1(&permute(
                &nodes,
                mask * (1 << NODE_COUNT) + missing_mask,
            ))
            .unwrap();
            for index in 0..NODE_COUNT {
                let actual = status(&replay, &nodes[index]);
                if cycle_nodes.contains(&index) {
                    assert_eq!(
                        actual.error.as_deref(),
                        Some("causal_cycle"),
                        "graph={mask} missing={missing_mask} node={index}"
                    );
                } else if cycle_nodes.iter().any(|cycle| reach[index][*cycle]) {
                    assert_eq!(
                        actual.error.as_deref(),
                        Some("invalid_causal_dependency"),
                        "graph={mask} missing={missing_mask} node={index}"
                    );
                } else {
                    let transitively_missing = missing_mask & (1 << index) != 0
                        || (0..NODE_COUNT).any(|target| {
                            missing_mask & (1 << target) != 0 && reach[index][target]
                        });
                    let expected = if transitively_missing {
                        HistoricalValidityState::Pending
                    } else {
                        HistoricalValidityState::Valid
                    };
                    assert_eq!(
                        actual.state, expected,
                        "graph={mask} missing={missing_mask} node={index}"
                    );
                }
            }
        }
    }
}

#[test]
fn fork_branches_and_cross_writer_descendants_are_forensic_only() {
    let equivalent_a = record_create(90, 90, &[("notes", json!("same"))]);
    let equivalent_b = record_create(91, 90, &[("notes", json!("same"))]);
    let different = record_create(92, 90, &[("notes", json!("different"))]);
    let own_descendant = make_commit(
        93,
        90,
        2,
        Some(&equivalent_a),
        &[&equivalent_a],
        vec![mutation(
            93,
            json!(["record", "r1"]),
            "upsert",
            full_record("r1", &[("notes", json!("own descendant"))]),
            vec![reference(&equivalent_a)],
            &["notes"],
        )],
        None,
    );
    let cross_descendant = record_update(
        94,
        94,
        &[&different],
        &[("notes", json!("cross descendant"))],
        &["notes"],
    );
    let commits = vec![
        equivalent_a.clone(),
        different.clone(),
        own_descendant.clone(),
        cross_descendant.clone(),
    ];
    for seed in 0..16 {
        let replay = replay_verified_history_v1(&permute(&commits, seed)).unwrap();
        assert_eq!(replay.forks.len(), 1);
        assert_eq!(replay.forks[0].safe_writer_frontier, "0");
        assert_eq!(replay.forensic_versions.len(), 4);
        assert!(replay.versions.is_empty());
        assert!(replay.materialized.is_empty());
        assert_eq!(
            replay.unsafe_commit_refs,
            sorted_refs(&[
                &equivalent_a,
                &different,
                &own_descendant,
                &cross_descendant
            ])
        );
    }
    let mut same_dot_different_hash = equivalent_a.clone();
    same_dot_different_hash.content_hash = "f".repeat(64);
    same_dot_different_hash.mutations[0].value["updatedAt"] = json!("2026-09-06T11:00:00.000Z");
    let replay = replay_verified_history_v1(&[same_dot_different_hash, equivalent_a]).unwrap();
    assert_eq!(replay.forks.len(), 1);
    assert_eq!(replay.forensic_versions.len(), 2);
    assert!(replay.versions.is_empty());

    let replay = replay_verified_history_v1(&[
        equivalent_b,
        record_create(95, 90, &[("notes", json!("same"))]),
    ])
    .unwrap();
    assert_eq!(replay.forensic_versions.len(), 2);
    assert!(replay.versions.is_empty());
}

#[test]
fn relation_detector_emits_four_frozen_kinds_and_blocks_entity_conflicts() {
    let fixture = phase0_fixture();
    let ra: CommitRef = serde_json::from_value(fixture["refs"]["RA"].clone()).unwrap();
    let rb: CommitRef = serde_json::from_value(fixture["refs"]["RB"].clone()).unwrap();
    let member = resolved_state(
        "live",
        rb.clone(),
        Some(json!({
            "collectionId": "c", "recordId": "r", "position": "0", "sourceKind": "manual"
        })),
    );
    let episode = resolved_state(
        "live",
        rb.clone(),
        Some(json!({"recordId": "r", "episodeNumber": 12, "completedAt": null})),
    );
    let tomb = resolved_state("tombstone", ra.clone(), None);
    let record = resolved_state("live", ra.clone(), Some(json!({"totalEpisodes": 10})));
    let cases = vec![
        (
            vec![
                MaterializedEntryV1 {
                    entity_key: json!(["collection", "c"]),
                    value: tomb.clone(),
                },
                MaterializedEntryV1 {
                    entity_key: json!(["collection-member", "c", "r"]),
                    value: member.clone(),
                },
            ],
            "collection-deleted-member-live",
        ),
        (
            vec![
                MaterializedEntryV1 {
                    entity_key: json!(["record", "r"]),
                    value: tomb.clone(),
                },
                MaterializedEntryV1 {
                    entity_key: json!(["collection-member", "c", "r"]),
                    value: member,
                },
            ],
            "record-deleted-member-live",
        ),
        (
            vec![
                MaterializedEntryV1 {
                    entity_key: json!(["record", "r"]),
                    value: tomb,
                },
                MaterializedEntryV1 {
                    entity_key: json!(["episode-completion", "r", 12]),
                    value: episode.clone(),
                },
            ],
            "record-deleted-episode-live",
        ),
        (
            vec![
                MaterializedEntryV1 {
                    entity_key: json!(["record", "r"]),
                    value: record,
                },
                MaterializedEntryV1 {
                    entity_key: json!(["episode-completion", "r", 12]),
                    value: episode.clone(),
                },
            ],
            "episode-exceeds-total",
        ),
    ];
    for (entities, expected) in cases {
        let actual = detect_watch_tracker_relations_v1(&entities).unwrap();
        assert_eq!(actual.conflicts.len(), 1);
        assert_eq!(actual.conflicts[0].core.relation_kind, expected);
        assert!(actual.blocked_by_entity_conflict.is_empty());
    }

    let conflict = MaterializedEntityV1::Conflict {
        conflict_id: "0".repeat(64),
        conflict_kind: "overlapping-field".to_string(),
        entity_key: json!(["record", "r"]),
        frontier: vec![ra, rb],
        conflict_fields: vec!["notes".to_string()],
        semantic_alternatives: vec![],
    };
    let blocked = detect_watch_tracker_relations_v1(&[
        MaterializedEntryV1 {
            entity_key: json!(["record", "r"]),
            value: conflict,
        },
        MaterializedEntryV1 {
            entity_key: json!(["episode-completion", "r", 12]),
            value: episode,
        },
    ])
    .unwrap();
    assert!(blocked.conflicts.is_empty());
    assert_eq!(blocked.blocked_by_entity_conflict.len(), 2);
}

#[test]
fn numeric_semantics_batch_shadow_and_tracking_false_match_frozen_rules() {
    let fixture = phase0_fixture();
    let ra: CommitRef = serde_json::from_value(fixture["refs"]["RA"].clone()).unwrap();
    let rb: CommitRef = serde_json::from_value(fixture["refs"]["RB"].clone()).unwrap();
    let numeric_relations = detect_watch_tracker_relations_v1(&[
        MaterializedEntryV1 {
            entity_key: json!(["record", "numeric"]),
            value: resolved_state("live", ra, Some(json!({"totalEpisodes": 3.0}))),
        },
        MaterializedEntryV1 {
            entity_key: json!(["episode-completion", "numeric", 5.0]),
            value: resolved_state(
                "live",
                rb,
                Some(json!({"recordId": "numeric", "episodeNumber": 5.0, "completedAt": null})),
            ),
        },
    ])
    .unwrap();
    assert_eq!(numeric_relations.conflicts.len(), 1);
    assert_eq!(
        numeric_relations.conflicts[0].core.relation_kind,
        "episode-exceeds-total"
    );

    let integer = record_create(
        201,
        201,
        &[
            ("totalEpisodes", json!(3)),
            ("episodeTrackingEnabled", json!(false)),
        ],
    );
    let mut float = integer.clone();
    float.mutations[0].value["totalEpisodes"] = json!(3.0);
    let integer_first =
        jcs_bytes(&replay_verified_history_v1(&[integer.clone(), float.clone()]).unwrap()).unwrap();
    let float_first =
        jcs_bytes(&replay_verified_history_v1(&[float.clone(), integer.clone()]).unwrap()).unwrap();
    assert_eq!(integer_first, float_first);
    assert_eq!(
        integer_first,
        jcs_bytes(
            &replay_verified_history_v1(&[integer.clone(), float.clone(), integer.clone()])
                .unwrap()
        )
        .unwrap()
    );

    let parent = record_create(
        202,
        202,
        &[
            ("totalEpisodes", json!(10.0)),
            ("episodeTrackingEnabled", json!(false)),
        ],
    );
    let episode_value = json!({
        "id": deterministic_id("episode-completion:v1", &["r1".into(), "5".into()]),
        "recordId": "r1", "episodeNumber": 5.0, "completedAt": null,
        "createdAt": "2026-09-06T10:00:00.000Z", "updatedAt": "2026-09-06T10:00:00.000Z",
        "rev": "0", "revActor": ""
    });
    let episode = make_commit(
        203,
        203,
        1,
        None,
        &[&parent],
        vec![mutation(
            203,
            json!(["episode-completion", "r1", 5.0]),
            "upsert",
            episode_value,
            vec![],
            business_field_order("episode-completion").unwrap(),
        )],
        None,
    );
    let replay = replay_verified_history_v1(&[episode.clone(), parent.clone()]).unwrap();
    assert_eq!(
        status(&replay, &episode).state,
        HistoricalValidityState::Valid
    );
    let numeric_shrink = record_update(
        2031,
        2031,
        &[&parent],
        &[
            ("totalEpisodes", json!(3.0)),
            ("episodeTrackingEnabled", json!(false)),
        ],
        &["totalEpisodes"],
    );
    let replay =
        replay_verified_history_v1(&[episode.clone(), parent.clone(), numeric_shrink.clone()])
            .unwrap();
    assert_eq!(replay.relations.conflicts.len(), 1);
    assert_eq!(
        replay.relations.conflicts[0].core.semantic_relation_facts["episodeNumber"],
        5
    );
    assert_eq!(
        replay.relations.conflicts[0].core.semantic_relation_facts["totalEpisodes"],
        3
    );
    let numeric_ordinary = make_commit(
        2032,
        2032,
        1,
        None,
        &[&numeric_shrink, &episode],
        vec![mutation(
            2032,
            json!(["record", "r1"]),
            "upsert",
            full_record(
                "r1",
                &[
                    ("totalEpisodes", json!(10.0)),
                    ("episodeTrackingEnabled", json!(false)),
                ],
            ),
            vec![reference(&numeric_shrink)],
            &["totalEpisodes"],
        )],
        None,
    );
    let replay = replay_verified_history_v1(&[
        episode.clone(),
        parent.clone(),
        numeric_shrink.clone(),
        numeric_ordinary.clone(),
    ])
    .unwrap();
    assert_eq!(
        status(&replay, &numeric_ordinary).error.as_deref(),
        Some("ordinary_mutation_blocked_by_relation_conflict")
    );

    let delete_parent = mutation(
        2041,
        json!(["record", "r1"]),
        "tombstone",
        tombstone("r1"),
        vec![reference(&parent)],
        &["$tombstone"],
    );
    let create_child = mutation(
        2042,
        json!(["episode-completion", "r1", 6]),
        "upsert",
        json!({
            "id": deterministic_id("episode-completion:v1", &["r1".into(), "6".into()]),
            "recordId": "r1", "episodeNumber": 6, "completedAt": null,
            "createdAt": "2026-09-06T10:00:00.000Z", "updatedAt": "2026-09-06T10:00:00.000Z",
            "rev": "0", "revActor": ""
        }),
        vec![],
        business_field_order("episode-completion").unwrap(),
    );
    for mutations in [
        vec![delete_parent.clone(), create_child.clone()],
        vec![create_child.clone(), delete_parent.clone()],
    ] {
        let batch = make_commit(204, 204, 1, None, &[&parent], mutations, None);
        let replay = replay_verified_history_v1(&[parent.clone(), batch.clone()]).unwrap();
        assert_eq!(
            status(&replay, &batch).error.as_deref(),
            Some("invalid_episode_parent_basis")
        );
        assert_eq!(replay.forensic_versions.len(), 1);
    }

    let member_parent = make_commit(
        205,
        205,
        1,
        None,
        &[],
        vec![
            mutation(
                2051,
                json!(["record", "member-record"]),
                "upsert",
                full_record("member-record", &[]),
                vec![],
                business_field_order("record").unwrap(),
            ),
            mutation(
                2052,
                json!(["collection", "member-collection"]),
                "upsert",
                full_collection("member-collection", &[]),
                vec![],
                business_field_order("collection").unwrap(),
            ),
        ],
        None,
    );
    let delete_collection = mutation(
        2061,
        json!(["collection", "member-collection"]),
        "tombstone",
        tombstone("member-collection"),
        vec![reference(&member_parent)],
        &["$tombstone"],
    );
    let create_member = mutation(
        2062,
        json!(["collection-member", "member-collection", "member-record"]),
        "upsert",
        json!({
            "id": deterministic_id("collection-member:v1", &["member-collection".into(), "member-record".into()]),
            "collectionId": "member-collection", "recordId": "member-record",
            "position": "0", "sourceKind": "manual",
            "createdAt": "2026-09-06T10:00:00.000Z", "updatedAt": "2026-09-06T10:00:00.000Z",
            "rev": "0", "revActor": ""
        }),
        vec![],
        business_field_order("collection-member").unwrap(),
    );
    for mutations in [
        vec![delete_collection.clone(), create_member.clone()],
        vec![create_member, delete_collection],
    ] {
        let batch = make_commit(206, 206, 1, None, &[&member_parent], mutations, None);
        let replay = replay_verified_history_v1(&[member_parent.clone(), batch.clone()]).unwrap();
        assert_eq!(
            status(&replay, &batch).error.as_deref(),
            Some("invalid_member_parent_basis")
        );
        assert_eq!(replay.forensic_versions.len(), 2);
    }
}

#[test]
fn duplicate_diagnostics_preserve_entities_and_ignore_insertion_order() {
    let fixture = phase0_fixture();
    let ra: CommitRef = serde_json::from_value(fixture["refs"]["RA"].clone()).unwrap();
    let rb: CommitRef = serde_json::from_value(fixture["refs"]["RB"].clone()).unwrap();
    let record_a =
        canonical_semantic_value(&full_record("r-a", &[("imdbId", json!("tt123"))])).unwrap();
    let record_b =
        canonical_semantic_value(&full_record("r-b", &[("imdbId", json!("tt123"))])).unwrap();
    let collection_a = canonical_semantic_value(&full_collection("c-a", &[])).unwrap();
    let collection_b = canonical_semantic_value(&full_collection("c-b", &[])).unwrap();
    let entities = vec![
        MaterializedEntryV1 {
            entity_key: json!(["record", "r-a"]),
            value: resolved_state("live", ra.clone(), Some(record_a)),
        },
        MaterializedEntryV1 {
            entity_key: json!(["record", "r-b"]),
            value: resolved_state("live", rb.clone(), Some(record_b)),
        },
        MaterializedEntryV1 {
            entity_key: json!(["collection", "c-a"]),
            value: resolved_state("live", ra, Some(collection_a)),
        },
        MaterializedEntryV1 {
            entity_key: json!(["collection", "c-b"]),
            value: resolved_state("live", rb, Some(collection_b)),
        },
    ];
    let forward = detect_duplicate_diagnostics_v1(&entities).unwrap();
    let reverse =
        detect_duplicate_diagnostics_v1(&entities.iter().cloned().rev().collect::<Vec<_>>())
            .unwrap();
    assert_eq!(jcs_bytes(&forward).unwrap(), jcs_bytes(&reverse).unwrap());
    assert_eq!(
        forward
            .iter()
            .map(|item| item.kind.as_str())
            .collect::<Vec<_>>(),
        [
            "duplicate-collection-normalized-name",
            "duplicate-record-external-identity"
        ]
    );
}

#[test]
fn reducer_precedence_preserves_provenance_and_never_synthesizes_refs() {
    let base = record_create(10, 10, &[]);
    let same_a = record_update(11, 11, &[&base], &[("notes", json!("same"))], &["notes"]);
    let mut same_b = record_update(12, 12, &[&base], &[("notes", json!("same"))], &["notes"]);
    same_b.mutations[0].value["updatedAt"] = json!("2026-09-06T13:00:00.000Z");
    let replay =
        replay_verified_history_v1(&[same_b.clone(), base.clone(), same_a.clone()]).unwrap();
    match materialized(&replay, &json!(["record", "r1"])) {
        MaterializedEntityV1::Resolved {
            business_value: Some(value),
            provenance_frontier,
            ..
        } => {
            assert_eq!(value["notes"], "same");
            assert_eq!(provenance_frontier, &sorted_refs(&[&same_a, &same_b]));
        }
        _ => panic!("expected semantically equivalent resolved value"),
    }

    let note = record_update(13, 13, &[&base], &[("notes", json!("note"))], &["notes"]);
    let rating = record_update(14, 14, &[&base], &[("rating", json!(8))], &["rating"]);
    let platform = record_update(
        141,
        141,
        &[&base],
        &[("platform", json!("web"))],
        &["platform"],
    );
    let replay =
        replay_verified_history_v1(&[platform.clone(), rating.clone(), base.clone(), note.clone()])
            .unwrap();
    match materialized(&replay, &json!(["record", "r1"])) {
        MaterializedEntityV1::Resolved {
            business_value: Some(value),
            provenance_frontier,
            ..
        } => {
            assert_eq!(value["notes"], "note");
            assert_eq!(value["rating"], 8);
            assert_eq!(value["platform"], "web");
            assert_eq!(
                provenance_frontier,
                &sorted_refs(&[&note, &rating, &platform])
            );
        }
        _ => panic!("expected derived resolved value"),
    }

    let overlap = record_update(15, 15, &[&base], &[("notes", json!("other"))], &["notes"]);
    assert_eq!(
        conflict_kind(materialized(
            &replay_verified_history_v1(&[base.clone(), note.clone(), overlap]).unwrap(),
            &json!(["record", "r1"])
        )),
        Some("overlapping-field")
    );
    let next_base = make_commit(
        142,
        10,
        2,
        Some(&base),
        &[&base],
        vec![mutation(
            142,
            json!(["record", "r1"]),
            "upsert",
            full_record("r1", &[("platform", json!("next"))]),
            vec![reference(&base)],
            &["platform"],
        )],
        None,
    );
    let descendant = record_update(
        143,
        143,
        &[&next_base],
        &[("platform", json!("next")), ("rating", json!(7))],
        &["rating"],
    );
    assert_eq!(
        conflict_kind(materialized(
            &replay_verified_history_v1(&[base.clone(), next_base, descendant, note.clone()])
                .unwrap(),
            &json!(["record", "r1"])
        )),
        Some("different-base")
    );
    let deleted = make_commit(
        16,
        16,
        1,
        None,
        &[&base],
        vec![mutation(
            16,
            json!(["record", "r1"]),
            "tombstone",
            tombstone("r1"),
            vec![reference(&base)],
            &["$tombstone"],
        )],
        None,
    );
    assert_eq!(
        conflict_kind(materialized(
            &replay_verified_history_v1(&[base.clone(), note.clone(), deleted]).unwrap(),
            &json!(["record", "r1"])
        )),
        Some("live-tombstone")
    );
    let locked = record_update(
        17,
        17,
        &[&base],
        &[("isLocked", json!(true))],
        &["isLocked"],
    );
    assert_eq!(
        conflict_kind(materialized(
            &replay_verified_history_v1(&[base.clone(), note.clone(), locked]).unwrap(),
            &json!(["record", "r1"])
        )),
        Some("locked-concurrent")
    );
    let no_original = record_update(
        18,
        18,
        &[&base],
        &[("originalName", json!(""))],
        &["originalName"],
    );
    let no_chinese = record_update(
        19,
        19,
        &[&base],
        &[("chineseName", json!(""))],
        &["chineseName"],
    );
    assert_eq!(
        conflict_kind(materialized(
            &replay_verified_history_v1(&[base.clone(), no_original, no_chinese]).unwrap(),
            &json!(["record", "r1"])
        )),
        Some("derived-domain")
    );
    let nullable_base = record_create(181, 181, &[("isLocked", Value::Null)]);
    let newer_base = record_update(
        182,
        182,
        &[&nullable_base],
        &[("isLocked", Value::Null), ("platform", json!("P"))],
        &["platform"],
    );
    let locking = record_update(
        183,
        183,
        &[&newer_base],
        &[("platform", json!("P")), ("isLocked", json!(true))],
        &["isLocked"],
    );
    let parallel = record_update(
        184,
        184,
        &[&nullable_base],
        &[("isLocked", Value::Null), ("notes", json!("parallel"))],
        &["notes"],
    );
    assert_eq!(
        conflict_kind(materialized(
            &replay_verified_history_v1(&[nullable_base.clone(), newer_base, locking, parallel])
                .unwrap(),
            &json!(["record", "r1"])
        )),
        Some("locked-concurrent")
    );
    let true_branch = record_update(
        185,
        185,
        &[&nullable_base],
        &[("isLocked", json!(true))],
        &["isLocked"],
    );
    let false_branch = record_update(
        186,
        186,
        &[&nullable_base],
        &[("isLocked", json!(false))],
        &["isLocked"],
    );
    assert_eq!(
        conflict_kind(materialized(
            &replay_verified_history_v1(&[nullable_base, true_branch, false_branch]).unwrap(),
            &json!(["record", "r1"])
        )),
        Some("locked-concurrent")
    );
}

#[test]
fn locked_basis_allows_only_isolated_unlock_and_rejects_metadata_only_mutation() {
    let locked = record_create(20, 20, &[("isLocked", json!(true))]);
    let edit = make_commit(
        21,
        20,
        2,
        Some(&locked),
        &[&locked],
        vec![mutation(
            21,
            json!(["record", "r1"]),
            "upsert",
            full_record(
                "r1",
                &[("isLocked", json!(true)), ("notes", json!("forbidden"))],
            ),
            vec![reference(&locked)],
            &["notes"],
        )],
        None,
    );
    assert_eq!(
        status(
            &replay_verified_history_v1(&[locked.clone(), edit.clone()]).unwrap(),
            &edit
        )
        .error
        .as_deref(),
        Some("ordinary_mutation_blocked_by_lock")
    );
    let unlock = make_commit(
        22,
        20,
        2,
        Some(&locked),
        &[&locked],
        vec![mutation(
            22,
            json!(["record", "r1"]),
            "upsert",
            full_record("r1", &[]),
            vec![reference(&locked)],
            &["isLocked"],
        )],
        None,
    );
    assert_eq!(
        status(
            &replay_verified_history_v1(&[locked.clone(), unlock.clone()]).unwrap(),
            &unlock
        )
        .state,
        HistoricalValidityState::Valid
    );
    let mut metadata = make_commit(
        23,
        23,
        1,
        None,
        &[&locked],
        vec![mutation(
            23,
            json!(["record", "r1"]),
            "upsert",
            full_record("r1", &[("isLocked", json!(true))]),
            vec![reference(&locked)],
            &[],
        )],
        None,
    );
    metadata.mutations[0].value["updatedAt"] = json!("2026-09-06T14:00:00.000Z");
    assert_eq!(
        status(
            &replay_verified_history_v1(&[locked, metadata.clone()]).unwrap(),
            &metadata
        )
        .error
        .as_deref(),
        Some("metadata_only_mutation")
    );
}

#[test]
fn entity_conflict_gate_resolution_and_late_alternative_are_causal() {
    let base = record_create(30, 30, &[]);
    let a = record_update(31, 31, &[&base], &[("notes", json!("A"))], &["notes"]);
    let b = record_update(32, 32, &[&base], &[("notes", json!("B"))], &["notes"]);
    let conflicted = replay_verified_history_v1(&[base.clone(), a.clone(), b.clone()]).unwrap();
    let conflict_id = match materialized(&conflicted, &json!(["record", "r1"])) {
        MaterializedEntityV1::Conflict { conflict_id, .. } => conflict_id.clone(),
        _ => panic!("expected entity conflict"),
    };
    let ordinary = make_commit(
        33,
        33,
        1,
        None,
        &[&a, &b],
        vec![mutation(
            33,
            json!(["record", "r1"]),
            "upsert",
            full_record("r1", &[("notes", json!("resolved"))]),
            sorted_refs(&[&a, &b]),
            business_field_order("record").unwrap(),
        )],
        None,
    );
    assert_eq!(
        status(
            &replay_verified_history_v1(&[base.clone(), a.clone(), b.clone(), ordinary.clone()])
                .unwrap(),
            &ordinary
        )
        .error
        .as_deref(),
        Some("ordinary_mutation_blocked_by_entity_conflict")
    );
    let mut incomplete_base = ordinary;
    incomplete_base.commit_id = uuid(331);
    incomplete_base.content_hash = hash(331);
    incomplete_base.mutations[0].base_frontier = vec![a.commit_ref()];
    assert_eq!(
        status(
            &replay_verified_history_v1(&[
                base.clone(),
                a.clone(),
                b.clone(),
                incomplete_base.clone()
            ])
            .unwrap(),
            &incomplete_base
        )
        .error
        .as_deref(),
        Some("invalid_entity_base_frontier")
    );
    let resolution = make_commit(
        34,
        34,
        1,
        None,
        &[&a, &b],
        vec![mutation(
            34,
            json!(["record", "r1"]),
            "upsert",
            full_record("r1", &[("notes", json!("resolved"))]),
            sorted_refs(&[&a, &b]),
            business_field_order("record").unwrap(),
        )],
        Some(vec![conflict_id]),
    );
    let replay =
        replay_verified_history_v1(&[resolution.clone(), b.clone(), base.clone(), a.clone()])
            .unwrap();
    assert_eq!(
        status(&replay, &resolution).state,
        HistoricalValidityState::Valid
    );

    let stale = make_commit(
        35,
        35,
        1,
        None,
        &[&a, &b],
        vec![mutation(
            35,
            json!(["record", "r1"]),
            "upsert",
            full_record("r1", &[("notes", json!("stale"))]),
            sorted_refs(&[&a, &b]),
            business_field_order("record").unwrap(),
        )],
        Some(vec!["f".repeat(64)]),
    );
    assert_eq!(
        status(
            &replay_verified_history_v1(&[base.clone(), a.clone(), b.clone(), stale.clone()])
                .unwrap(),
            &stale
        )
        .error
        .as_deref(),
        Some("stale_resolution")
    );
    let late = record_update(
        36,
        29,
        &[&base],
        &[("platform", json!("late"))],
        &["platform"],
    );
    let replay =
        replay_verified_history_v1(&[base, a, b, resolution.clone(), late.clone()]).unwrap();
    assert_eq!(
        status(&replay, &resolution).state,
        HistoricalValidityState::Valid
    );
    assert!(matches!(
        materialized(&replay, &json!(["record", "r1"])),
        MaterializedEntityV1::Conflict { .. }
    ));
    assert_eq!(
        replay.frontiers[0].frontier,
        sorted_refs(&[&resolution, &late])
    );
}

#[test]
fn author_parent_predicates_relation_gate_and_atomic_resolution_are_enforced() {
    let record_value = full_record(
        "r-parent",
        &[
            ("totalEpisodes", json!(12)),
            ("episodeTrackingEnabled", json!(true)),
        ],
    );
    let collection_value = full_collection("c-parent", &[]);
    let episode_id = deterministic_id(
        "episode-completion:v1",
        &["r-parent".to_string(), "12".to_string()],
    );
    let parents = make_commit(
        40,
        40,
        1,
        None,
        &[],
        vec![
            mutation(
                401,
                json!(["record", "r-parent"]),
                "upsert",
                record_value.clone(),
                vec![],
                business_field_order("record").unwrap(),
            ),
            mutation(
                402,
                json!(["collection", "c-parent"]),
                "upsert",
                collection_value,
                vec![],
                business_field_order("collection").unwrap(),
            ),
            mutation(
                403,
                json!(["episode-completion", "r-parent", 12]),
                "upsert",
                json!({
                    "id": episode_id,
                    "recordId": "r-parent",
                    "episodeNumber": 12,
                    "completedAt": null,
                    "createdAt": "2026-09-06T10:00:00.000Z",
                    "updatedAt": "2026-09-06T10:00:00.000Z",
                    "rev": "0",
                    "revActor": ""
                }),
                vec![],
                business_field_order("episode-completion").unwrap(),
            ),
        ],
        None,
    );
    let shrink = make_commit(
        41,
        41,
        1,
        None,
        &[&parents],
        vec![mutation(
            41,
            json!(["record", "r-parent"]),
            "upsert",
            full_record(
                "r-parent",
                &[
                    ("totalEpisodes", json!(10)),
                    ("episodeTrackingEnabled", json!(true)),
                ],
            ),
            vec![reference(&parents)],
            &["totalEpisodes"],
        )],
        None,
    );
    let replay = replay_verified_history_v1(&[parents.clone(), shrink.clone()]).unwrap();
    assert_eq!(replay.relations.conflicts.len(), 1);
    assert_eq!(
        replay.relations.conflicts[0].core.relation_kind,
        "episode-exceeds-total"
    );
    let relation_id = replay.relations.conflicts[0].relation_conflict_id.clone();

    let partial = make_commit(
        411,
        44,
        1,
        None,
        &[&shrink],
        vec![mutation(
            411,
            json!(["record", "r-parent"]),
            "upsert",
            record_value.clone(),
            vec![reference(&shrink)],
            &["totalEpisodes"],
        )],
        Some(vec![relation_id.clone()]),
    );
    assert_eq!(
        status(
            &replay_verified_history_v1(&[parents.clone(), shrink.clone(), partial.clone()])
                .unwrap(),
            &partial
        )
        .error
        .as_deref(),
        Some("incomplete_relation_resolution")
    );

    let ordinary = make_commit(
        42,
        42,
        1,
        None,
        &[&shrink],
        vec![mutation(
            42,
            json!(["record", "r-parent"]),
            "upsert",
            record_value.clone(),
            vec![reference(&shrink)],
            &["totalEpisodes"],
        )],
        None,
    );
    assert_eq!(
        status(
            &replay_verified_history_v1(&[parents.clone(), shrink.clone(), ordinary.clone()])
                .unwrap(),
            &ordinary
        )
        .error
        .as_deref(),
        Some("ordinary_mutation_blocked_by_relation_conflict")
    );

    let resolution = make_commit(
        43,
        43,
        1,
        None,
        &[&shrink],
        vec![
            mutation(
                431,
                json!(["record", "r-parent"]),
                "upsert",
                record_value,
                vec![reference(&shrink)],
                &["totalEpisodes"],
            ),
            mutation(
                432,
                json!(["episode-completion", "r-parent", 12]),
                "upsert",
                json!({
                    "id": deterministic_id(
                        "episode-completion:v1",
                        &["r-parent".to_string(), "12".to_string()]
                    ),
                    "recordId": "r-parent",
                    "episodeNumber": 12,
                    "completedAt": null,
                    "createdAt": "2026-09-06T10:00:00.000Z",
                    "updatedAt": "2026-09-06T10:00:00.000Z",
                    "rev": "0",
                    "revActor": ""
                }),
                vec![reference(&parents)],
                &[],
            ),
        ],
        Some(vec![relation_id]),
    );
    let replay =
        replay_verified_history_v1(&[resolution.clone(), shrink.clone(), parents.clone()]).unwrap();
    assert_eq!(
        status(&replay, &resolution).state,
        HistoricalValidityState::Valid
    );
    assert!(replay.relations.conflicts.is_empty());

    let mut invalid_atomic = parents;
    invalid_atomic.commit_id = uuid(44);
    invalid_atomic.content_hash = hash(44);
    invalid_atomic.mutations[2].value["episodeNumber"] = json!(13);
    let replay = replay_verified_history_v1(std::slice::from_ref(&invalid_atomic)).unwrap();
    assert_eq!(
        status(&replay, &invalid_atomic).state,
        HistoricalValidityState::Invalid
    );
    assert!(replay.versions.is_empty());
}

#[test]
fn relation_resolution_must_name_every_known_conflict_it_eliminates() {
    let episode_value = |episode: i64| {
        json!({
            "id": deterministic_id("episode-completion:v1", &["multi".into(), episode.to_string()]),
            "recordId": "multi", "episodeNumber": episode, "completedAt": null,
            "createdAt": "2026-09-06T10:00:00.000Z", "updatedAt": "2026-09-06T10:00:00.000Z",
            "rev": "0", "revActor": ""
        })
    };
    let parent = make_commit(
        440,
        440,
        1,
        None,
        &[],
        vec![
            mutation(
                4401,
                json!(["record", "multi"]),
                "upsert",
                full_record("multi", &[("totalEpisodes", json!(7))]),
                vec![],
                business_field_order("record").unwrap(),
            ),
            mutation(
                4402,
                json!(["episode-completion", "multi", 5]),
                "upsert",
                episode_value(5),
                vec![],
                business_field_order("episode-completion").unwrap(),
            ),
            mutation(
                4403,
                json!(["episode-completion", "multi", 7]),
                "upsert",
                episode_value(7),
                vec![],
                business_field_order("episode-completion").unwrap(),
            ),
        ],
        None,
    );
    let shrink = make_commit(
        441,
        441,
        1,
        None,
        &[&parent],
        vec![mutation(
            441,
            json!(["record", "multi"]),
            "upsert",
            full_record("multi", &[("totalEpisodes", json!(3))]),
            vec![reference(&parent)],
            &["totalEpisodes"],
        )],
        None,
    );
    let replay = replay_verified_history_v1(&[parent.clone(), shrink.clone()]).unwrap();
    assert_eq!(replay.relations.conflicts.len(), 2);
    let id_for = |episode: i64| {
        replay
            .relations
            .conflicts
            .iter()
            .find(|conflict| conflict.core.semantic_relation_facts["episodeNumber"] == episode)
            .unwrap()
            .relation_conflict_id
            .clone()
    };
    let id_five = id_for(5);
    let id_seven = id_for(7);
    let one = make_commit(
        442,
        442,
        1,
        None,
        &[&shrink],
        vec![
            mutation(
                4421,
                json!(["record", "multi"]),
                "upsert",
                full_record("multi", &[("totalEpisodes", json!(7))]),
                vec![reference(&shrink)],
                &["totalEpisodes"],
            ),
            mutation(
                4422,
                json!(["episode-completion", "multi", 7]),
                "upsert",
                episode_value(7),
                vec![reference(&parent)],
                &[],
            ),
        ],
        Some(vec![id_seven.clone()]),
    );
    assert_eq!(
        status(
            &replay_verified_history_v1(&[parent.clone(), shrink.clone(), one.clone()]).unwrap(),
            &one
        )
        .error
        .as_deref(),
        Some("incomplete_relation_resolution")
    );
    let both = make_commit(
        443,
        443,
        1,
        None,
        &[&shrink],
        vec![
            mutation(
                4431,
                json!(["record", "multi"]),
                "upsert",
                full_record("multi", &[("totalEpisodes", json!(7))]),
                vec![reference(&shrink)],
                &["totalEpisodes"],
            ),
            mutation(
                4432,
                json!(["episode-completion", "multi", 5]),
                "upsert",
                episode_value(5),
                vec![reference(&parent)],
                &[],
            ),
            mutation(
                4433,
                json!(["episode-completion", "multi", 7]),
                "upsert",
                episode_value(7),
                vec![reference(&parent)],
                &[],
            ),
        ],
        Some({
            let mut ids = vec![id_five, id_seven];
            ids.sort();
            ids
        }),
    );
    let replay = replay_verified_history_v1(&[both.clone(), shrink, parent]).unwrap();
    assert_eq!(status(&replay, &both).state, HistoricalValidityState::Valid);
    assert!(replay.relations.conflicts.is_empty());
}

fn permute(values: &[CommitV1], seed: u32) -> Vec<CommitV1> {
    let mut result = values.to_vec();
    let mut state = seed;
    for index in (1..result.len()).rev() {
        state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        let other = state as usize % (index + 1);
        result.swap(index, other);
    }
    result
}

#[test]
fn causal_dag_frontier_and_replay_are_permutation_invariant() {
    let base = record_create(50, 50, &[]);
    let a = record_update(51, 51, &[&base], &[("notes", json!("A"))], &["notes"]);
    let b = record_update(52, 52, &[&base], &[("rating", json!(9))], &["rating"]);
    let observed = make_commit(
        53,
        51,
        2,
        Some(&a),
        &[&a, &b],
        vec![mutation(
            53,
            json!(["record", "r1"]),
            "upsert",
            full_record(
                "r1",
                &[
                    ("notes", json!("A")),
                    ("rating", json!(9)),
                    ("platform", json!("P")),
                ],
            ),
            sorted_refs(&[&a, &b]),
            &["platform"],
        )],
        None,
    );
    let commits = vec![base, a, b, observed.clone()];
    let baseline = jcs_bytes(&replay_verified_history_v1(&commits).unwrap()).unwrap();
    for seed in 0..64 {
        assert_eq!(
            jcs_bytes(&replay_verified_history_v1(&permute(&commits, seed)).unwrap()).unwrap(),
            baseline
        );
    }
    let replay = replay_verified_history_v1(&commits).unwrap();
    let verified = commits
        .iter()
        .map(|commit| {
            (
                format!(
                    "{}\0{}\0{}\0{}",
                    commit.writer_id, commit.writer_seq, commit.commit_id, commit.content_hash
                ),
                commit.clone(),
            )
        })
        .collect::<HashMap<_, _>>();
    let frontier = compute_entity_frontier_v1(&replay.versions, &verified);
    assert_eq!(frontier.len(), 1);
    assert_eq!(frontier[0].commit_ref, observed.commit_ref());

    assert_eq!(
        jcs_bytes(&replay_verified_history_v1(&[commits[0].clone(), commits[0].clone()]).unwrap())
            .unwrap(),
        jcs_bytes(&replay_verified_history_v1(std::slice::from_ref(&commits[0])).unwrap()).unwrap()
    );
    let mut collision = commits[0].clone();
    collision.mutations[0].value["updatedAt"] = json!("2026-09-06T11:00:00.000Z");
    for ordered in [
        vec![commits[0].clone(), collision.clone()],
        vec![commits[0].clone(), collision.clone(), commits[0].clone()],
        vec![collision.clone(), commits[0].clone(), collision.clone()],
    ] {
        let collided = replay_verified_history_v1(&ordered).unwrap();
        assert_eq!(collided.validity.len(), 1);
        assert_eq!(
            collided.validity[0].validity.error.as_deref(),
            Some("duplicate_commit_ref")
        );
        assert!(collided.versions.is_empty());
    }

    let mut duplicate = commits[0].clone();
    duplicate.mutations.push(duplicate.mutations[0].clone());
    assert_eq!(
        validate_commit_envelope_v1(&duplicate).unwrap_err().0,
        "duplicate_local_mutation_id"
    );
}

#[test]
fn generated_causal_dag_matches_reference_transitive_closure() {
    let mut commits = Vec::<CommitV1>::new();
    let mut ancestors = Vec::<BTreeSet<usize>>::new();
    let mut state = 0x51a7_c0de_u32;
    for index in 0..20_usize {
        let mut parent_indexes = BTreeSet::new();
        if index > 0 {
            parent_indexes.insert(index - 1);
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            parent_indexes.insert(state as usize % index);
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            parent_indexes.insert(state as usize % index);
        }
        let mut closure = parent_indexes.clone();
        for parent in &parent_indexes {
            closure.extend(ancestors[*parent].iter().copied());
        }
        ancestors.push(closure);
        let id = format!("dag-{index:02}");
        let name = format!("D{index}");
        let normalized = format!("d{index}");
        let parents = parent_indexes
            .iter()
            .map(|parent| &commits[*parent])
            .collect::<Vec<_>>();
        commits.push(make_commit(
            100 + index as u32,
            100 + index as u32,
            1,
            None,
            &parents,
            vec![mutation(
                100 + index as u32,
                json!(["collection", id]),
                "upsert",
                full_collection(
                    &id,
                    &[("name", json!(name)), ("normalizedName", json!(normalized))],
                ),
                vec![],
                business_field_order("collection").unwrap(),
            )],
            None,
        ));
    }
    let replay = replay_verified_history_v1(&permute(&commits, 99)).unwrap();
    assert_eq!(
        replay
            .validity
            .iter()
            .filter(|entry| entry.validity.state == HistoricalValidityState::Valid)
            .count(),
        commits.len()
    );
    let verified = commits
        .iter()
        .map(|commit| {
            (
                format!(
                    "{}\0{}\0{}\0{}",
                    commit.writer_id, commit.writer_seq, commit.commit_id, commit.content_hash
                ),
                commit.clone(),
            )
        })
        .collect::<HashMap<_, _>>();
    for covering in 0..commits.len() {
        for covered in 0..commits.len() {
            assert_eq!(
                causally_covers_v1(
                    &commits[covering].commit_ref(),
                    &commits[covered].commit_ref(),
                    &verified
                ),
                covering == covered || ancestors[covering].contains(&covered),
                "{covering} covers {covered}"
            );
        }
    }
    let baseline = jcs_bytes(&replay_verified_history_v1(&commits).unwrap()).unwrap();
    for seed in 0..8 {
        assert_eq!(
            jcs_bytes(&replay_verified_history_v1(&permute(&commits, seed)).unwrap()).unwrap(),
            baseline
        );
    }
}
