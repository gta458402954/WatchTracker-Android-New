use super::canonical::{
    compare_commit_ref_v1, compare_entity_key_v1, jcs_bytes, parse_writer_seq, sha256_hex,
    validate_canonical_uuid_v4, validate_commit_ref, validate_entity_key, validate_safe_integer,
    validate_timestamp, ProtocolError, Result,
};
use super::conflict::{
    build_entity_conflict_core_v1, build_relation_conflict_core_v1, entity_conflict_id_v1,
    relation_conflict_id_v1,
};
use super::semantic::{
    business_field_order, canonical_semantic_value, validate_native_entity, validate_tombstone,
};
use super::types::{
    CommitMutationV1, CommitRef, CommitV1, DuplicateDiagnosticV1, EntityConflictAlternativeInput,
    EntityConflictKind, EntityVersionV1, FrontierEntryV1, HistoricalValidity,
    HistoricalValidityState, MaterializedEntityV1, MaterializedEntryV1, MetadataVariantV1,
    RelationConflictKind, RelationConflictV1, RelationDetectionV1, RelationParticipant,
    ValidityEntryV1, VerifiedReplayV1, WriterForkV1,
};
use serde::de::{self, Deserialize, Deserializer, MapAccess, SeqAccess, Visitor};
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fmt;

pub const MAX_MUTATIONS_PER_COMMIT_V1: usize = 256;

fn exact_ref_key(reference: &CommitRef) -> String {
    format!(
        "{}\0{}\0{}\0{}",
        reference.writer_id, reference.writer_seq, reference.commit_id, reference.content_hash
    )
}

fn entity_key_id(key: &Value) -> Result<Vec<u8>> {
    jcs_bytes(key)
}

fn same_ref(a: &CommitRef, b: &CommitRef) -> bool {
    compare_commit_ref_v1(a, b).is_eq()
}

fn sorted_refs(refs: &[CommitRef]) -> Vec<CommitRef> {
    let mut result = refs.to_vec();
    result.sort_by(compare_commit_ref_v1);
    result
}

fn same_ref_set(a: &[CommitRef], b: &[CommitRef]) -> bool {
    let left = sorted_refs(a);
    let right = sorted_refs(b);
    left.len() == right.len() && left.iter().zip(&right).all(|(a, b)| same_ref(a, b))
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .as_bytes()
            .iter()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(byte))
}

fn validate_mutation_envelope(mutation: &CommitMutationV1) -> Result<()> {
    validate_canonical_uuid_v4(&mutation.local_mutation_id)?;
    validate_entity_key(&mutation.entity_key)?;
    if mutation.entity_key[0] != mutation.entity_type {
        return Err(ProtocolError("mutation_entity_type_mismatch"));
    }
    if !matches!(mutation.operation.as_str(), "upsert" | "tombstone") {
        return Err(ProtocolError("invalid_mutation_operation"));
    }
    if !mutation.value.is_object() {
        return Err(ProtocolError("invalid_mutation_value"));
    }
    for reference in &mutation.base_frontier {
        validate_commit_ref(reference)?;
    }
    if mutation
        .base_frontier
        .windows(2)
        .any(|pair| !compare_commit_ref_v1(&pair[0], &pair[1]).is_lt())
    {
        return Err(ProtocolError("noncanonical_entity_base_frontier"));
    }
    let mut fields = BTreeSet::new();
    for field in &mutation.changed_fields {
        if !fields.insert(field) {
            return Err(ProtocolError("invalid_changed_fields"));
        }
    }
    Ok(())
}

pub fn validate_commit_envelope_v1(commit: &CommitV1) -> Result<()> {
    if commit.protocol != "watchtracker-s2-lite" || commit.protocol_version != 1 {
        return Err(ProtocolError("unsupported_protocol_version"));
    }
    if commit.s2_semantic_profile_version != 1 {
        return Err(ProtocolError("unsupported_semantic_profile"));
    }
    if !commit.required_features.is_empty() {
        return Err(ProtocolError("unsupported_required_feature"));
    }
    validate_canonical_uuid_v4(&commit.writer_id)?;
    parse_writer_seq(&commit.writer_seq)?;
    validate_canonical_uuid_v4(&commit.commit_id)?;
    if !valid_sha256(&commit.content_hash) {
        return Err(ProtocolError("invalid_content_hash"));
    }
    if let Some(previous) = &commit.previous_writer_commit {
        validate_commit_ref(previous)?;
    }
    let mut writers = BTreeSet::new();
    for reference in &commit.basis_clock {
        validate_commit_ref(reference)?;
        if !writers.insert(reference.writer_id.as_str()) {
            return Err(ProtocolError("duplicate_basis_writer"));
        }
    }
    if commit
        .basis_clock
        .windows(2)
        .any(|pair| !compare_commit_ref_v1(&pair[0], &pair[1]).is_lt())
    {
        return Err(ProtocolError("noncanonical_basis_clock"));
    }
    if !matches!(
        commit.commit_kind.as_str(),
        "mutation" | "resolution" | "bootstrap"
    ) {
        return Err(ProtocolError("invalid_commit_kind"));
    }
    validate_timestamp(&commit.created_at)?;
    if !matches!(
        commit.source.r#type.as_str(),
        "native" | "manual-resolution" | "legacy-bootstrap" | "new-root-bootstrap"
    ) {
        return Err(ProtocolError("invalid_commit_source"));
    }
    if commit.resolves.iter().any(|value| !valid_sha256(value))
        || commit.resolves.windows(2).any(|pair| pair[0] >= pair[1])
    {
        return Err(ProtocolError("invalid_resolves"));
    }
    if commit.commit_kind == "mutation" {
        if !commit.resolves.is_empty() || commit.source.r#type != "native" {
            return Err(ProtocolError("invalid_mutation_commit"));
        }
    } else if commit.commit_kind == "resolution" {
        if commit.source.r#type != "manual-resolution" || commit.resolves.is_empty() {
            return Err(ProtocolError("invalid_resolution_commit"));
        }
    } else if !commit.resolves.is_empty()
        || !matches!(
            commit.source.r#type.as_str(),
            "legacy-bootstrap" | "new-root-bootstrap"
        )
    {
        return Err(ProtocolError("invalid_bootstrap_commit"));
    }
    if commit.mutations.is_empty() || commit.mutations.len() > MAX_MUTATIONS_PER_COMMIT_V1 {
        return Err(ProtocolError("invalid_mutation_count"));
    }
    let mut mutation_ids = BTreeSet::new();
    let mut keys = Vec::<Value>::new();
    for mutation in &commit.mutations {
        validate_mutation_envelope(mutation)?;
        if !mutation_ids.insert(mutation.local_mutation_id.as_str()) {
            return Err(ProtocolError("duplicate_local_mutation_id"));
        }
        if keys
            .iter()
            .any(|key| compare_entity_key_v1(key, &mutation.entity_key).is_eq())
        {
            return Err(ProtocolError("duplicate_entity_key"));
        }
        keys.push(mutation.entity_key.clone());
    }
    Ok(())
}

struct StrictJsonValueV1(Value);

impl<'de> Deserialize<'de> for StrictJsonValueV1 {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_any(StrictJsonVisitorV1)
    }
}

