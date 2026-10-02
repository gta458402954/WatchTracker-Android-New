use std::fs;

use serde_json::Value;

use super::activation_cutover::{
    begin_activation_cutover_recovery_v1, create_activation_cutover_state_v1, decide_legacy_put_v1,
    recover_activation_cutover_v1, ActivationCutoverStateV1, LegacyPutDecisionV1,
};
use super::canonical::sha256_hex;
use super::remote_discovery::{
    create_discovery_state_v1, observe_candidate_listing_v1, parse_activation_candidate_path_v1,
    verify_activation_candidate_v1, ActivationVerificationV1, DiscoveryStateV1,
    VerifiedFingerprintEvidenceV1, VerifiedRemoteObjectV1,
};

fn fixture() -> Value {
    serde_json::from_str(
        &fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../contracts/s2-lite/v1/activation-cutover-golden-v1.json"
        ))
        .unwrap(),
    )
    .unwrap()
}

fn discovery_with_evidence(evidence: &[Value]) -> DiscoveryStateV1 {
    let mut discovery = create_discovery_state_v1();
    discovery.verified_objects = evidence
        .iter()
        .map(|value| VerifiedRemoteObjectV1 {
            path: value["path"].as_str().unwrap().to_string(),
            kind: "activation".to_string(),
            exact_bytes_hash: value["exactBytesHash"].as_str().unwrap().to_string(),
            exact_bytes_hex: String::new(),
            content_hash: value["contentHash"].as_str().unwrap().to_string(),
            commit_ref: None,
            activation_id: Some(value["activationId"].as_str().unwrap().to_string()),
            fingerprint_evidence: value["legacyFingerprint"].as_str().map_or(
                VerifiedFingerprintEvidenceV1::Null,
                |fingerprint| VerifiedFingerprintEvidenceV1::Value {
                    value: fingerprint.to_string(),
                },
            ),
        })
        .collect();
    discovery
}

fn discovery_with_tagged_evidence(fixture: &Value, entries: &[Value]) -> DiscoveryStateV1 {
    let mut discovery = create_discovery_state_v1();
    discovery.verified_objects = entries
        .iter()
        .map(|entry| {
            let value = &fixture["evidence"][entry["key"].as_str().unwrap()];
            VerifiedRemoteObjectV1 {
                path: value["path"].as_str().unwrap().to_string(),
                kind: "activation".to_string(),
                exact_bytes_hash: value["exactBytesHash"].as_str().unwrap().to_string(),
                exact_bytes_hex: String::new(),
                content_hash: value["contentHash"].as_str().unwrap().to_string(),
                commit_ref: None,
                activation_id: Some(value["activationId"].as_str().unwrap().to_string()),
                fingerprint_evidence: serde_json::from_value(entry["fingerprintEvidence"].clone())
                    .unwrap(),
            }
        })
        .collect();
    discovery
}

fn expected_state(fixture: &Value, name: &str) -> ActivationCutoverStateV1 {
    let scenario = fixture["scenarios"]
        .as_array()
        .unwrap()
        .iter()
        .find(|scenario| scenario["name"] == name)
        .unwrap();
    serde_json::from_value(scenario["expected"].clone()).unwrap()
}

fn decision_value(decision: &LegacyPutDecisionV1) -> Value {
    match decision {
        LegacyPutDecisionV1::AllowedS2NotActivated => {
            serde_json::json!({ "allowed": true, "reason": "S2_NOT_ACTIVATED" })
        }
        LegacyPutDecisionV1::DeniedRemoteS2Activated => {
            serde_json::json!({ "allowed": false, "reason": "REMOTE_S2_ACTIVATED" })
        }
        LegacyPutDecisionV1::DeniedCutoverRecoveryNotReady => {
            serde_json::json!({ "allowed": false, "reason": "CUTOVER_RECOVERY_NOT_READY" })
        }
    }
}

