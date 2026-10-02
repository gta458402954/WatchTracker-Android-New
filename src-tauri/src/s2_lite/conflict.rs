use super::canonical::{
    compare_commit_dot_v1, compare_commit_ref_v1, compare_entity_key_v1, jcs_bytes, sha256_jcs,
    validate_commit_ref, validate_entity_key, validate_safe_integer, ProtocolError, Result,
};
use super::semantic::business_field_order;
use super::types::{
    CommitRef, EntityConflictAlternativeInput, EntityConflictCore, EntityConflictKind,
    RelationConflictCore, RelationConflictKind, RelationParticipant, SemanticAlternative,
};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

fn entity_type(entity_key: &Value) -> Result<&str> {
    entity_key
        .as_array()
        .and_then(|items| items.first())
        .and_then(Value::as_str)
        .ok_or(ProtocolError("invalid_entity_key"))
}

fn live_field<'a>(state: &'a Value, field: &str) -> Option<&'a Value> {
    (state.get("state").and_then(Value::as_str) == Some("live"))
        .then(|| state.get("value")?.get(field))
        .flatten()
}

fn different_live_field(
    alternatives: &[EntityConflictAlternativeInput],
    field: &str,
) -> Result<bool> {
    let values = alternatives
        .iter()
        .filter_map(|alternative| live_field(&alternative.semantic_state, field))
        .map(jcs_bytes)
        .collect::<Result<Vec<_>>>()?;
    Ok(values.len() >= 2 && values.iter().skip(1).any(|value| value != &values[0]))
}

pub fn conflict_fields_v1(
    entity_key: &Value,
    kind: EntityConflictKind,
    alternatives: &[EntityConflictAlternativeInput],
) -> Result<Vec<String>> {
    match kind {
        EntityConflictKind::LiveTombstone => return Ok(vec!["$existence".into()]),
        EntityConflictKind::DifferentBase => return Ok(vec!["$base".into()]),
        EntityConflictKind::DerivedDomain => return Ok(vec!["$domain".into()]),
        _ => {}
    }
    let entity_type = entity_type(entity_key)?;
    let order = business_field_order(entity_type).ok_or(ProtocolError("invalid_entity_type"))?;
    if kind == EntityConflictKind::LockedConcurrent {
        if entity_type != "record" {
            return Err(ProtocolError("locked_conflict_requires_record"));
        }
        let mut fields = vec!["isLocked".to_string()];
        for field in order {
            if *field != "isLocked" && different_live_field(alternatives, field)? {
                fields.push((*field).to_string());
            }
        }
        return Ok(fields);
    }
    let mut counts = BTreeMap::<&str, usize>::new();
    for alternative in alternatives {
        let unique = alternative
            .changed_fields
            .iter()
            .map(String::as_str)
            .collect::<BTreeSet<_>>();
        for field in unique {
            *counts.entry(field).or_default() += 1;
        }
    }
    Ok(order
        .iter()
        .filter(|field| counts.get(**field).copied().unwrap_or_default() >= 2)
        .map(|field| (*field).to_string())
        .collect())
}

fn validate_semantic_state(entity_key: &Value, value: &Value) -> Result<()> {
    let map = value
        .as_object()
        .ok_or(ProtocolError("invalid_semantic_state"))?;
    match map.get("state").and_then(Value::as_str) {
        Some("tombstone") if map.len() == 1 => Ok(()),
        Some("live") if map.len() == 2 && map.get("value").is_some_and(Value::is_object) => {
            let entity_type = entity_type(entity_key)?;
            let expected = business_field_order(entity_type)
                .ok_or(ProtocolError("invalid_entity_type"))?
                .iter()
                .copied()
                .collect::<BTreeSet<_>>();
            let actual = map["value"]
                .as_object()
                .expect("checked live value")
                .keys()
                .map(String::as_str)
                .collect::<BTreeSet<_>>();
            if actual != expected {
                return Err(ProtocolError("invalid_semantic_state"));
            }
            jcs_bytes(value).map(|_| ())
        }
        _ => Err(ProtocolError("invalid_semantic_state")),
    }
}