struct StrictJsonVisitorV1;

impl<'de> Visitor<'de> for StrictJsonVisitorV1 {
    type Value = StrictJsonValueV1;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a JSON value without duplicate object keys")
    }

    fn visit_bool<E>(self, value: bool) -> std::result::Result<Self::Value, E> {
        Ok(StrictJsonValueV1(Value::Bool(value)))
    }

    fn visit_i64<E>(self, value: i64) -> std::result::Result<Self::Value, E> {
        Ok(StrictJsonValueV1(Value::Number(value.into())))
    }

    fn visit_u64<E>(self, value: u64) -> std::result::Result<Self::Value, E> {
        Ok(StrictJsonValueV1(Value::Number(value.into())))
    }

    fn visit_f64<E>(self, value: f64) -> std::result::Result<Self::Value, E>
    where
        E: de::Error,
    {
        serde_json::Number::from_f64(value)
            .map(Value::Number)
            .map(StrictJsonValueV1)
            .ok_or_else(|| E::custom("non-finite JSON number"))
    }

    fn visit_str<E>(self, value: &str) -> std::result::Result<Self::Value, E> {
        Ok(StrictJsonValueV1(Value::String(value.to_owned())))
    }

    fn visit_string<E>(self, value: String) -> std::result::Result<Self::Value, E> {
        Ok(StrictJsonValueV1(Value::String(value)))
    }

    fn visit_none<E>(self) -> std::result::Result<Self::Value, E> {
        Ok(StrictJsonValueV1(Value::Null))
    }

    fn visit_unit<E>(self) -> std::result::Result<Self::Value, E> {
        Ok(StrictJsonValueV1(Value::Null))
    }

    fn visit_seq<A>(self, mut sequence: A) -> std::result::Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        let mut values = Vec::new();
        while let Some(value) = sequence.next_element::<StrictJsonValueV1>()? {
            values.push(value.0);
        }
        Ok(StrictJsonValueV1(Value::Array(values)))
    }

    fn visit_map<A>(self, mut object: A) -> std::result::Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut values = Map::new();
        while let Some(key) = object.next_key::<String>()? {
            if values.contains_key(&key) {
                return Err(de::Error::custom("duplicate JSON object key"));
            }
            let value = object.next_value::<StrictJsonValueV1>()?;
            values.insert(key, value.0);
        }
        Ok(StrictJsonValueV1(Value::Object(values)))
    }
}

fn validate_frozen_json_bytes_v1(raw_json: &[u8]) -> Result<Value> {
    const ENCODING_MARKERS: [&[u8]; 5] = [
        &[0xef, 0xbb, 0xbf],
        &[0xfe, 0xff],
        &[0xff, 0xfe],
        &[0x00, 0x00, 0xfe, 0xff],
        &[0xff, 0xfe, 0x00, 0x00],
    ];
    if ENCODING_MARKERS
        .iter()
        .any(|marker| raw_json.starts_with(marker))
        || std::str::from_utf8(raw_json).is_err()
    {
        return Err(ProtocolError("invalid_commit_json_bytes"));
    }
    let mut deserializer = serde_json::Deserializer::from_slice(raw_json);
    let value = StrictJsonValueV1::deserialize(&mut deserializer)
        .map_err(|_| ProtocolError("invalid_commit_json"))?;
    deserializer
        .end()
        .map_err(|_| ProtocolError("invalid_commit_json"))?;
    Ok(value.0)
}

pub fn decode_frozen_wire_commit_v1(raw_json: &[u8]) -> Result<CommitV1> {
    let mut wire = validate_frozen_json_bytes_v1(raw_json)?;
    let object = wire
        .as_object_mut()
        .ok_or(ProtocolError("invalid_commit_envelope"))?;
    let expected = [
        "protocol",
        "protocolVersion",
        "s2SemanticProfileVersion",
        "requiredFeatures",
        "writerId",
        "writerSeq",
        "commitId",
        "previousWriterCommit",
        "basisClock",
        "commitKind",
        "createdAt",
        "source",
        "mutations",
    ]
    .into_iter()
    .collect::<BTreeSet<_>>();
    let actual = object.keys().map(String::as_str).collect::<BTreeSet<_>>();
    if !expected.iter().all(|field| actual.contains(field))
        || actual
            .iter()
            .any(|field| !expected.contains(field) && *field != "resolves")
    {
        return Err(ProtocolError("invalid_commit_envelope"));
    }
    if object.get("commitKind") == Some(&Value::String("resolution".to_string()))
        && !object.contains_key("resolves")
    {
        return Err(ProtocolError("invalid_commit_envelope"));
    }
    object
        .entry("resolves".to_string())
        .or_insert_with(|| Value::Array(Vec::new()));
    if validate_safe_integer(&object["protocolVersion"], 1, 1).is_err() {
        return Err(ProtocolError("unsupported_protocol_version"));
    }
    if validate_safe_integer(&object["s2SemanticProfileVersion"], 1, 1).is_err() {
        return Err(ProtocolError("unsupported_semantic_profile"));
    }
    object.insert("protocolVersion".to_string(), json!(1));
    object.insert("s2SemanticProfileVersion".to_string(), json!(1));
    object.insert(
        "contentHash".to_string(),
        Value::String(sha256_hex(raw_json)),
    );
    let commit = serde_json::from_value::<CommitV1>(wire)
        .map_err(|_| ProtocolError("invalid_commit_envelope"))?;
    validate_commit_envelope_v1(&commit)?;
    Ok(commit)
}

pub fn validate_writer_chain_link_v1(
    commit: &CommitV1,
    verified_dependencies: &HashMap<String, CommitV1>,
) -> Result<()> {
    let seq = parse_writer_seq(&commit.writer_seq)?;
    let own_basis = commit
        .basis_clock
        .iter()
        .find(|reference| reference.writer_id == commit.writer_id);
    if seq == 1 {
        if commit.previous_writer_commit.is_some() || own_basis.is_some() {
            return Err(ProtocolError("invalid_writer_causal_chain"));
        }
        return Ok(());
    }
    let previous = commit
        .previous_writer_commit
        .as_ref()
        .ok_or(ProtocolError("invalid_writer_causal_chain"))?;
    if previous.writer_id != commit.writer_id
        || parse_writer_seq(&previous.writer_seq)? != seq - 1
        || own_basis.map_or(true, |basis| !same_ref(previous, basis))
        || !verified_dependencies.contains_key(&exact_ref_key(previous))
    {
        return Err(ProtocolError("invalid_writer_causal_chain"));
    }
    Ok(())
}

pub fn detect_writer_forks_v1(commits: &[CommitV1]) -> Vec<WriterForkV1> {
    let mut groups = BTreeMap::<(String, u64), Vec<CommitRef>>::new();
    for commit in commits {
        let reference = commit.commit_ref();
        let Ok(seq) = parse_writer_seq(&reference.writer_seq) else {
            continue;
        };
        if validate_commit_ref(&reference).is_err() {
            continue;
        }
        let refs = groups
            .entry((reference.writer_id.clone(), seq))
            .or_default();
        if !refs.iter().any(|item| same_ref(item, &reference)) {
            refs.push(reference);
        }
    }
    groups
        .into_iter()
        .filter_map(|((writer_id, seq), mut alternatives)| {
            if alternatives.len() < 2 {
                return None;
            }
            alternatives.sort_by(compare_commit_ref_v1);
            Some(WriterForkV1 {
                writer_id,
                writer_seq: seq.to_string(),
                alternatives,
                safe_writer_frontier: (seq - 1).to_string(),
            })
        })
        .collect()
}

