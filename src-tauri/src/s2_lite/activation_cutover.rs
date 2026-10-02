use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use super::canonical::{ProtocolError, Result};
use super::remote_discovery::{DiscoveryStateV1, VerifiedFingerprintEvidenceV1};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VerifiedActivationEvidenceV1 {
    pub path: String,
    pub activation_id: String,
    pub content_hash: String,
    pub exact_bytes_hash: String,
    pub legacy_fingerprint: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "PascalCase", tag = "state")]
pub enum ActivationFingerprintConsistencyV1 {
    NoEvidence,
    Consistent {
        #[serde(rename = "legacyFingerprint")]
        legacy_fingerprint: Option<String>,
    },
    Conflict,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivationCutoverFatalV1 {
    pub code: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivationCutoverStateV1 {
    pub state_version: u8,
    pub remote_s2_activated: bool,
    pub verified_activation_evidence: Vec<VerifiedActivationEvidenceV1>,
    pub fingerprint_consistency: ActivationFingerprintConsistencyV1,
    pub root_fatal_signals: Vec<ActivationCutoverFatalV1>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LegacyPutDecisionV1 {
    AllowedS2NotActivated,
    DeniedRemoteS2Activated,
    DeniedCutoverRecoveryNotReady,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum ActivationCutoverRecoveryInnerV1 {
    NotReady { reason: &'static str },
    Ready { state: ActivationCutoverStateV1 },
}

/// Opaque recovery capability. Its ready state can only be created by reconciliation.
///
/// ```compile_fail
/// use app_lib::s2_lite::activation_cutover::ActivationCutoverRecoveryV1;
/// let _forged = ActivationCutoverRecoveryV1 { inner: panic!("not executed") };
/// ```
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActivationCutoverRecoveryV1 {
    inner: ActivationCutoverRecoveryInnerV1,
}

impl ActivationCutoverRecoveryV1 {
    pub fn diagnostic_state(&self) -> Option<ActivationCutoverStateV1> {
        match &self.inner {
            ActivationCutoverRecoveryInnerV1::Ready { state } => Some(state.clone()),
            ActivationCutoverRecoveryInnerV1::NotReady { .. } => None,
        }
    }

    pub fn not_ready_reason(&self) -> Option<&'static str> {
        match self.inner {
            ActivationCutoverRecoveryInnerV1::NotReady { reason } => Some(reason),
            ActivationCutoverRecoveryInnerV1::Ready { .. } => None,
        }
    }
}

pub fn create_activation_cutover_state_v1() -> ActivationCutoverStateV1 {
    ActivationCutoverStateV1 {
        state_version: 1,
        remote_s2_activated: false,
        verified_activation_evidence: vec![],
        fingerprint_consistency: ActivationFingerprintConsistencyV1::NoEvidence,
        root_fatal_signals: vec![],
    }
}

fn fingerprint_key(value: &Option<String>) -> String {
    value
        .as_ref()
        .map_or_else(|| "0:null".to_string(), |value| format!("1:{value}"))
}

fn compute_consistency(
    evidence: &[VerifiedActivationEvidenceV1],
) -> ActivationFingerprintConsistencyV1 {
    if evidence.is_empty() {
        return ActivationFingerprintConsistencyV1::NoEvidence;
    }
    let fingerprints = evidence
        .iter()
        .map(|value| fingerprint_key(&value.legacy_fingerprint))
        .collect::<BTreeSet<_>>();
    if fingerprints.len() > 1 {
        ActivationFingerprintConsistencyV1::Conflict
    } else {
        ActivationFingerprintConsistencyV1::Consistent {
            legacy_fingerprint: evidence[0].legacy_fingerprint.clone(),
        }
    }
}

pub fn evaluate_activation_cutover_v1(
    prior: &ActivationCutoverStateV1,
    discovery: &DiscoveryStateV1,
) -> Result<ActivationCutoverStateV1> {
    let mut state = prior.clone();
    for object in discovery
        .verified_objects
        .iter()
        .filter(|object| object.kind == "activation")
    {
        let activation_id = object
            .activation_id
            .as_ref()
            .ok_or(ProtocolError("verified_activation_evidence_incomplete"))?;
        let legacy_fingerprint = match &object.fingerprint_evidence {
            VerifiedFingerprintEvidenceV1::Missing => {
                return Err(ProtocolError("verified_activation_evidence_incomplete"));
            }
            VerifiedFingerprintEvidenceV1::Null => None,
            VerifiedFingerprintEvidenceV1::Value { value } => Some(value.clone()),
        };
        if !state
            .verified_activation_evidence
            .iter()
            .any(|value| value.path == object.path)
        {
            state
                .verified_activation_evidence
                .push(VerifiedActivationEvidenceV1 {
                    path: object.path.clone(),
                    activation_id: activation_id.clone(),
                    content_hash: object.content_hash.clone(),
                    exact_bytes_hash: object.exact_bytes_hash.clone(),
                    legacy_fingerprint,
                });
        }
    }
    state
        .verified_activation_evidence
        .sort_by(|left, right| left.path.cmp(&right.path));
    state.remote_s2_activated |= !state.verified_activation_evidence.is_empty();
    state.fingerprint_consistency = compute_consistency(&state.verified_activation_evidence);
    if state.fingerprint_consistency == ActivationFingerprintConsistencyV1::Conflict
        && !state
            .root_fatal_signals
            .iter()
            .any(|value| value.code == "SYNC_ROOT_FROZEN_LEGACY_CHANGE")
    {
        state.root_fatal_signals.push(ActivationCutoverFatalV1 {
            code: "SYNC_ROOT_FROZEN_LEGACY_CHANGE".to_string(),
        });
    }
    Ok(state)
}

pub fn begin_activation_cutover_recovery_v1() -> ActivationCutoverRecoveryV1 {
    ActivationCutoverRecoveryV1 {
        inner: ActivationCutoverRecoveryInnerV1::NotReady {
            reason: "RECOVERY_NOT_STARTED",
        },
    }
}

pub fn recover_activation_cutover_v1(
    discovery: &DiscoveryStateV1,
    persisted: Option<&ActivationCutoverStateV1>,
) -> ActivationCutoverRecoveryV1 {
    let prior = persisted
        .cloned()
        .unwrap_or_else(create_activation_cutover_state_v1);
    let Ok(state) = evaluate_activation_cutover_v1(&prior, discovery) else {
        return ActivationCutoverRecoveryV1 {
            inner: ActivationCutoverRecoveryInnerV1::NotReady {
                reason: "PERSISTED_STATE_INVALID",
            },
        };
    };
    let has_evidence = !state.verified_activation_evidence.is_empty();
    let has_conflict_fatal = state
        .root_fatal_signals
        .iter()
        .any(|value| value.code == "SYNC_ROOT_FROZEN_LEGACY_CHANGE");
    if state.state_version != 1
        || (state.remote_s2_activated && !has_evidence)
        || (has_conflict_fatal
            && state.fingerprint_consistency != ActivationFingerprintConsistencyV1::Conflict)
    {
        return ActivationCutoverRecoveryV1 {
            inner: ActivationCutoverRecoveryInnerV1::NotReady {
                reason: "PERSISTED_STATE_INVALID",
            },
        };
    }
    ActivationCutoverRecoveryV1 {
        inner: ActivationCutoverRecoveryInnerV1::Ready { state },
    }
}

pub fn decide_legacy_put_v1(recovery: &ActivationCutoverRecoveryV1) -> LegacyPutDecisionV1 {
    match &recovery.inner {
        ActivationCutoverRecoveryInnerV1::NotReady { .. } => {
            LegacyPutDecisionV1::DeniedCutoverRecoveryNotReady
        }
        ActivationCutoverRecoveryInnerV1::Ready { state } if state.remote_s2_activated => {
            LegacyPutDecisionV1::DeniedRemoteS2Activated
        }
        ActivationCutoverRecoveryInnerV1::Ready { .. } => {
            LegacyPutDecisionV1::AllowedS2NotActivated
        }
    }
}
