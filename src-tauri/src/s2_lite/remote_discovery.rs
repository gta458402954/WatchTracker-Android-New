use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::canonical::{
    compare_commit_ref_v1, parse_writer_seq, sha256_hex, validate_canonical_uuid,
    validate_canonical_uuid_v4, validate_commit_ref, ProtocolError, Result,
};
use super::causal::decode_frozen_wire_commit_v1;
use super::immutable_publish::{build_commit_remote_path_v1, SEGMENT_NAME_WIDTH_V1};
use super::types::{CommitRef, CommitV1};

const MAX_RETAINED_SEGMENTS_V1: u64 = 4096;
const MAX_TRACKED_GAP_SEQUENCE_V1: u64 = 65_536;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum ObservedCandidateV1 {
    #[serde(rename = "activation")]
    Activation {
        path: String,
        activation_id: String,
        content_hash: String,
    },
    #[serde(rename = "commit")]
    Commit {
        path: String,
        writer_id: String,
        segment_name: String,
        writer_seq: String,
        commit_id: String,
        content_hash: String,
    },
}

impl ObservedCandidateV1 {
    pub fn path(&self) -> &str {
        match self {
            Self::Activation { path, .. } | Self::Commit { path, .. } => path,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VerifiedRemoteObjectV1 {
    pub path: String,
    pub kind: String,
    pub exact_bytes_hash: String,
    pub exact_bytes_hex: String,
    pub content_hash: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub commit_ref: Option<CommitRef>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub activation_id: Option<String>,
    pub fingerprint_evidence: VerifiedFingerprintEvidenceV1,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "PascalCase", tag = "state")]
pub enum VerifiedFingerprintEvidenceV1 {
    Missing,
    Null,
    Value { value: String },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActivationVerificationV1 {
    pub activation_id: String,
    pub legacy_fingerprint: Option<String>,
    pub semantic_profile_supported: bool,
    pub required_features_supported: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ActivationValidatorErrorV1 {
    ProtocolValidation,
    Internal(ProtocolError),
}

pub type ActivationValidatorResultV1 =
    std::result::Result<ActivationVerificationV1, ActivationValidatorErrorV1>;

/// Strict production validation for an exact activation object fetched during
/// discovery.  This deliberately returns only the frozen semantic facts used
/// by cutover evaluation; path/hash identity is established separately by the
/// discovery verifier before this validator is called.
pub fn validate_production_activation_body_v1(bytes: &[u8]) -> ActivationValidatorResultV1 {
    let value: Value = serde_json::from_slice(bytes)
        .map_err(|_| ActivationValidatorErrorV1::ProtocolValidation)?;
    let object = value
        .as_object()
        .ok_or(ActivationValidatorErrorV1::ProtocolValidation)?;
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
        || object["requiredFeatures"] != serde_json::json!([])
    {
        return Err(ActivationValidatorErrorV1::ProtocolValidation);
    }
    let activation_id = object["activationId"]
        .as_str()
        .ok_or(ActivationValidatorErrorV1::ProtocolValidation)?;
    if validate_canonical_uuid_v4(activation_id).is_err() {
        return Err(ActivationValidatorErrorV1::ProtocolValidation);
    }
    let legacy_fingerprint = if object["legacyFingerprint"].is_null() {
        None
    } else {
        let fingerprint = object["legacyFingerprint"]
            .as_str()
            .ok_or(ActivationValidatorErrorV1::ProtocolValidation)?;
        if fingerprint.len() != 64
            || !fingerprint
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        {
            return Err(ActivationValidatorErrorV1::ProtocolValidation);
        }
        Some(fingerprint.to_string())
    };
    Ok(ActivationVerificationV1 {
        activation_id: activation_id.to_string(),
        legacy_fingerprint,
        semantic_profile_supported: true,
        required_features_supported: true,
    })
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RootFatalSignalV1 {
    pub code: String,
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub writer_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub writer_seq: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub safe_writer_frontier: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoricalAuditCursorV1 {
    pub last_writer_id: Option<String>,
    pub last_segment_by_writer: BTreeMap<String, Option<String>>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ExactWorkClassV1 {
    Dependency,
    Candidate,
    Reverify,
}

impl ExactWorkClassV1 {
    fn all() -> [Self; 3] {
        [Self::Dependency, Self::Candidate, Self::Reverify]
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExactWorkSchedulerV1 {
    pub next_class: ExactWorkClassV1,
    pub after_by_class: BTreeMap<ExactWorkClassV1, Option<String>>,
}

impl Default for ExactWorkSchedulerV1 {
    fn default() -> Self {
        Self {
            next_class: ExactWorkClassV1::Dependency,
            after_by_class: ExactWorkClassV1::all()
                .into_iter()
                .map(|class| (class, None))
                .collect(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscoveryStateV1 {
    pub state_version: u8,
    pub observed_activations: Vec<String>,
    pub observed_writers: Vec<String>,
    pub observed_segments: Vec<String>,
    pub observed_candidates: Vec<ObservedCandidateV1>,
    pub verified_objects: Vec<VerifiedRemoteObjectV1>,
    pub known_gaps: Vec<String>,
    pub targeted_queue: Vec<CommitRef>,
    pub historical_closed_segments: Vec<String>,
    pub historical_audit_cursor: HistoricalAuditCursorV1,
    pub gap_segment_cursor_by_writer: BTreeMap<String, Option<String>>,
    #[serde(default)]
    pub reverification_queue: Vec<String>,
    #[serde(default)]
    pub terminal_candidate_paths: Vec<String>,
    #[serde(default)]
    pub exact_work_scheduler: ExactWorkSchedulerV1,
    #[serde(default)]
    pub last_round_scheduled_lists: Vec<String>,
    #[serde(default)]
    pub last_round_scheduled_gets: Vec<String>,
    pub root_fatal_signals: Vec<RootFatalSignalV1>,
    pub last_round_indeterminate: bool,
}

pub fn create_discovery_state_v1() -> DiscoveryStateV1 {
    DiscoveryStateV1 {
        state_version: 1,
        observed_activations: vec![],
        observed_writers: vec![],
        observed_segments: vec![],
        observed_candidates: vec![],
        verified_objects: vec![],
        known_gaps: vec![],
        targeted_queue: vec![],
        historical_closed_segments: vec![],
        historical_audit_cursor: HistoricalAuditCursorV1::default(),
        gap_segment_cursor_by_writer: BTreeMap::new(),
        reverification_queue: vec![],
        terminal_candidate_paths: vec![],
        exact_work_scheduler: ExactWorkSchedulerV1::default(),
        last_round_scheduled_lists: vec![],
        last_round_scheduled_gets: vec![],
        root_fatal_signals: vec![],
        last_round_indeterminate: false,
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DiscoveryBudgetsV1 {
    pub max_exact_fetches_per_sync: usize,
    pub max_segments_per_writer_per_sync: usize,
    pub max_dependency_targets_per_sync: usize,
    pub max_listing_entries_per_directory: usize,
}

impl Default for DiscoveryBudgetsV1 {
    fn default() -> Self {
        Self {
            max_exact_fetches_per_sync: 64,
            max_segments_per_writer_per_sync: 5,
            max_dependency_targets_per_sync: 64,
            max_listing_entries_per_directory: 4096,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DirectoryListResultV1 {
    Entries(Vec<String>),
    Indeterminate,
    AuthOrCapabilityFailure,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DiscoveryExactGetResultV1 {
    DefinitelyPresent(Vec<u8>),
    DefinitelyAbsent,
    Indeterminate,
    AuthOrCapabilityFailure,
}

pub trait DiscoveryRemoteV1 {
    fn list_directory(&mut self, path: &str) -> DirectoryListResultV1;
    fn get_exact(&mut self, path: &str) -> DiscoveryExactGetResultV1;
}

fn canonical_path_input(path: &str) -> bool {
    !path.starts_with('/') && !path.contains("..") && !path.contains('\\')
}

fn exact_lower_hex(value: &str, width: usize) -> bool {
    value.len() == width
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn canonical_uuid_shape(value: &str) -> bool {
    value.len() == 36
        && value.bytes().enumerate().all(|(index, byte)| match index {
            8 | 13 | 18 | 23 => byte == b'-',
            _ => byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte),
        })
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum CandidatePathClassificationV1 {
    Candidate,
    CanonicalIdentityMismatch,
    UnrelatedJunk,
}

pub fn classify_candidate_path_v1(path: &str) -> CandidatePathClassificationV1 {
    if parse_writer_candidate_path_v1(path).is_some()
        || parse_activation_candidate_path_v1(path).is_some()
    {
        return CandidatePathClassificationV1::Candidate;
    }
    let components = path.split('/').collect::<Vec<_>>();
    let commit_looking = if components.len() == 5
        && components[0] == "writers"
        && components[2] == "segments"
        && canonical_uuid_shape(components[1])
        && exact_lower_hex(components[3], SEGMENT_NAME_WIDTH_V1)
    {
        components[4]
            .strip_suffix(".json")
            .map(|filename| filename.split("--").collect::<Vec<_>>())
            .is_some_and(|fields| {
                fields.len() == 3
                    && fields[0].len() == 20
                    && fields[0].bytes().all(|byte| byte.is_ascii_digit())
                    && canonical_uuid_shape(fields[1])
                    && exact_lower_hex(fields[2], 64)
            })
    } else {
        false
    };
    let activation_looking = if components.len() == 2 && components[0] == "activations" {
        components[1]
            .strip_suffix(".json")
            .and_then(|filename| filename.split_once("--"))
            .is_some_and(|(activation_id, content_hash)| {
                canonical_uuid_shape(activation_id) && exact_lower_hex(content_hash, 64)
            })
    } else {
        false
    };
    if commit_looking || activation_looking {
        CandidatePathClassificationV1::CanonicalIdentityMismatch
    } else {
        CandidatePathClassificationV1::UnrelatedJunk
    }
}

fn bytes_to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

pub fn parse_activation_candidate_path_v1(path: &str) -> Option<ObservedCandidateV1> {
    if !canonical_path_input(path) {
        return None;
    }
    let body = path.strip_prefix("activations/")?.strip_suffix(".json")?;
    let (activation_id, content_hash) = body.split_once("--")?;
    if body.matches("--").count() != 1
        || validate_canonical_uuid(activation_id).is_err()
        || !exact_lower_hex(content_hash, 64)
    {
        return None;
    }
    Some(ObservedCandidateV1::Activation {
        path: path.to_string(),
        activation_id: activation_id.to_string(),
        content_hash: content_hash.to_string(),
    })
}

pub fn parse_writer_candidate_path_v1(path: &str) -> Option<ObservedCandidateV1> {
    if !canonical_path_input(path) {
        return None;
    }
    let parts = path.split('/').collect::<Vec<_>>();
    if parts.len() != 5 || parts[0] != "writers" || parts[2] != "segments" {
        return None;
    }
    let writer_id = parts[1];
    let segment_name = parts[3];
    let filename = parts[4].strip_suffix(".json")?;
    let fields = filename.split("--").collect::<Vec<_>>();
    if fields.len() != 3
        || validate_canonical_uuid_v4(writer_id).is_err()
        || !exact_lower_hex(segment_name, SEGMENT_NAME_WIDTH_V1)
        || fields[0].len() != 20
        || !fields[0].bytes().all(|byte| byte.is_ascii_digit())
        || validate_canonical_uuid_v4(fields[1]).is_err()
        || !exact_lower_hex(fields[2], 64)
    {
        return None;
    }
    let writer_seq = fields[0].parse::<u64>().ok()?;
    if writer_seq == 0 {
        return None;
    }
    let commit_ref = CommitRef {
        writer_id: writer_id.to_string(),
        writer_seq: writer_seq.to_string(),
        commit_id: fields[1].to_string(),
        content_hash: fields[2].to_string(),
    };
    if build_commit_remote_path_v1(&commit_ref).ok()?.as_str() != path {
        return None;
    }
    Some(ObservedCandidateV1::Commit {
        path: path.to_string(),
        writer_id: writer_id.to_string(),
        segment_name: segment_name.to_string(),
        writer_seq: writer_seq.to_string(),
        commit_id: fields[1].to_string(),
        content_hash: fields[2].to_string(),
    })
}

fn push_unique(values: &mut Vec<String>, value: String) {
    if !values.contains(&value) {
        values.push(value);
        values.sort();
    }
}

fn push_fatal(state: &mut DiscoveryStateV1, signal: RootFatalSignalV1) {
    if !state.root_fatal_signals.contains(&signal) {
        state.root_fatal_signals.push(signal);
        state.root_fatal_signals.sort_by(|a, b| {
            serde_json::to_string(a)
                .unwrap()
                .cmp(&serde_json::to_string(b).unwrap())
        });
    }
}

pub fn observe_writer_listing_v1(state: &mut DiscoveryStateV1, entries: &[String]) {
    for entry in entries {
        let normalized = if entry.contains('/') {
            entry.trim_end_matches('/').to_string()
        } else {
            format!("writers/{entry}")
        };
        if let Some(writer_id) = normalized.strip_prefix("writers/") {
            if !writer_id.contains('/') && validate_canonical_uuid_v4(writer_id).is_ok() {
                push_unique(&mut state.observed_writers, writer_id.to_string());
            }
        }
    }
}

pub fn observe_segment_listing_v1(
    state: &mut DiscoveryStateV1,
    writer_id: &str,
    entries: &[String],
) {
    for entry in entries {
        let segment = if entry.contains('/') {
            let prefix = format!("writers/{writer_id}/segments/");
            entry
                .trim_end_matches('/')
                .strip_prefix(&prefix)
                .unwrap_or("")
        } else {
            entry.as_str()
        };
        if exact_lower_hex(segment, SEGMENT_NAME_WIDTH_V1) {
            push_unique(
                &mut state.observed_segments,
                format!("{writer_id}/{segment}"),
            );
        }
    }
    update_historical_segments_v1(state);
}

pub fn observe_candidate_listing_v1(state: &mut DiscoveryStateV1, entries: &[String]) {
    for path in entries {
        let candidate = parse_writer_candidate_path_v1(path)
            .or_else(|| parse_activation_candidate_path_v1(path));
        let Some(candidate) = candidate else {
            if classify_candidate_path_v1(path)
                == CandidatePathClassificationV1::CanonicalIdentityMismatch
            {
                push_fatal(
                    state,
                    RootFatalSignalV1 {
                        code: "REMOTE_S2_PATH_IDENTITY_MISMATCH".to_string(),
                        path: path.clone(),
                        writer_id: None,
                        writer_seq: None,
                        safe_writer_frontier: None,
                    },
                );
            }
            continue;
        };
        if !state
            .observed_candidates
            .iter()
            .any(|value| value.path() == path)
        {
            match &candidate {
                ObservedCandidateV1::Activation { .. } => {
                    push_unique(&mut state.observed_activations, path.clone());
                }
                ObservedCandidateV1::Commit {
                    writer_id,
                    segment_name,
                    ..
                } => {
                    push_unique(&mut state.observed_writers, writer_id.clone());
                    push_unique(
                        &mut state.observed_segments,
                        format!("{writer_id}/{segment_name}"),
                    );
                }
            }
            state.observed_candidates.push(candidate);
            state
                .observed_candidates
                .sort_by(|a, b| a.path().cmp(b.path()));
        }
    }
    update_historical_segments_v1(state);
}

pub fn verify_activation_candidate_v1(
    state: &mut DiscoveryStateV1,
    candidate: &ObservedCandidateV1,
    bytes: &[u8],
    verification: &ActivationVerificationV1,
) -> Result<()> {
    if !verify_activation_exact_identity_v1(state, candidate, bytes)? {
        return Ok(());
    }
    retain_activation_verification_v1(state, candidate, bytes, verification)
}

fn verify_activation_exact_identity_v1(
    state: &mut DiscoveryStateV1,
    candidate: &ObservedCandidateV1,
    bytes: &[u8],
) -> Result<bool> {
    let ObservedCandidateV1::Activation {
        path, content_hash, ..
    } = candidate
    else {
        return Err(ProtocolError("not_activation_candidate"));
    };
    let exact_bytes_hash = sha256_hex(bytes);
    if state
        .verified_objects
        .iter()
        .find(|value| value.path == *path)
        .is_some_and(|value| value.exact_bytes_hash != exact_bytes_hash)
        || exact_bytes_hash != *content_hash
    {
        push_fatal(
            state,
            RootFatalSignalV1 {
                code: "REMOTE_IMMUTABLE_PATH_CONTENT_MISMATCH".to_string(),
                path: path.clone(),
                writer_id: None,
                writer_seq: None,
                safe_writer_frontier: None,
            },
        );
        return Ok(false);
    }
    Ok(true)
}

fn retain_activation_verification_v1(
    state: &mut DiscoveryStateV1,
    candidate: &ObservedCandidateV1,
    bytes: &[u8],
    verification: &ActivationVerificationV1,
) -> Result<()> {
    let ObservedCandidateV1::Activation {
        path,
        activation_id,
        content_hash,
    } = candidate
    else {
        return Err(ProtocolError("not_activation_candidate"));
    };
    let exact_bytes_hash = sha256_hex(bytes);
    if verification.activation_id != *activation_id {
        push_fatal(
            state,
            RootFatalSignalV1 {
                code: "REMOTE_S2_PATH_BODY_IDENTITY_MISMATCH".to_string(),
                path: path.clone(),
                writer_id: None,
                writer_seq: None,
                safe_writer_frontier: None,
            },
        );
        return Ok(());
    }
    if !verification.semantic_profile_supported || !verification.required_features_supported {
        push_fatal(
            state,
            RootFatalSignalV1 {
                code: "REMOTE_S2_UNSUPPORTED_FEATURE".to_string(),
                path: path.clone(),
                writer_id: None,
                writer_seq: None,
                safe_writer_frontier: None,
            },
        );
        return Ok(());
    }
    if !state
        .verified_objects
        .iter()
        .any(|value| value.path == *path)
    {
        state.verified_objects.push(VerifiedRemoteObjectV1 {
            path: path.clone(),
            kind: "activation".to_string(),
            exact_bytes_hash,
            exact_bytes_hex: bytes_to_hex(bytes),
            content_hash: content_hash.clone(),
            commit_ref: None,
            activation_id: Some(activation_id.clone()),
            fingerprint_evidence: verification.legacy_fingerprint.as_ref().map_or(
                VerifiedFingerprintEvidenceV1::Null,
                |value| VerifiedFingerprintEvidenceV1::Value {
                    value: value.clone(),
                },
            ),
        });
        state.verified_objects.sort_by(|a, b| a.path.cmp(&b.path));
    }
    Ok(())
}

fn update_historical_segments_v1(state: &mut DiscoveryStateV1) {
    for writer_id in state.observed_writers.clone() {
        let prefix = format!("{writer_id}/");
        let highest = state
            .observed_segments
            .iter()
            .filter_map(|key| key.strip_prefix(&prefix))
            .filter_map(|segment| u64::from_str_radix(segment, 16).ok())
            .max();
        let Some(highest) = highest else { continue };
        if highest > MAX_RETAINED_SEGMENTS_V1 {
            push_fatal(
                state,
                RootFatalSignalV1 {
                    code: "DISCOVERY_PROTOCOL_LIMIT_EXCEEDED".to_string(),
                    path: format!("writers/{writer_id}/segments/"),
                    writer_id: None,
                    writer_seq: None,
                    safe_writer_frontier: None,
                },
            );
            continue;
        }
        for index in 0..highest {
            push_unique(
                &mut state.historical_closed_segments,
                format!("{writer_id}/{index:0SEGMENT_NAME_WIDTH_V1$x}"),
            );
        }
    }
}

pub fn choose_historical_audit_target_v1(state: &mut DiscoveryStateV1) -> Option<String> {
    let mut by_writer = BTreeMap::<String, Vec<String>>::new();
    for key in &state.historical_closed_segments {
        let (writer, segment) = key.rsplit_once('/')?;
        by_writer
            .entry(writer.to_string())
            .or_default()
            .push(segment.to_string());
    }
    if by_writer.is_empty() {
        return None;
    }
    let writers = by_writer.keys().cloned().collect::<Vec<_>>();
    let writer_id = state
        .historical_audit_cursor
        .last_writer_id
        .as_ref()
        .and_then(|previous| writers.iter().find(|writer| *writer > previous))
        .cloned()
        .unwrap_or_else(|| writers[0].clone());
    let segments = by_writer.get_mut(&writer_id).unwrap();
    segments.sort();
    let previous_segment = state
        .historical_audit_cursor
        .last_segment_by_writer
        .get(&writer_id)
        .and_then(Clone::clone);
    let segment = previous_segment
        .as_ref()
        .and_then(|previous| segments.iter().find(|segment| *segment > previous))
        .cloned()
        .unwrap_or_else(|| segments[0].clone());
    state.historical_audit_cursor.last_writer_id = Some(writer_id.clone());
    state
        .historical_audit_cursor
        .last_segment_by_writer
        .insert(writer_id.clone(), Some(segment.clone()));
    Some(format!("{writer_id}/{segment}"))
}

fn ref_key(value: &CommitRef) -> String {
    format!(
        "{}/{}/{}/{}",
        value.writer_id, value.writer_seq, value.commit_id, value.content_hash
    )
}

fn add_target(state: &mut DiscoveryStateV1, value: &CommitRef) {
    if state
        .verified_objects
        .iter()
        .filter_map(|object| object.commit_ref.as_ref())
        .any(|known| known == value)
        || state.targeted_queue.contains(value)
    {
        return;
    }
    state.targeted_queue.push(value.clone());
    state.targeted_queue.sort_by(compare_commit_ref_v1);
}

fn recompute_gaps(state: &mut DiscoveryStateV1, writer_id: &str) {
    let seqs = state
        .verified_objects
        .iter()
        .filter_map(|object| object.commit_ref.as_ref())
        .filter(|value| value.writer_id == writer_id)
        .filter_map(|value| parse_writer_seq(&value.writer_seq).ok())
        .collect::<BTreeSet<_>>();
    let Some(highest) = seqs.last().copied() else {
        return;
    };
    if highest > MAX_TRACKED_GAP_SEQUENCE_V1 {
        push_fatal(
            state,
            RootFatalSignalV1 {
                code: "DISCOVERY_PROTOCOL_LIMIT_EXCEEDED".to_string(),
                path: format!("writers/{writer_id}/gaps"),
                writer_id: None,
                writer_seq: None,
                safe_writer_frontier: None,
            },
        );
        return;
    }
    state.known_gaps.retain(|key| {
        let Some(seq) = key.strip_prefix(&format!("{writer_id}/")) else {
            return true;
        };
        seq.parse::<u64>().map_or(true, |seq| !seqs.contains(&seq))
    });
    for seq in 1..highest {
        if !seqs.contains(&seq) {
            push_unique(&mut state.known_gaps, format!("{writer_id}/{seq}"));
        }
    }
}

pub fn verify_commit_candidate_v1(
    state: &mut DiscoveryStateV1,
    candidate: &ObservedCandidateV1,
    bytes: &[u8],
) -> Result<Option<CommitV1>> {
    let ObservedCandidateV1::Commit {
        path,
        writer_id,
        writer_seq,
        commit_id,
        content_hash,
        ..
    } = candidate
    else {
        return Err(ProtocolError("not_commit_candidate"));
    };
    let exact_bytes_hash = sha256_hex(bytes);
    if state
        .verified_objects
        .iter()
        .find(|value| value.path == *path)
        .is_some_and(|value| value.exact_bytes_hash != exact_bytes_hash)
        || exact_bytes_hash != *content_hash
    {
        push_fatal(
            state,
            RootFatalSignalV1 {
                code: "REMOTE_IMMUTABLE_PATH_CONTENT_MISMATCH".to_string(),
                path: path.clone(),
                writer_id: None,
                writer_seq: None,
                safe_writer_frontier: None,
            },
        );
        return Ok(None);
    }
    let commit = match decode_frozen_wire_commit_v1(bytes) {
        Ok(commit) => commit,
        Err(_) => {
            push_fatal(
                state,
                RootFatalSignalV1 {
                    code: "REMOTE_S2_OBJECT_INVALID".to_string(),
                    path: path.clone(),
                    writer_id: None,
                    writer_seq: None,
                    safe_writer_frontier: None,
                },
            );
            return Ok(None);
        }
    };
    if commit.writer_id != *writer_id
        || commit.writer_seq != *writer_seq
        || commit.commit_id != *commit_id
        || commit.content_hash != *content_hash
    {
        push_fatal(
            state,
            RootFatalSignalV1 {
                code: "REMOTE_S2_PATH_BODY_IDENTITY_MISMATCH".to_string(),
                path: path.clone(),
                writer_id: None,
                writer_seq: None,
                safe_writer_frontier: None,
            },
        );
        return Ok(None);
    }
    let commit_ref = commit.commit_ref();
    if !state
        .verified_objects
        .iter()
        .any(|value| value.path == *path)
    {
        state.verified_objects.push(VerifiedRemoteObjectV1 {
            path: path.clone(),
            kind: "commit".to_string(),
            exact_bytes_hash,
            exact_bytes_hex: bytes_to_hex(bytes),
            content_hash: content_hash.clone(),
            commit_ref: Some(commit_ref.clone()),
            activation_id: None,
            fingerprint_evidence: VerifiedFingerprintEvidenceV1::Missing,
        });
        state.verified_objects.sort_by(|a, b| a.path.cmp(&b.path));
    }
    let mut same_seq_objects = state
        .verified_objects
        .iter()
        .filter_map(|value| {
            value.commit_ref.as_ref().and_then(|other| {
                (other.writer_id == commit_ref.writer_id
                    && other.writer_seq == commit_ref.writer_seq)
                    .then(|| (value.path.clone(), other.clone()))
            })
        })
        .collect::<Vec<_>>();
    same_seq_objects.sort_by(|a, b| a.0.cmp(&b.0));
    let alternatives = same_seq_objects
        .iter()
        .map(|(_, value)| format!("{}/{}", value.commit_id, value.content_hash))
        .collect::<BTreeSet<_>>();
    if alternatives.len() > 1 {
        let safe = parse_writer_seq(&commit_ref.writer_seq)? - 1;
        state.root_fatal_signals.retain(|signal| {
            !(signal.code == "WRITER_FORK"
                && signal.writer_id.as_ref() == Some(&commit_ref.writer_id)
                && signal.writer_seq.as_ref() == Some(&commit_ref.writer_seq))
        });
        push_fatal(
            state,
            RootFatalSignalV1 {
                code: "WRITER_FORK".to_string(),
                path: same_seq_objects[0].0.clone(),
                writer_id: Some(commit_ref.writer_id.clone()),
                writer_seq: Some(commit_ref.writer_seq.clone()),
                safe_writer_frontier: Some(safe.to_string()),
            },
        );
    }
    if let Some(previous) = &commit.previous_writer_commit {
        add_target(state, previous);
    }
    for dependency in &commit.basis_clock {
        add_target(state, dependency);
    }
    state
        .targeted_queue
        .retain(|value| ref_key(value) != ref_key(&commit_ref));
    recompute_gaps(state, &commit_ref.writer_id);
    Ok(Some(commit))
}

fn bounded_entries_v1(
    state: &mut DiscoveryStateV1,
    path: &str,
    result: DirectoryListResultV1,
    budget: usize,
) -> Vec<String> {
    let DirectoryListResultV1::Entries(entries) = result else {
        state.last_round_indeterminate = true;
        return vec![];
    };
    let mut entries = entries
        .into_iter()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    if entries.len() > budget {
        push_fatal(
            state,
            RootFatalSignalV1 {
                code: "DISCOVERY_PROTOCOL_LIMIT_EXCEEDED".to_string(),
                path: path.to_string(),
                writer_id: None,
                writer_seq: None,
                safe_writer_frontier: None,
            },
        );
        entries.truncate(budget);
    }
    entries
}

fn scan_targets_for_writer_v1(
    state: &mut DiscoveryStateV1,
    writer_id: &str,
    audit_target: Option<&str>,
    budget: usize,
) -> Vec<String> {
    let prefix = format!("{writer_id}/");
    let mut segments = state
        .observed_segments
        .iter()
        .filter(|key| key.starts_with(&prefix))
        .cloned()
        .collect::<Vec<_>>();
    segments.sort();
    let mut targets = vec![];
    let add = |targets: &mut Vec<String>, value: String| {
        if !targets.contains(&value) {
            targets.push(value);
        }
    };
    if let Some(target) = audit_target.filter(|target| target.starts_with(&prefix)) {
        add(&mut targets, target.to_string());
    }
    if let Some(highest) = segments.last() {
        if let Ok(high_index) = u64::from_str_radix(&highest[prefix.len()..], 16) {
            for offset in 0..=2_u64 {
                if high_index >= offset {
                    add(
                        &mut targets,
                        format!(
                            "{writer_id}/{:0width$x}",
                            high_index - offset,
                            width = SEGMENT_NAME_WIDTH_V1
                        ),
                    );
                }
            }
        }
    }
    let mut gap_segments = BTreeSet::new();
    for gap in state
        .known_gaps
        .iter()
        .filter(|key| key.starts_with(&prefix))
    {
        if let Ok(seq) = gap[prefix.len()..].parse::<u64>() {
            if seq > 0 {
                gap_segments.insert(format!(
                    "{writer_id}/{:0width$x}",
                    (seq - 1) / 256,
                    width = SEGMENT_NAME_WIDTH_V1
                ));
            }
        }
    }
    if !gap_segments.is_empty() {
        let previous = state
            .gap_segment_cursor_by_writer
            .get(writer_id)
            .and_then(Clone::clone);
        let target = previous
            .as_ref()
            .and_then(|previous| gap_segments.iter().find(|value| *value > previous))
            .cloned()
            .unwrap_or_else(|| gap_segments.iter().next().unwrap().clone());
        add(&mut targets, target.clone());
        state
            .gap_segment_cursor_by_writer
            .insert(writer_id.to_string(), Some(target));
    }
    targets.truncate(budget);
    targets
}

fn next_fair_item_v1(
    items: &[String],
    after: Option<&str>,
    selected: &BTreeSet<String>,
) -> Option<String> {
    let available = items
        .iter()
        .filter(|item| !selected.contains(*item))
        .cloned()
        .collect::<BTreeSet<_>>();
    after
        .and_then(|after| available.iter().find(|item| item.as_str() > after).cloned())
        .or_else(|| available.iter().next().cloned())
}

fn schedule_exact_work_v1(
    state: &mut DiscoveryStateV1,
    budgets: &DiscoveryBudgetsV1,
) -> Result<Vec<(String, ExactWorkClassV1)>> {
    let verified_paths = state
        .verified_objects
        .iter()
        .map(|value| value.path.clone())
        .collect::<BTreeSet<_>>();
    let terminal_paths = state
        .terminal_candidate_paths
        .iter()
        .cloned()
        .collect::<BTreeSet<_>>();
    let mut work = BTreeMap::<ExactWorkClassV1, Vec<String>>::new();
    work.insert(
        ExactWorkClassV1::Dependency,
        state
            .targeted_queue
            .iter()
            .map(build_commit_remote_path_v1)
            .collect::<Result<Vec<_>>>()?,
    );
    work.insert(
        ExactWorkClassV1::Candidate,
        state
            .observed_candidates
            .iter()
            .filter(|value| {
                !verified_paths.contains(value.path()) && !terminal_paths.contains(value.path())
            })
            .map(|value| value.path().to_string())
            .collect(),
    );
    work.insert(
        ExactWorkClassV1::Reverify,
        state
            .reverification_queue
            .iter()
            .filter(|path| verified_paths.contains(*path))
            .cloned()
            .collect(),
    );
    let classes = ExactWorkClassV1::all();
    let mut class_index = classes
        .iter()
        .position(|class| *class == state.exact_work_scheduler.next_class)
        .unwrap_or(0);
    let mut selected = BTreeSet::new();
    let mut result = vec![];
    let mut dependency_count = 0_usize;
    while result.len() < budgets.max_exact_fetches_per_sync {
        let mut scheduled = false;
        for offset in 0..classes.len() {
            let index = (class_index + offset) % classes.len();
            let class = classes[index];
            if class == ExactWorkClassV1::Dependency
                && dependency_count >= budgets.max_dependency_targets_per_sync
            {
                continue;
            }
            let after = state
                .exact_work_scheduler
                .after_by_class
                .get(&class)
                .and_then(|value| value.as_deref());
            let Some(path) = next_fair_item_v1(work.get(&class).unwrap(), after, &selected) else {
                continue;
            };
            if class == ExactWorkClassV1::Dependency {
                dependency_count += 1;
            }
            selected.insert(path.clone());
            result.push((path.clone(), class));
            state
                .exact_work_scheduler
                .after_by_class
                .insert(class, Some(path));
            class_index = (index + 1) % classes.len();
            state.exact_work_scheduler.next_class = classes[class_index];
            scheduled = true;
            break;
        }
        if !scheduled {
            break;
        }
    }
    Ok(result)
}

pub fn run_discovery_round_v1<R, V>(
    prior: &DiscoveryStateV1,
    remote: &mut R,
    activation_validator: &mut V,
    budgets: &DiscoveryBudgetsV1,
) -> Result<DiscoveryStateV1>
where
    R: DiscoveryRemoteV1,
    V: FnMut(&[u8]) -> ActivationValidatorResultV1,
{
    let mut state = prior.clone();
    state.last_round_indeterminate = false;
    state.last_round_scheduled_lists.clear();
    state.last_round_scheduled_gets.clear();

    state
        .last_round_scheduled_lists
        .push("activations/".to_string());
    let activation_result = remote.list_directory("activations/");
    let activation_entries = bounded_entries_v1(
        &mut state,
        "activations/",
        activation_result,
        budgets.max_listing_entries_per_directory,
    );
    observe_candidate_listing_v1(&mut state, &activation_entries);

    state
        .last_round_scheduled_lists
        .push("writers/".to_string());
    let writer_result = remote.list_directory("writers/");
    let writer_entries = bounded_entries_v1(
        &mut state,
        "writers/",
        writer_result,
        budgets.max_listing_entries_per_directory,
    );
    observe_writer_listing_v1(&mut state, &writer_entries);

    for writer_id in state.observed_writers.clone() {
        let path = format!("writers/{writer_id}/segments/");
        state.last_round_scheduled_lists.push(path.clone());
        let result = remote.list_directory(&path);
        let entries = bounded_entries_v1(
            &mut state,
            &path,
            result,
            budgets.max_listing_entries_per_directory,
        );
        observe_segment_listing_v1(&mut state, &writer_id, &entries);
    }

    let audit_target = choose_historical_audit_target_v1(&mut state);
    for writer_id in state.observed_writers.clone() {
        let targets = scan_targets_for_writer_v1(
            &mut state,
            &writer_id,
            audit_target.as_deref(),
            budgets.max_segments_per_writer_per_sync,
        );
        for key in targets {
            let segment = &key[writer_id.len() + 1..];
            let path = format!("writers/{writer_id}/segments/{segment}/");
            state.last_round_scheduled_lists.push(path.clone());
            let result = remote.list_directory(&path);
            let entries = bounded_entries_v1(
                &mut state,
                &path,
                result,
                budgets.max_listing_entries_per_directory,
            );
            if audit_target.as_ref() == Some(&key) {
                for entry in &entries {
                    if parse_writer_candidate_path_v1(entry).is_some()
                        && state
                            .verified_objects
                            .iter()
                            .any(|value| value.path == *entry)
                    {
                        push_unique(&mut state.reverification_queue, entry.clone());
                    }
                }
            }
            observe_candidate_listing_v1(&mut state, &entries);
        }
    }

    let scheduled = schedule_exact_work_v1(&mut state, budgets)?;
    state.last_round_scheduled_gets = scheduled.iter().map(|(path, _)| path.clone()).collect();
    for (path, work_class) in scheduled {
        match remote.get_exact(&path) {
            DiscoveryExactGetResultV1::Indeterminate
            | DiscoveryExactGetResultV1::AuthOrCapabilityFailure => {
                state.last_round_indeterminate = true;
            }
            DiscoveryExactGetResultV1::DefinitelyAbsent => {}
            DiscoveryExactGetResultV1::DefinitelyPresent(bytes) => {
                if work_class == ExactWorkClassV1::Reverify {
                    state.reverification_queue.retain(|value| value != &path);
                }
                let candidate = state
                    .observed_candidates
                    .iter()
                    .find(|value| value.path() == path)
                    .cloned()
                    .or_else(|| parse_writer_candidate_path_v1(&path))
                    .or_else(|| parse_activation_candidate_path_v1(&path));
                if let Some(candidate) = candidate {
                    if !state
                        .observed_candidates
                        .iter()
                        .any(|value| value.path() == path)
                    {
                        observe_candidate_listing_v1(&mut state, std::slice::from_ref(&path));
                    }
                    match &candidate {
                        ObservedCandidateV1::Commit { .. } => {
                            verify_commit_candidate_v1(&mut state, &candidate, &bytes)?;
                        }
                        ObservedCandidateV1::Activation { .. } => {
                            if verify_activation_exact_identity_v1(&mut state, &candidate, &bytes)?
                            {
                                match activation_validator(&bytes) {
                                    Ok(verification) => retain_activation_verification_v1(
                                        &mut state,
                                        &candidate,
                                        &bytes,
                                        &verification,
                                    )?,
                                    Err(ActivationValidatorErrorV1::ProtocolValidation) => {
                                        push_fatal(
                                            &mut state,
                                            RootFatalSignalV1 {
                                                code: "REMOTE_S2_OBJECT_INVALID".to_string(),
                                                path: path.clone(),
                                                writer_id: None,
                                                writer_seq: None,
                                                safe_writer_frontier: None,
                                            },
                                        );
                                    }
                                    Err(ActivationValidatorErrorV1::Internal(error)) => {
                                        return Err(error);
                                    }
                                }
                            }
                        }
                    }
                    if !state
                        .verified_objects
                        .iter()
                        .any(|value| value.path == path)
                    {
                        push_unique(&mut state.terminal_candidate_paths, path);
                    }
                }
            }
        }
    }
    Ok(state)
}

pub fn canonical_discovery_projection_v1(state: &DiscoveryStateV1) -> serde_json::Value {
    serde_json::json!({
        "observedActivations": state.observed_activations,
        "observedWriters": state.observed_writers,
        "observedSegments": state.observed_segments,
        "observedCandidates": state.observed_candidates,
        "verifiedObjects": state.verified_objects,
        "knownGaps": state.known_gaps,
        "targetedQueue": state.targeted_queue,
        "historicalClosedSegments": state.historical_closed_segments,
        "historicalAuditCursor": state.historical_audit_cursor,
        "gapSegmentCursorByWriter": state.gap_segment_cursor_by_writer,
        "reverificationQueue": state.reverification_queue,
        "terminalCandidatePaths": state.terminal_candidate_paths,
        "exactWorkScheduler": state.exact_work_scheduler,
        "lastRoundScheduledLists": state.last_round_scheduled_lists,
        "lastRoundScheduledGets": state.last_round_scheduled_gets,
        "rootFatalSignals": state.root_fatal_signals,
    })
}

pub fn retained_verified_commit_bytes_v1(state: &DiscoveryStateV1) -> Result<Vec<Vec<u8>>> {
    let mut objects = state
        .verified_objects
        .iter()
        .filter(|object| object.kind == "commit")
        .collect::<Vec<_>>();
    objects.sort_by(|a, b| a.path.cmp(&b.path));
    objects
        .into_iter()
        .map(|object| {
            if object.exact_bytes_hex.len() % 2 != 0 {
                return Err(ProtocolError("discovery_state_corrupt_exact_bytes"));
            }
            (0..object.exact_bytes_hex.len())
                .step_by(2)
                .map(|index| {
                    u8::from_str_radix(&object.exact_bytes_hex[index..index + 2], 16)
                        .map_err(|_| ProtocolError("discovery_state_corrupt_exact_bytes"))
                })
                .collect()
        })
        .collect()
}

pub fn compare_candidate_identity_v1(
    left: &ObservedCandidateV1,
    right: &ObservedCandidateV1,
) -> Ordering {
    left.path().cmp(right.path())
}

pub fn validate_target_ref_v1(value: &CommitRef) -> Result<()> {
    validate_commit_ref(value)
}