fn ancestor_ref_keys(heads: &[CommitRef], commits: &HashMap<String, CommitV1>) -> HashSet<String> {
    let mut result = HashSet::new();
    let mut pending = heads.to_vec();
    while let Some(reference) = pending.pop() {
        let key = exact_ref_key(&reference);
        if result.contains(&key) {
            continue;
        }
        let Some(commit) = commits.get(&key) else {
            continue;
        };
        result.insert(key);
        pending.extend(commit.basis_clock.clone());
    }
    result
}

fn pending_cycle_keys(
    commits: &HashMap<String, CommitV1>,
    validity: &HashMap<String, HistoricalValidity>,
) -> HashSet<String> {
    let pending = commits
        .keys()
        .filter(|key| {
            validity
                .get(*key)
                .is_some_and(|state| state.state == HistoricalValidityState::Pending)
        })
        .cloned()
        .collect::<BTreeSet<_>>();
    let candidates = pending;
    let mut adjacency = BTreeMap::<String, Vec<String>>::new();
    for key in &candidates {
        let mut dependencies = commits[key]
            .basis_clock
            .iter()
            .map(exact_ref_key)
            .filter(|reference| candidates.contains(reference))
            .collect::<Vec<_>>();
        dependencies.sort();
        adjacency.insert(key.clone(), dependencies);
    }

    struct TarjanState {
        next_index: usize,
        indices: HashMap<String, usize>,
        lowlinks: HashMap<String, usize>,
        stack: Vec<String>,
        on_stack: HashSet<String>,
        cycles: HashSet<String>,
    }

    fn strong_connect(
        key: &str,
        adjacency: &BTreeMap<String, Vec<String>>,
        state: &mut TarjanState,
    ) {
        let index = state.next_index;
        state.next_index += 1;
        state.indices.insert(key.to_string(), index);
        state.lowlinks.insert(key.to_string(), index);
        state.stack.push(key.to_string());
        state.on_stack.insert(key.to_string());

        for dependency in &adjacency[key] {
            if !state.indices.contains_key(dependency) {
                strong_connect(dependency, adjacency, state);
                let dependency_lowlink = state.lowlinks[dependency];
                state
                    .lowlinks
                    .entry(key.to_string())
                    .and_modify(|lowlink| *lowlink = (*lowlink).min(dependency_lowlink));
            } else if state.on_stack.contains(dependency) {
                let dependency_index = state.indices[dependency];
                state
                    .lowlinks
                    .entry(key.to_string())
                    .and_modify(|lowlink| *lowlink = (*lowlink).min(dependency_index));
            }
        }

        if state.lowlinks[key] == state.indices[key] {
            let mut component = Vec::new();
            loop {
                let member = state.stack.pop().unwrap();
                state.on_stack.remove(&member);
                let complete = member == key;
                component.push(member);
                if complete {
                    break;
                }
            }
            if component.len() > 1 || adjacency[key].iter().any(|dependency| dependency == key) {
                state.cycles.extend(component);
            }
        }
    }

    let mut state = TarjanState {
        next_index: 0,
        indices: HashMap::new(),
        lowlinks: HashMap::new(),
        stack: Vec::new(),
        on_stack: HashSet::new(),
        cycles: HashSet::new(),
    };
    for key in &candidates {
        if !state.indices.contains_key(key) {
            strong_connect(key, &adjacency, &mut state);
        }
    }
    state.cycles
}

fn unsafe_commit_keys(
    forks: &[WriterForkV1],
    verified_commits: &HashMap<String, CommitV1>,
) -> HashSet<String> {
    let mut unsafe_keys = forks
        .iter()
        .flat_map(|fork| fork.alternatives.iter().map(exact_ref_key))
        .collect::<HashSet<_>>();
    loop {
        let additions = verified_commits
            .iter()
            .filter(|(key, commit)| {
                !unsafe_keys.contains(*key)
                    && commit
                        .basis_clock
                        .iter()
                        .any(|reference| unsafe_keys.contains(&exact_ref_key(reference)))
            })
            .map(|(key, _)| key.clone())
            .collect::<Vec<_>>();
        if additions.is_empty() {
            return unsafe_keys;
        }
        unsafe_keys.extend(additions);
    }
}

pub fn causally_covers_v1(
    covering: &CommitRef,
    covered: &CommitRef,
    verified_commits: &HashMap<String, CommitV1>,
) -> bool {
    if same_ref(covering, covered) {
        return true;
    }
    verified_commits
        .get(&exact_ref_key(covering))
        .is_some_and(|commit| {
            ancestor_ref_keys(&commit.basis_clock, verified_commits)
                .contains(&exact_ref_key(covered))
        })
}

pub fn compute_entity_frontier_v1(
    versions: &[EntityVersionV1],
    verified_commits: &HashMap<String, CommitV1>,
) -> Vec<EntityVersionV1> {
    let mut frontier = versions
        .iter()
        .filter(|candidate| {
            !versions.iter().any(|other| {
                !same_ref(&candidate.commit_ref, &other.commit_ref)
                    && causally_covers_v1(
                        &other.commit_ref,
                        &candidate.commit_ref,
                        verified_commits,
                    )
            })
        })
        .cloned()
        .collect::<Vec<_>>();
    frontier.sort_by(|a, b| compare_commit_ref_v1(&a.commit_ref, &b.commit_ref));
    frontier
}

pub fn expected_entity_frontier_v1(
    entity_key: &Value,
    author_verified_basis: &[CommitRef],
    versions: &[EntityVersionV1],
    verified_commits: &HashMap<String, CommitV1>,
) -> Vec<EntityVersionV1> {
    let visible = ancestor_ref_keys(author_verified_basis, verified_commits);
    let candidates = versions
        .iter()
        .filter(|version| {
            compare_entity_key_v1(&version.entity_key, entity_key).is_eq()
                && visible.contains(&exact_ref_key(&version.commit_ref))
        })
        .cloned()
        .collect::<Vec<_>>();
    compute_entity_frontier_v1(&candidates, verified_commits)
}

fn metadata_of(version: &EntityVersionV1) -> Value {
    let mut metadata = Map::new();
    if let Some(map) = version.full_value.as_object() {
        for field in [
            "id",
            "createdAt",
            "updatedAt",
            "rev",
            "revActor",
            "deletedAt",
        ] {
            if let Some(value) = map.get(field) {
                metadata.insert(field.to_string(), value.clone());
            }
        }
    }
    Value::Object(metadata)
}