#[test]
fn shared_phase_three_c_fixture_matches_every_transition() {
    let fixture = fixture();
    assert_eq!(
        fixture["schema"],
        "watchtracker-s2-lite-activation-cutover-golden-v1"
    );
    assert_eq!(fixture["scenarios"].as_array().unwrap().len(), 9);
    for scenario in fixture["scenarios"].as_array().unwrap() {
        let prior: ActivationCutoverStateV1 =
            serde_json::from_value(scenario["prior"].clone()).unwrap();
        let evidence = scenario["verifiedActivations"].as_array().unwrap();
        let discovery = discovery_with_evidence(evidence);
        let discovery: DiscoveryStateV1 =
            serde_json::from_slice(&serde_json::to_vec(&discovery).unwrap()).unwrap();
        let recovery = recover_activation_cutover_v1(&discovery, Some(&prior));
        let actual = recovery
            .diagnostic_state()
            .unwrap_or_else(|| panic!("{} was not ready", scenario["name"]));
        assert_eq!(
            serde_json::to_value(actual).unwrap(),
            scenario["expected"],
            "{}",
            scenario["name"]
        );
        let allowed = decide_legacy_put_v1(&recovery) == LegacyPutDecisionV1::AllowedS2NotActivated;
        assert_eq!(
            allowed,
            scenario["legacyPutAllowed"].as_bool().unwrap(),
            "{}",
            scenario["name"]
        );
    }
}

#[test]
fn activation_order_never_selects_a_winner() {
    let fixture = fixture();
    for name in [
        "multiple-same-fingerprint",
        "conflicting-fingerprints",
        "null-fingerprint-equality",
        "null-vs-non-null-conflict",
    ] {
        let scenario = fixture["scenarios"]
            .as_array()
            .unwrap()
            .iter()
            .find(|scenario| scenario["name"] == name)
            .unwrap();
        let evidence = scenario["verifiedActivations"].as_array().unwrap();
        let mut reverse = evidence.clone();
        reverse.reverse();
        let forward = recover_activation_cutover_v1(
            &discovery_with_evidence(evidence),
            Some(&create_activation_cutover_state_v1()),
        );
        let reversed = recover_activation_cutover_v1(
            &discovery_with_evidence(&reverse),
            Some(&create_activation_cutover_state_v1()),
        );
        assert_eq!(forward, reversed, "{name}");
        let state = forward
            .diagnostic_state()
            .unwrap_or_else(|| panic!("{name} was not ready"));
        assert_eq!(serde_json::to_value(state).unwrap(), scenario["expected"]);
    }
}

#[test]
fn activated_and_frozen_state_survives_serialized_restart_and_omission() {
    let fixture = fixture();
    for name in [
        "restart-after-activation",
        "conflict-remains-after-omission",
    ] {
        let scenario = fixture["scenarios"]
            .as_array()
            .unwrap()
            .iter()
            .find(|scenario| scenario["name"] == name)
            .unwrap();
        let prior: ActivationCutoverStateV1 =
            serde_json::from_value(scenario["prior"].clone()).unwrap();
        let restarted: ActivationCutoverStateV1 =
            serde_json::from_slice(&serde_json::to_vec(&prior).unwrap()).unwrap();
        let recovery =
            recover_activation_cutover_v1(&create_discovery_state_v1(), Some(&restarted));
        let actual = recovery
            .diagnostic_state()
            .unwrap_or_else(|| panic!("{name} was not ready"));
        assert_eq!(actual, prior, "{name}");
        assert_eq!(
            decide_legacy_put_v1(&recovery),
            LegacyPutDecisionV1::DeniedRemoteS2Activated
        );
    }
}

#[test]
fn recovery_is_fail_closed_and_reconciles_empty_or_false_cutover_state() {
    assert_eq!(
        decide_legacy_put_v1(&begin_activation_cutover_recovery_v1()),
        LegacyPutDecisionV1::DeniedCutoverRecoveryNotReady
    );
    let fixture = fixture();
    let discovery = discovery_with_evidence(&[fixture["evidence"]["a"].clone()]);
    for persisted in [None, Some(create_activation_cutover_state_v1())] {
        let recovery = recover_activation_cutover_v1(&discovery, persisted.as_ref());
        let state = recovery
            .diagnostic_state()
            .expect("durable discovery evidence did not recover");
        assert!(state.remote_s2_activated);
        assert_eq!(
            decide_legacy_put_v1(&recovery),
            LegacyPutDecisionV1::DeniedRemoteS2Activated
        );
    }
}