fn expected_relation_entity_keys(kind: RelationConflictKind, facts: &Value) -> Vec<Value> {
    match kind {
        RelationConflictKind::CollectionDeletedMemberLive => vec![
            json!(["collection", facts["collectionId"]]),
            json!([
                "collection-member",
                facts["collectionId"],
                facts["recordId"]
            ]),
        ],
        RelationConflictKind::RecordDeletedMemberLive => vec![
            json!(["record", facts["recordId"]]),
            json!([
                "collection-member",
                facts["collectionId"],
                facts["recordId"]
            ]),
        ],
        RelationConflictKind::RecordDeletedEpisodeLive
        | RelationConflictKind::EpisodeExceedsTotal => vec![
            json!(["record", facts["recordId"]]),
            json!([
                "episode-completion",
                facts["recordId"],
                facts["episodeNumber"]
            ]),
        ],
    }
}

pub fn build_entity_conflict_core_v1(
    entity_key: Value,
    kind: EntityConflictKind,
    mut alternatives: Vec<EntityConflictAlternativeInput>,
) -> Result<EntityConflictCore> {
    validate_entity_key(&entity_key)?;
    if alternatives.len() < 2 {
        return Err(ProtocolError("conflict_requires_multiple_alternatives"));
    }
    for alternative in &alternatives {
        validate_commit_ref(&alternative.commit_ref)?;
        validate_semantic_state(&entity_key, &alternative.semantic_state)?;
        for base_ref in &alternative.base_frontier {
            validate_commit_ref(base_ref)?;
        }
    }
    alternatives.sort_by(|a, b| compare_commit_dot_v1(&a.commit_ref.dot(), &b.commit_ref.dot()));
    for pair in alternatives.windows(2) {
        if compare_commit_dot_v1(&pair[0].commit_ref.dot(), &pair[1].commit_ref.dot()).is_eq() {
            return Err(ProtocolError("duplicate_or_forked_dot"));
        }
    }
    let conflict_fields = conflict_fields_v1(&entity_key, kind, &alternatives)?;
    let frontier_dots = alternatives
        .iter()
        .map(|alternative| alternative.commit_ref.dot())
        .collect::<Vec<_>>();
    let semantic_alternatives = alternatives
        .into_iter()
        .enumerate()
        .map(|(index, alternative)| SemanticAlternative {
            dot: frontier_dots[index].clone(),
            semantic_state: alternative.semantic_state,
        })
        .collect();
    Ok(EntityConflictCore {
        domain: "watchtracker-s2-lite-entity-conflict-v1",
        entity_key,
        conflict_kind: kind.as_str(),
        frontier_dots,
        conflict_fields,
        semantic_alternatives,
    })
}

pub fn entity_conflict_id_v1(core: &EntityConflictCore) -> Result<String> {
    sha256_jcs(core)
}

fn exact_ref_key(reference: &CommitRef) -> String {
    format!(
        "{}\0{}\0{}\0{}",
        reference.writer_id, reference.writer_seq, reference.commit_id, reference.content_hash
    )
}

fn exact_fact_keys(facts: &Value, expected: &[&str]) -> Result<()> {
    let actual = facts
        .as_object()
        .ok_or(ProtocolError("invalid_relation_facts"))?
        .keys()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    if actual != expected.iter().copied().collect::<BTreeSet<_>>() {
        return Err(ProtocolError("invalid_relation_facts"));
    }
    Ok(())
}