fn resolved(semantic_state: Value, frontier: &[EntityVersionV1]) -> MaterializedEntityV1 {
    let mut ordered = frontier.to_vec();
    ordered.sort_by(|a, b| compare_commit_ref_v1(&a.commit_ref, &b.commit_ref));
    let business_value =
        (semantic_state["state"] == "live").then(|| semantic_state["value"].clone());
    MaterializedEntityV1::Resolved {
        semantic_state,
        business_value,
        provenance_frontier: ordered
            .iter()
            .map(|version| version.commit_ref.clone())
            .collect(),
        metadata_variants: ordered
            .iter()
            .map(|version| MetadataVariantV1 {
                commit_ref: version.commit_ref.clone(),
                metadata: metadata_of(version),
            })
            .collect(),
    }
}

fn same_semantic_state(frontier: &[EntityVersionV1]) -> Result<bool> {
    let Some(first) = frontier.first() else {
        return Ok(true);
    };
    let first = jcs_bytes(&first.semantic_state)?;
    for version in &frontier[1..] {
        if jcs_bytes(&version.semantic_state)? != first {
            return Ok(false);
        }
    }
    Ok(true)
}

fn changed_fields_overlap(frontier: &[EntityVersionV1]) -> bool {
    let mut seen = BTreeSet::new();
    frontier.iter().any(|version| {
        version
            .changed_fields
            .iter()
            .any(|field| !seen.insert(field.as_str()))
    })
}

fn has_locked_concurrency(frontier: &[EntityVersionV1]) -> bool {
    if frontier
        .first()
        .and_then(|version| version.entity_key.as_array())
        .and_then(|key| key.first())
        .and_then(Value::as_str)
        != Some("record")
    {
        return false;
    }
    frontier.iter().enumerate().any(|(index, version)| {
        version.semantic_state["state"] == "live"
            && version
                .changed_fields
                .iter()
                .any(|field| field == "isLocked")
            && version.semantic_state["value"]["isLocked"] == true
            && frontier.iter().enumerate().any(|(other_index, other)| {
                other_index != index && !other.changed_fields.is_empty()
            })
    })
}

fn conflict(
    entity_key: &Value,
    kind: EntityConflictKind,
    frontier: &[EntityVersionV1],
) -> Result<MaterializedEntityV1> {
    let core = build_entity_conflict_core_v1(
        entity_key.clone(),
        kind,
        frontier
            .iter()
            .map(|version| EntityConflictAlternativeInput {
                commit_ref: version.commit_ref.clone(),
                semantic_state: version.semantic_state.clone(),
                changed_fields: version.changed_fields.clone(),
                base_frontier: version.base_frontier.clone(),
            })
            .collect(),
    )?;
    Ok(MaterializedEntityV1::Conflict {
        conflict_id: entity_conflict_id_v1(&core)?,
        conflict_kind: kind.as_str().to_string(),
        entity_key: entity_key.clone(),
        frontier: sorted_refs(
            &frontier
                .iter()
                .map(|version| version.commit_ref.clone())
                .collect::<Vec<_>>(),
        ),
        conflict_fields: core.conflict_fields,
        semantic_alternatives: core.semantic_alternatives,
    })
}

fn validate_derived_live(
    entity_key: &Value,
    value: &Value,
    metadata_source: &EntityVersionV1,
) -> bool {
    let mut full = metadata_source
        .full_value
        .as_object()
        .cloned()
        .unwrap_or_default();
    let Some(business) = value.as_object() else {
        return false;
    };
    let business_fields = business_field_order(
        entity_key
            .as_array()
            .and_then(|key| key.first())
            .and_then(Value::as_str)
            .unwrap_or_default(),
    )
    .unwrap_or_default();
    for field in business_fields {
        full.remove(*field);
    }
    for (field, item) in business {
        full.insert(field.clone(), item.clone());
    }
    validate_native_entity(entity_key, &Value::Object(full)).is_ok()
}

pub fn materialize_v1(
    entity_key: &Value,
    exact_expected_frontier: &[EntityVersionV1],
    verified_closure: &HashMap<String, CommitV1>,
    all_versions: &[EntityVersionV1],
) -> Result<MaterializedEntityV1> {
    let frontier = compute_entity_frontier_v1(exact_expected_frontier, verified_closure);
    if frontier.is_empty() {
        return Ok(MaterializedEntityV1::Absent);
    }
    if same_semantic_state(&frontier)? || frontier.len() == 1 {
        return Ok(resolved(frontier[0].semantic_state.clone(), &frontier));
    }
    if frontier
        .iter()
        .any(|version| version.operation == "tombstone")
    {
        return conflict(entity_key, EntityConflictKind::LiveTombstone, &frontier);
    }
    let same_base = frontier
        .iter()
        .all(|version| same_ref_set(&version.base_frontier, &frontier[0].base_frontier));
    if has_locked_concurrency(&frontier) {
        return conflict(entity_key, EntityConflictKind::LockedConcurrent, &frontier);
    }
    if !same_base {
        return conflict(entity_key, EntityConflictKind::DifferentBase, &frontier);
    }
    if changed_fields_overlap(&frontier) {
        return conflict(entity_key, EntityConflictKind::OverlappingField, &frontier);
    }
    let base_versions = frontier[0]
        .base_frontier
        .iter()
        .filter_map(|reference| {
            all_versions.iter().find(|version| {
                compare_entity_key_v1(&version.entity_key, entity_key).is_eq()
                    && same_ref(&version.commit_ref, reference)
            })
        })
        .cloned()
        .collect::<Vec<_>>();
    let base = materialize_v1(entity_key, &base_versions, verified_closure, all_versions)?;
    let MaterializedEntityV1::Resolved {
        semantic_state: base_state,
        ..
    } = base
    else {
        return conflict(entity_key, EntityConflictKind::DifferentBase, &frontier);
    };
    if base_state["state"] != "live" {
        return conflict(entity_key, EntityConflictKind::DifferentBase, &frontier);
    }
    let mut merged = base_state["value"]
        .as_object()
        .cloned()
        .ok_or(ProtocolError("invalid_semantic_state"))?;
    for version in &frontier {
        for field in &version.changed_fields {
            merged.insert(
                field.clone(),
                version.semantic_state["value"][field].clone(),
            );
        }
    }
    let merged = Value::Object(merged);
    if !validate_derived_live(entity_key, &merged, &frontier[0]) {
        return conflict(entity_key, EntityConflictKind::DerivedDomain, &frontier);
    }
    Ok(resolved(
        json!({"state": "live", "value": merged}),
        &frontier,
    ))
}

fn materialize_visible_entities(
    versions: &[EntityVersionV1],
    commits: &HashMap<String, CommitV1>,
) -> Result<Vec<MaterializedEntryV1>> {
    let mut keys = BTreeMap::<Vec<u8>, Value>::new();
    for version in versions {
        keys.insert(
            entity_key_id(&version.entity_key)?,
            version.entity_key.clone(),
        );
    }
    keys.into_values()
        .map(|entity_key| {
            let candidates = versions
                .iter()
                .filter(|version| compare_entity_key_v1(&version.entity_key, &entity_key).is_eq())
                .cloned()
                .collect::<Vec<_>>();
            let frontier = compute_entity_frontier_v1(&candidates, commits);
            Ok(MaterializedEntryV1 {
                value: materialize_v1(&entity_key, &frontier, commits, versions)?,
                entity_key,
            })
        })
        .collect()
}