#[test]
fn verified_null_fingerprint_survives_discovery_serialization_before_recovery() {
    let fixture = fixture();
    let activation_id = fixture["evidence"]["n1"]["activationId"].as_str().unwrap();
    let bytes = b"verified-null-activation";
    let path = format!("activations/{activation_id}--{}.json", sha256_hex(bytes));
    let candidate = parse_activation_candidate_path_v1(&path).unwrap();
    let mut discovery = create_discovery_state_v1();
    observe_candidate_listing_v1(&mut discovery, std::slice::from_ref(&path));
    verify_activation_candidate_v1(
        &mut discovery,
        &candidate,
        bytes,
        &ActivationVerificationV1 {
            activation_id: activation_id.to_string(),
            legacy_fingerprint: None,
            semantic_profile_supported: true,
            required_features_supported: true,
        },
    )
    .unwrap();
    let restarted: DiscoveryStateV1 =
        serde_json::from_slice(&serde_json::to_vec(&discovery).unwrap()).unwrap();
    assert_eq!(
        restarted.verified_objects[0].fingerprint_evidence,
        VerifiedFingerprintEvidenceV1::Null
    );
    let recovery = recover_activation_cutover_v1(&restarted, None);
    let state = recovery
        .diagnostic_state()
        .expect("verified null did not recover");
    assert_eq!(
        serde_json::to_value(state.fingerprint_consistency).unwrap(),
        serde_json::json!({ "state": "Consistent", "legacyFingerprint": null })
    );
}

#[test]
fn shared_recovery_fixture_covers_controlled_readiness_and_fingerprint_roundtrips() {
    let fixture = fixture();
    let recovery_scenarios = fixture["recoveryScenarios"].as_array().unwrap();
    assert_eq!(recovery_scenarios.len(), 11);
    for scenario in recovery_scenarios {
        if scenario["operation"] == "notReady" {
            let recovery = begin_activation_cutover_recovery_v1();
            assert_eq!(recovery.not_ready_reason(), Some("RECOVERY_NOT_STARTED"));
            assert_eq!(
                decision_value(&decide_legacy_put_v1(&recovery)),
                scenario["expectedDecision"],
                "{}",
                scenario["name"]
            );
            continue;
        }

        let discovery =
            discovery_with_tagged_evidence(&fixture, scenario["evidence"].as_array().unwrap());
        let discovery = if scenario["operation"] == "roundtripRecover" {
            let restarted: DiscoveryStateV1 =
                serde_json::from_slice(&serde_json::to_vec(&discovery).unwrap()).unwrap();
            assert_eq!(
                serde_json::to_value(&restarted.verified_objects[0].fingerprint_evidence).unwrap(),
                scenario["expectedFingerprintEvidence"],
                "{}",
                scenario["name"]
            );
            restarted
        } else {
            discovery
        };
        let persisted = scenario["persistedScenario"]
            .as_str()
            .map(|name| expected_state(&fixture, name));
        let recovery = recover_activation_cutover_v1(&discovery, persisted.as_ref());
        assert_eq!(
            decision_value(&decide_legacy_put_v1(&recovery)),
            scenario["expectedDecision"],
            "{}",
            scenario["name"]
        );
        if let Some(expected_readiness) = scenario["expectedReadiness"].as_str() {
            let actual = if recovery.diagnostic_state().is_some() {
                "Ready"
            } else {
                "NotReady"
            };
            assert_eq!(actual, expected_readiness, "{}", scenario["name"]);
        }
        let state = recovery.diagnostic_state();
        if let Some(expected_scenario) = scenario["expectedScenario"].as_str() {
            assert_eq!(
                state,
                Some(expected_state(&fixture, expected_scenario)),
                "{}",
                scenario["name"]
            );
        }
        if let Some(expected_activated) = scenario["expectedActivated"].as_bool() {
            let state = state.unwrap();
            assert_eq!(
                state.remote_s2_activated, expected_activated,
                "{}",
                scenario["name"]
            );
            assert_eq!(
                serde_json::to_value(state.fingerprint_consistency).unwrap(),
                scenario["expectedConsistency"],
                "{}",
                scenario["name"]
            );
        }
    }
}

#[test]
fn ready_diagnostic_copy_cannot_change_the_private_decision_state() {
    let fixture = fixture();
    let discovery = discovery_with_evidence(&[fixture["evidence"]["a"].clone()]);
    let recovery = recover_activation_cutover_v1(&discovery, None);
    let mut diagnostic = recovery.diagnostic_state().unwrap();
    diagnostic.remote_s2_activated = false;
    diagnostic.verified_activation_evidence.clear();
    assert_eq!(
        decide_legacy_put_v1(&recovery),
        LegacyPutDecisionV1::DeniedRemoteS2Activated
    );
    assert!(recovery.diagnostic_state().unwrap().remote_s2_activated);
}