fn validate_relation_facts(kind: RelationConflictKind, facts: &Value) -> Result<()> {
    match kind {
        RelationConflictKind::CollectionDeletedMemberLive => {
            exact_fact_keys(
                facts,
                &["collectionId", "recordId", "collectionState", "memberState"],
            )?;
            if facts["collectionState"] != "tombstone" || facts["memberState"] != "live" {
                return Err(ProtocolError("invalid_relation_facts"));
            }
        }
        RelationConflictKind::RecordDeletedMemberLive => {
            exact_fact_keys(
                facts,
                &["collectionId", "recordId", "recordState", "memberState"],
            )?;
            if facts["recordState"] != "tombstone" || facts["memberState"] != "live" {
                return Err(ProtocolError("invalid_relation_facts"));
            }
        }
        RelationConflictKind::RecordDeletedEpisodeLive => {
            exact_fact_keys(
                facts,
                &["recordId", "episodeNumber", "recordState", "episodeState"],
            )?;
            if facts["recordState"] != "tombstone" || facts["episodeState"] != "live" {
                return Err(ProtocolError("invalid_relation_facts"));
            }
            validate_safe_integer(&facts["episodeNumber"], 1, i32::MAX as i64)?;
        }
        RelationConflictKind::EpisodeExceedsTotal => {
            exact_fact_keys(facts, &["recordId", "episodeNumber", "totalEpisodes"])?;
            let episode = validate_safe_integer(&facts["episodeNumber"], 1, i32::MAX as i64)?;
            let total = validate_safe_integer(&facts["totalEpisodes"], 1, i32::MAX as i64)?;
            if episode <= total {
                return Err(ProtocolError("relation_not_conflicting"));
            }
        }
    }
    if !facts["recordId"].is_string()
        || (facts.get("collectionId").is_some() && !facts["collectionId"].is_string())
    {
        return Err(ProtocolError("invalid_relation_facts"));
    }
    Ok(())
}

pub fn build_relation_conflict_core_v1(
    kind: RelationConflictKind,
    semantic_relation_facts: Value,
    participants: Vec<RelationParticipant>,
) -> Result<RelationConflictCore> {
    if participants.len() != 2 {
        return Err(ProtocolError("invalid_relation_participants"));
    }
    for participant in &participants {
        validate_entity_key(&participant.entity_key)?;
    }
    validate_relation_facts(kind, &semantic_relation_facts)?;
    let mut entity_keys = participants
        .iter()
        .map(|participant| participant.entity_key.clone())
        .collect::<Vec<_>>();
    entity_keys.sort_by(compare_entity_key_v1);
    if compare_entity_key_v1(&entity_keys[0], &entity_keys[1]).is_eq() {
        return Err(ProtocolError("invalid_relation_participants"));
    }
    let mut expected_keys = expected_relation_entity_keys(kind, &semantic_relation_facts);
    expected_keys.sort_by(compare_entity_key_v1);
    if entity_keys
        .iter()
        .zip(&expected_keys)
        .any(|(actual, expected)| !compare_entity_key_v1(actual, expected).is_eq())
    {
        return Err(ProtocolError("invalid_relation_participants"));
    }
    let mut refs = BTreeMap::<String, CommitRef>::new();
    for participant in participants {
        if participant.provenance_frontier.is_empty() {
            return Err(ProtocolError("empty_relation_provenance"));
        }
        for reference in participant.provenance_frontier {
            validate_commit_ref(&reference)?;
            refs.insert(exact_ref_key(&reference), reference);
        }
    }
    let mut version_refs = refs.into_values().collect::<Vec<_>>();
    version_refs.sort_by(compare_commit_ref_v1);
    Ok(RelationConflictCore {
        domain: "watchtracker-s2-lite-relation-conflict-v1",
        relation_kind: kind.as_str(),
        entity_keys,
        version_refs,
        semantic_relation_facts,
    })
}

pub fn relation_conflict_id_v1(core: &RelationConflictCore) -> Result<String> {
    sha256_jcs(core)
}

pub fn canonical_tombstone_state() -> Value {
    json!({ "state": "tombstone" })
}