fn find_materialized<'a>(
    values: &'a [MaterializedEntryV1],
    key: &Value,
) -> Option<&'a MaterializedEntityV1> {
    values
        .iter()
        .find(|item| compare_entity_key_v1(&item.entity_key, key).is_eq())
        .map(|item| &item.value)
}

fn provenance(value: &MaterializedEntityV1) -> Option<&[CommitRef]> {
    match value {
        MaterializedEntityV1::Resolved {
            provenance_frontier,
            ..
        } => Some(provenance_frontier),
        _ => None,
    }
}

fn is_conflict(value: Option<&MaterializedEntityV1>) -> bool {
    matches!(value, Some(MaterializedEntityV1::Conflict { .. }))
}

fn semantic_state(value: Option<&MaterializedEntityV1>) -> Option<&Value> {
    match value {
        Some(MaterializedEntityV1::Resolved { semantic_state, .. }) => Some(semantic_state),
        _ => None,
    }
}

fn relation_participant(key: Value, value: &MaterializedEntityV1) -> Result<RelationParticipant> {
    Ok(RelationParticipant {
        entity_key: key,
        provenance_frontier: provenance(value)
            .ok_or(ProtocolError("invalid_relation_participant"))?
            .to_vec(),
    })
}

fn add_relation(
    conflicts: &mut Vec<RelationConflictV1>,
    kind: RelationConflictKind,
    facts: Value,
    participants: Vec<RelationParticipant>,
) -> Result<()> {
    let core = build_relation_conflict_core_v1(kind, facts, participants)?;
    conflicts.push(RelationConflictV1 {
        relation_conflict_id: relation_conflict_id_v1(&core)?,
        core,
    });
    Ok(())
}

pub fn detect_watch_tracker_relations_v1(
    entities: &[MaterializedEntryV1],
) -> Result<RelationDetectionV1> {
    let mut conflicts = Vec::new();
    let mut blocked = BTreeMap::<Vec<u8>, Value>::new();
    for item in entities {
        let Some(key) = item.entity_key.as_array() else {
            continue;
        };
        match key.first().and_then(Value::as_str) {
            Some("collection-member") => {
                let collection_key = json!(["collection", key[1]]);
                let record_key = json!(["record", key[2]]);
                let collection = find_materialized(entities, &collection_key);
                let record = find_materialized(entities, &record_key);
                if is_conflict(Some(&item.value)) || is_conflict(collection) {
                    blocked.insert(entity_key_id(&item.entity_key)?, item.entity_key.clone());
                    blocked.insert(entity_key_id(&collection_key)?, collection_key.clone());
                }
                if is_conflict(Some(&item.value)) || is_conflict(record) {
                    blocked.insert(entity_key_id(&item.entity_key)?, item.entity_key.clone());
                    blocked.insert(entity_key_id(&record_key)?, record_key.clone());
                }
                if semantic_state(Some(&item.value)).is_some_and(|state| state["state"] == "live")
                    && semantic_state(collection).is_some_and(|state| state["state"] == "tombstone")
                {
                    add_relation(
                        &mut conflicts,
                        RelationConflictKind::CollectionDeletedMemberLive,
                        json!({
                            "collectionId": key[1], "recordId": key[2],
                            "collectionState": "tombstone", "memberState": "live"
                        }),
                        vec![
                            relation_participant(collection_key, collection.unwrap())?,
                            relation_participant(item.entity_key.clone(), &item.value)?,
                        ],
                    )?;
                }
                if semantic_state(Some(&item.value)).is_some_and(|state| state["state"] == "live")
                    && semantic_state(record).is_some_and(|state| state["state"] == "tombstone")
                {
                    add_relation(
                        &mut conflicts,
                        RelationConflictKind::RecordDeletedMemberLive,
                        json!({
                            "collectionId": key[1], "recordId": key[2],
                            "recordState": "tombstone", "memberState": "live"
                        }),
                        vec![
                            relation_participant(record_key, record.unwrap())?,
                            relation_participant(item.entity_key.clone(), &item.value)?,
                        ],
                    )?;
                }
            }
            Some("episode-completion") => {
                let record_key = json!(["record", key[1]]);
                let record = find_materialized(entities, &record_key);
                if is_conflict(Some(&item.value)) || is_conflict(record) {
                    blocked.insert(entity_key_id(&item.entity_key)?, item.entity_key.clone());
                    blocked.insert(entity_key_id(&record_key)?, record_key.clone());
                    continue;
                }
                if semantic_state(Some(&item.value)).is_some_and(|state| state["state"] == "live")
                    && semantic_state(record).is_some_and(|state| state["state"] == "tombstone")
                {
                    let episode = validate_safe_integer(&key[2], 1, i32::MAX as i64)?;
                    add_relation(
                        &mut conflicts,
                        RelationConflictKind::RecordDeletedEpisodeLive,
                        json!({
                            "recordId": key[1], "episodeNumber": episode,
                            "recordState": "tombstone", "episodeState": "live"
                        }),
                        vec![
                            relation_participant(record_key, record.unwrap())?,
                            relation_participant(item.entity_key.clone(), &item.value)?,
                        ],
                    )?;
                } else if semantic_state(Some(&item.value))
                    .is_some_and(|state| state["state"] == "live")
                {
                    if let Some(state) =
                        semantic_state(record).filter(|state| state["state"] == "live")
                    {
                        if let (Ok(episode), Ok(total)) = (
                            validate_safe_integer(&key[2], 1, i32::MAX as i64),
                            validate_safe_integer(
                                &state["value"]["totalEpisodes"],
                                1,
                                i32::MAX as i64,
                            ),
                        ) {
                            if episode > total {
                                add_relation(
                                    &mut conflicts,
                                    RelationConflictKind::EpisodeExceedsTotal,
                                    json!({
                                        "recordId": key[1], "episodeNumber": episode,
                                        "totalEpisodes": total
                                    }),
                                    vec![
                                        relation_participant(record_key, record.unwrap())?,
                                        relation_participant(item.entity_key.clone(), &item.value)?,
                                    ],
                                )?;
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }
    conflicts.sort_by(|a, b| a.relation_conflict_id.cmp(&b.relation_conflict_id));
    Ok(RelationDetectionV1 {
        conflicts,
        blocked_by_entity_conflict: blocked.into_values().collect(),
    })
}

fn resolved_business(value: &MaterializedEntityV1) -> Option<&Value> {
    match value {
        MaterializedEntityV1::Resolved {
            business_value: Some(value),
            ..
        } => Some(value),
        _ => None,
    }
}

pub fn detect_duplicate_diagnostics_v1(
    entities: &[MaterializedEntryV1],
) -> Result<Vec<DuplicateDiagnosticV1>> {
    let mut groups = BTreeMap::<(String, Vec<u8>), (Value, Vec<Value>)>::new();
    let mut add = |kind: &str, value: Value, key: Value| -> Result<()> {
        groups
            .entry((kind.to_string(), jcs_bytes(&value)?))
            .or_insert_with(|| (value, Vec::new()))
            .1
            .push(key);
        Ok(())
    };
    for item in entities {
        let Some(business) = resolved_business(&item.value) else {
            continue;
        };
        match item.entity_key[0].as_str() {
            Some("collection") => {
                add(
                    "duplicate-collection-normalized-name",
                    business["normalizedName"].clone(),
                    item.entity_key.clone(),
                )?;
                if business["sourceKind"] != "manual" {
                    add(
                        "duplicate-collection-source",
                        json!([business["sourceKind"], business["sourceKey"]]),
                        item.entity_key.clone(),
                    )?;
                }
            }
            Some("record") => {
                if !business["imdbId"].is_null() {
                    add(
                        "duplicate-record-external-identity",
                        Value::String(format!("imdb:{}", business["imdbId"].as_str().unwrap())),
                        item.entity_key.clone(),
                    )?;
                }
                if !business["tmdbId"].is_null() {
                    add(
                        "duplicate-record-external-identity",
                        json!([
                            "tmdb",
                            business["tmdbMediaKind"],
                            business["tmdbId"],
                            business["tmdbParentId"],
                            business["tmdbSeasonNumber"]
                        ]),
                        item.entity_key.clone(),
                    )?;
                }
            }
            _ => {}
        }
    }
    groups
        .into_iter()
        .filter(|(_, (_, keys))| keys.len() > 1)
        .map(|((kind, _), (value, mut keys))| {
            keys.sort_by(compare_entity_key_v1);
            Ok(DuplicateDiagnosticV1 {
                kind,
                value,
                entity_keys: keys,
            })
        })
        .collect()
}

fn expected_changed_fields(
    mutation: &CommitMutationV1,
    base: &MaterializedEntityV1,
    resolution: bool,
) -> Result<Vec<String>> {
    if mutation.operation == "tombstone" {
        return Ok(vec!["$tombstone".to_string()]);
    }
    let next = canonical_semantic_value(&mutation.value)?;
    let entity_type = mutation.entity_key[0]
        .as_str()
        .ok_or(ProtocolError("invalid_entity_key"))?;
    let order = business_field_order(entity_type).ok_or(ProtocolError("invalid_entity_type"))?;
    if let MaterializedEntityV1::Resolved { semantic_state, .. } = base {
        if semantic_state["state"] == "live" {
            let mut changed = Vec::new();
            for field in order {
                if jcs_bytes(&semantic_state["value"][field])? != jcs_bytes(&next[field])? {
                    changed.push((*field).to_string());
                }
            }
            return Ok(changed);
        }
    }
    if matches!(base, MaterializedEntityV1::Conflict { .. }) && !resolution {
        return Err(ProtocolError(
            "ordinary_mutation_blocked_by_entity_conflict",
        ));
    }
    Ok(order.iter().map(|field| (*field).to_string()).collect())
}

fn assert_canonical_changed_fields(
    mutation: &CommitMutationV1,
    expected: &[String],
    resolution: bool,
) -> Result<()> {
    if mutation.changed_fields != expected {
        return Err(ProtocolError("invalid_changed_fields"));
    }
    if mutation.operation == "upsert" && expected.is_empty() && !resolution {
        return Err(ProtocolError("metadata_only_mutation"));
    }
    Ok(())
}

fn create_version(commit: &CommitV1, mutation: &CommitMutationV1) -> Result<EntityVersionV1> {
    let entity_type = mutation.entity_key[0]
        .as_str()
        .ok_or(ProtocolError("invalid_entity_key"))?;
    let semantic = if mutation.operation == "upsert" {
        validate_native_entity(&mutation.entity_key, &mutation.value)?;
        Some(canonical_semantic_value(&mutation.value)?)
    } else {
        validate_tombstone(entity_type, &mutation.entity_key, &mutation.value)?;
        None
    };
    let semantic_state = semantic.as_ref().map_or_else(
        || json!({"state": "tombstone"}),
        |value| json!({"state": "live", "value": value}),
    );
    Ok(EntityVersionV1 {
        entity_key: mutation.entity_key.clone(),
        operation: mutation.operation.clone(),
        full_value: mutation.value.clone(),
        semantic_state,
        canonical_semantic_value: semantic,
        changed_fields: mutation.changed_fields.clone(),
        base_frontier: mutation.base_frontier.clone(),
        commit_ref: commit.commit_ref(),
        commit_dot: commit.commit_ref().dot(),
        causal_basis: commit.basis_clock.clone(),
    })
}

fn is_resolved_live(value: Option<&MaterializedEntityV1>) -> Option<&Value> {
    match value {
        Some(MaterializedEntityV1::Resolved {
            business_value: Some(value),
            semantic_state,
            ..
        }) if semantic_state["state"] == "live" => Some(value),
        _ => None,
    }
}

fn record_supports_episode(value: &Value, episode: i64) -> bool {
    value["mediaType"] != "电影"
        && validate_safe_integer(&value["totalEpisodes"], 1, i32::MAX as i64)
            .ok()
            .is_some_and(|total| episode <= total)
}

fn validate_author_references(
    candidates: &[EntityVersionV1],
    basis_materialized: &[MaterializedEntryV1],
) -> Result<()> {
    let mut batch = BTreeMap::<Vec<u8>, &EntityVersionV1>::new();
    for version in candidates {
        batch.insert(entity_key_id(&version.entity_key)?, version);
    }
    let live_value = |key: &Value| -> Option<&Value> {
        if let Ok(id) = entity_key_id(key) {
            if let Some(version) = batch.get(&id) {
                if version.semantic_state["state"] == "live" {
                    return version.canonical_semantic_value.as_ref();
                }
                return None;
            }
        }
        is_resolved_live(find_materialized(basis_materialized, key))
    };
    for version in candidates {
        if version.operation != "upsert" {
            continue;
        }
        let key = version
            .entity_key
            .as_array()
            .ok_or(ProtocolError("invalid_entity_key"))?;
        match key[0].as_str() {
            Some("episode-completion") => {
                let episode = validate_safe_integer(&key[2], 1, i32::MAX as i64)
                    .map_err(|_| ProtocolError("invalid_entity_key"))?;
                let parent_key = json!(["record", key[1]]);
                if live_value(&parent_key)
                    .map_or(true, |parent| !record_supports_episode(parent, episode))
                {
                    return Err(ProtocolError("invalid_episode_parent_basis"));
                }
            }
            Some("collection-member")
                if live_value(&json!(["collection", key[1]])).is_none()
                    || live_value(&json!(["record", key[2]])).is_none() =>
            {
                return Err(ProtocolError("invalid_member_parent_basis"));
            }
            _ => {}
        }
    }
    Ok(())
}

fn relation_contains_key(relation: &RelationConflictV1, key: &Value) -> bool {
    relation
        .core
        .entity_keys
        .iter()
        .any(|candidate| compare_entity_key_v1(candidate, key).is_eq())
}

fn same_relation_identity(a: &RelationConflictV1, b: &RelationConflictV1) -> bool {
    a.core.relation_kind == b.core.relation_kind
        && a.core.entity_keys.len() == b.core.entity_keys.len()
        && a.core
            .entity_keys
            .iter()
            .zip(&b.core.entity_keys)
            .all(|(left, right)| compare_entity_key_v1(left, right).is_eq())
}

pub fn validate_entity_resolution_v1(
    commit: &CommitV1,
    basis_materialized: &[MaterializedEntryV1],
    candidates: &[EntityVersionV1],
) -> Result<Vec<Value>> {
    if commit.commit_kind != "resolution" || commit.source.r#type != "manual-resolution" {
        return Err(ProtocolError("invalid_resolution_commit"));
    }
    let mut participants = BTreeMap::<Vec<u8>, Value>::new();
    for candidate in candidates {
        let Some(MaterializedEntityV1::Conflict { conflict_id, .. }) =
            find_materialized(basis_materialized, &candidate.entity_key)
        else {
            continue;
        };
        if !commit.resolves.contains(conflict_id) {
            return Err(ProtocolError("incomplete_entity_resolution"));
        }
        participants.insert(
            entity_key_id(&candidate.entity_key)?,
            candidate.entity_key.clone(),
        );
    }
    Ok(participants.into_values().collect())
}

pub fn validate_relation_resolution_v1(
    commit: &CommitV1,
    basis_relations: &RelationDetectionV1,
    candidates: &[EntityVersionV1],
    after_relations: &RelationDetectionV1,
) -> Result<Vec<Value>> {
    if commit.commit_kind != "resolution" || commit.source.r#type != "manual-resolution" {
        return Err(ProtocolError("invalid_resolution_commit"));
    }
    let targeted = basis_relations
        .conflicts
        .iter()
        .filter(|relation| commit.resolves.contains(&relation.relation_conflict_id))
        .collect::<Vec<_>>();
    let post_ids = after_relations
        .conflicts
        .iter()
        .map(|relation| relation.relation_conflict_id.as_str())
        .collect::<BTreeSet<_>>();
    if basis_relations.conflicts.iter().any(|relation| {
        !post_ids.contains(relation.relation_conflict_id.as_str())
            && !commit.resolves.contains(&relation.relation_conflict_id)
    }) {
        return Err(ProtocolError("incomplete_relation_resolution"));
    }
    let mut participants = BTreeMap::<Vec<u8>, Value>::new();
    for relation in &targeted {
        for key in &relation.core.entity_keys {
            if !candidates
                .iter()
                .any(|version| compare_entity_key_v1(&version.entity_key, key).is_eq())
            {
                return Err(ProtocolError("incomplete_relation_resolution"));
            }
            participants.insert(entity_key_id(key)?, key.clone());
        }
    }
    if after_relations.conflicts.iter().any(|after| {
        targeted
            .iter()
            .any(|target| same_relation_identity(after, target))
    }) {
        return Err(ProtocolError("incomplete_relation_resolution"));
    }
    Ok(participants.into_values().collect())
}

fn historical_commit(
    commit: &CommitV1,
    verified_commits: &HashMap<String, CommitV1>,
    prior_versions: &[EntityVersionV1],
) -> Result<Vec<EntityVersionV1>> {
    let is_explicit_resolution = commit.commit_kind == "resolution";
    validate_writer_chain_link_v1(commit, verified_commits)?;
    let visible_keys = ancestor_ref_keys(&commit.basis_clock, verified_commits);
    let basis_versions = prior_versions
        .iter()
        .filter(|version| visible_keys.contains(&exact_ref_key(&version.commit_ref)))
        .cloned()
        .collect::<Vec<_>>();
    let basis_materialized = materialize_visible_entities(&basis_versions, verified_commits)?;
    let relations = detect_watch_tracker_relations_v1(&basis_materialized)?;
    let mut candidates = Vec::new();
    let mut entity_conflict_ids = BTreeSet::new();
    let mut resolution_participant_keys = BTreeMap::<Vec<u8>, Value>::new();
    for mutation in &commit.mutations {
        let expected = expected_entity_frontier_v1(
            &mutation.entity_key,
            &commit.basis_clock,
            prior_versions,
            verified_commits,
        );
        if !same_ref_set(
            &mutation.base_frontier,
            &expected
                .iter()
                .map(|version| version.commit_ref.clone())
                .collect::<Vec<_>>(),
        ) {
            return Err(ProtocolError("invalid_entity_base_frontier"));
        }
        let absent = MaterializedEntityV1::Absent;
        let base = find_materialized(&basis_materialized, &mutation.entity_key).unwrap_or(&absent);
        if !is_explicit_resolution && matches!(base, MaterializedEntityV1::Conflict { .. }) {
            return Err(ProtocolError(
                "ordinary_mutation_blocked_by_entity_conflict",
            ));
        }
        if !is_explicit_resolution
            && relations
                .conflicts
                .iter()
                .any(|relation| relation_contains_key(relation, &mutation.entity_key))
        {
            return Err(ProtocolError(
                "ordinary_mutation_blocked_by_relation_conflict",
            ));
        }
        if let MaterializedEntityV1::Conflict { conflict_id, .. } = base {
            entity_conflict_ids.insert(conflict_id.clone());
            resolution_participant_keys.insert(
                entity_key_id(&mutation.entity_key)?,
                mutation.entity_key.clone(),
            );
        }
        let expected_fields = expected_changed_fields(mutation, base, is_explicit_resolution)?;
        assert_canonical_changed_fields(mutation, &expected_fields, is_explicit_resolution)?;
        let candidate = create_version(commit, mutation)?;
        if !is_explicit_resolution
            && mutation.entity_key[0] == "record"
            && is_resolved_live(Some(base)).is_some_and(|value| value["isLocked"] == true)
            && (candidate.operation != "upsert"
                || candidate.changed_fields != ["isLocked"]
                || candidate.semantic_state["value"]["isLocked"] != false)
        {
            return Err(ProtocolError("ordinary_mutation_blocked_by_lock"));
        }
        candidates.push(candidate);
    }
    validate_author_references(&candidates, &basis_materialized)?;

    if commit.commit_kind == "resolution" {
        let relation_ids = relations
            .conflicts
            .iter()
            .map(|relation| relation.relation_conflict_id.clone())
            .collect::<BTreeSet<_>>();
        let targets = entity_conflict_ids
            .union(&relation_ids)
            .cloned()
            .collect::<BTreeSet<_>>();
        if commit.resolves.iter().any(|id| !targets.contains(id)) {
            return Err(ProtocolError("stale_resolution"));
        }
        let mut simulated_commits = verified_commits.clone();
        simulated_commits.insert(exact_ref_key(&commit.commit_ref()), commit.clone());
        // Historical validity is author-basis relative. Later concurrent versions remain
        // alternatives in global replay and cannot retroactively invalidate this resolution.
        let mut simulated_versions = basis_versions.clone();
        simulated_versions.extend(candidates.clone());
        let after_materialized =
            materialize_visible_entities(&simulated_versions, &simulated_commits)?;
        let after_relations = detect_watch_tracker_relations_v1(&after_materialized)?;
        for key in validate_entity_resolution_v1(commit, &basis_materialized, &candidates)? {
            resolution_participant_keys.insert(entity_key_id(&key)?, key);
        }
        for key in
            validate_relation_resolution_v1(commit, &relations, &candidates, &after_relations)?
        {
            resolution_participant_keys.insert(entity_key_id(&key)?, key);
        }
        if candidates.iter().any(|version| {
            entity_key_id(&version.entity_key)
                .map_or(true, |key| !resolution_participant_keys.contains_key(&key))
        }) {
            return Err(ProtocolError("invalid_resolution_composition"));
        }
    }
    Ok(candidates)
}

pub fn replay_verified_history_v1(commits: &[CommitV1]) -> Result<VerifiedReplayV1> {
    let mut validity = HashMap::<String, HistoricalValidity>::new();
    let mut structural = HashMap::<String, CommitV1>::new();
    let mut input_groups = BTreeMap::<String, Vec<&CommitV1>>::new();
    for commit in commits {
        input_groups
            .entry(exact_ref_key(&commit.commit_ref()))
            .or_default()
            .push(commit);
    }
    for (key, group) in &input_groups {
        let commit = group[0];
        let representations = group
            .iter()
            .map(|candidate| jcs_bytes(*candidate))
            .collect::<Result<BTreeSet<_>>>()?;
        if representations.len() != 1 {
            validity.insert(
                key.clone(),
                HistoricalValidity {
                    state: HistoricalValidityState::Invalid,
                    error: Some("duplicate_commit_ref".to_string()),
                },
            );
            continue;
        }
        match validate_commit_envelope_v1(commit) {
            Ok(()) => {
                structural.insert(key.clone(), commit.clone());
                validity.insert(
                    key.clone(),
                    HistoricalValidity {
                        state: HistoricalValidityState::Pending,
                        error: None,
                    },
                );
            }
            Err(error) => {
                validity.insert(
                    key.clone(),
                    HistoricalValidity {
                        state: HistoricalValidityState::Invalid,
                        error: Some(error.0.to_string()),
                    },
                );
            }
        }
    }
    let mut verified_commits = HashMap::<String, CommitV1>::new();
    let mut versions = Vec::<EntityVersionV1>::new();
    loop {
        let mut pending = structural
            .iter()
            .filter(|(key, _)| {
                validity
                    .get(*key)
                    .is_some_and(|status| status.state == HistoricalValidityState::Pending)
            })
            .map(|(key, commit)| (key.clone(), commit.clone()))
            .collect::<Vec<_>>();
        pending.sort_by(|a, b| a.0.cmp(&b.0));
        let mut progressed = false;
        for (key, commit) in pending {
            let dependencies = commit
                .basis_clock
                .iter()
                .map(exact_ref_key)
                .collect::<Vec<_>>();
            if dependencies.iter().any(|reference| {
                validity
                    .get(reference)
                    .is_some_and(|status| status.state == HistoricalValidityState::Invalid)
            }) {
                validity.insert(
                    key,
                    HistoricalValidity {
                        state: HistoricalValidityState::Invalid,
                        error: Some("invalid_causal_dependency".to_string()),
                    },
                );
                progressed = true;
                continue;
            }
            if dependencies
                .iter()
                .any(|reference| !validity.contains_key(reference))
            {
                continue;
            }
            if dependencies.iter().any(|reference| {
                validity.get(reference).map_or(true, |status| {
                    status.state != HistoricalValidityState::Valid
                })
            }) {
                continue;
            }
            match historical_commit(&commit, &verified_commits, &versions) {
                Ok(produced) => {
                    verified_commits.insert(key.clone(), commit);
                    versions.extend(produced);
                    validity.insert(
                        key,
                        HistoricalValidity {
                            state: HistoricalValidityState::Valid,
                            error: None,
                        },
                    );
                }
                Err(error) => {
                    validity.insert(
                        key,
                        HistoricalValidity {
                            state: HistoricalValidityState::Invalid,
                            error: Some(error.0.to_string()),
                        },
                    );
                }
            }
            progressed = true;
        }
        if !progressed {
            break;
        }
    }
    for key in pending_cycle_keys(&structural, &validity) {
        validity.insert(
            key,
            HistoricalValidity {
                state: HistoricalValidityState::Invalid,
                error: Some("causal_cycle".to_string()),
            },
        );
    }
    loop {
        let invalid_descendants = structural
            .iter()
            .filter(|(key, commit)| {
                validity
                    .get(*key)
                    .is_some_and(|state| state.state == HistoricalValidityState::Pending)
                    && commit.basis_clock.iter().any(|reference| {
                        validity
                            .get(&exact_ref_key(reference))
                            .is_some_and(|state| state.state == HistoricalValidityState::Invalid)
                    })
            })
            .map(|(key, _)| key.clone())
            .collect::<Vec<_>>();
        if invalid_descendants.is_empty() {
            break;
        }
        for key in invalid_descendants {
            validity.insert(
                key,
                HistoricalValidity {
                    state: HistoricalValidityState::Invalid,
                    error: Some("invalid_causal_dependency".to_string()),
                },
            );
        }
    }

    let forks = detect_writer_forks_v1(commits);
    let unsafe_keys = unsafe_commit_keys(&forks, &verified_commits);
    let safe_commits = verified_commits
        .iter()
        .filter(|(key, _)| !unsafe_keys.contains(*key))
        .map(|(key, commit)| (key.clone(), commit.clone()))
        .collect::<HashMap<_, _>>();
    versions.sort_by(|a, b| {
        compare_entity_key_v1(&a.entity_key, &b.entity_key)
            .then_with(|| compare_commit_ref_v1(&a.commit_ref, &b.commit_ref))
    });
    let forensic_versions = versions;
    let safe_versions = forensic_versions
        .iter()
        .filter(|version| !unsafe_keys.contains(&exact_ref_key(&version.commit_ref)))
        .cloned()
        .collect::<Vec<_>>();
    let materialized = materialize_visible_entities(&safe_versions, &safe_commits)?;
    let frontiers = materialized
        .iter()
        .map(|item| {
            let candidates = safe_versions
                .iter()
                .filter(|version| {
                    compare_entity_key_v1(&version.entity_key, &item.entity_key).is_eq()
                })
                .cloned()
                .collect::<Vec<_>>();
            FrontierEntryV1 {
                entity_key: item.entity_key.clone(),
                frontier: compute_entity_frontier_v1(&candidates, &safe_commits)
                    .into_iter()
                    .map(|version| version.commit_ref)
                    .collect(),
            }
        })
        .collect();
    let validity_output = input_groups
        .into_values()
        .map(|group| group[0].commit_ref())
        .map(|reference| ValidityEntryV1 {
            validity: validity.get(&exact_ref_key(&reference)).cloned().unwrap_or(
                HistoricalValidity {
                    state: HistoricalValidityState::Invalid,
                    error: Some("invalid_commit".to_string()),
                },
            ),
            commit_ref: reference,
        })
        .collect::<Vec<_>>();
    let mut unsafe_commit_refs = unsafe_keys
        .iter()
        .filter_map(|key| {
            verified_commits
                .get(key)
                .or_else(|| {
                    commits
                        .iter()
                        .find(|commit| exact_ref_key(&commit.commit_ref()) == *key)
                })
                .map(CommitV1::commit_ref)
        })
        .collect::<Vec<_>>();
    unsafe_commit_refs.sort_by(compare_commit_ref_v1);
    Ok(VerifiedReplayV1 {
        validity: validity_output,
        forks,
        unsafe_commit_refs,
        forensic_versions,
        versions: safe_versions,
        frontiers,
        relations: detect_watch_tracker_relations_v1(&materialized)?,
        duplicate_diagnostics: detect_duplicate_diagnostics_v1(&materialized)?,
        materialized,
    })
}
