use super::canonical::{compare_entity_key_v1, validate_entity_key, ProtocolError, Result};
use super::semantic::validate_native_entity;
use super::types::{BootstrapEntity, BootstrapPlan};
use std::collections::{BTreeMap, BTreeSet};

pub const MAX_BOOTSTRAP_MUTATIONS_PER_COMMIT: usize = 256;

fn rank(entity_type: &str) -> Result<u8> {
    match entity_type {
        "record" => Ok(0),
        "collection" => Ok(1),
        "episode-completion" => Ok(2),
        "collection-member" => Ok(3),
        _ => Err(ProtocolError("invalid_entity_type")),
    }
}

fn key_parts(entity: &BootstrapEntity) -> Result<&Vec<serde_json::Value>> {
    validate_entity_key(&entity.entity_key)?;
    let parts = entity
        .entity_key
        .as_array()
        .ok_or(ProtocolError("invalid_bootstrap_entity_key"))?;
    if parts.first().and_then(serde_json::Value::as_str) != Some(entity.entity_type.as_str()) {
        return Err(ProtocolError("bootstrap_entity_type_mismatch"));
    }
    Ok(parts)
}

fn chunks(entities: &[BootstrapEntity]) -> Vec<Vec<BootstrapEntity>> {
    entities
        .chunks(MAX_BOOTSTRAP_MUTATIONS_PER_COMMIT)
        .map(<[BootstrapEntity]>::to_vec)
        .collect()
}

pub fn build_bootstrap_plan_v1(input: &[BootstrapEntity]) -> Result<BootstrapPlan> {
    for entity in input {
        key_parts(entity)?;
        rank(&entity.entity_type)?;
        validate_native_entity(&entity.entity_key, &entity.value)?;
    }
    let mut ordered = input.to_vec();
    ordered.sort_by(|left, right| {
        rank(&left.entity_type)
            .expect("validated entity type")
            .cmp(&rank(&right.entity_type).expect("validated entity type"))
            .then_with(|| compare_entity_key_v1(&left.entity_key, &right.entity_key))
    });
    for pair in ordered.windows(2) {
        if compare_entity_key_v1(&pair[0].entity_key, &pair[1].entity_key).is_eq() {
            return Err(ProtocolError("duplicate_bootstrap_entity_key"));
        }
    }
    let records = input
        .iter()
        .filter(|entity| entity.entity_type == "record")
        .map(|entity| {
            (
                key_parts(entity).expect("validated key")[1]
                    .as_str()
                    .unwrap(),
                &entity.value,
            )
        })
        .collect::<BTreeMap<_, _>>();
    let collections = input
        .iter()
        .filter(|entity| entity.entity_type == "collection")
        .map(|entity| {
            key_parts(entity).expect("validated key")[1]
                .as_str()
                .unwrap()
        })
        .collect::<BTreeSet<_>>();
    for entity in input {
        let parts = key_parts(entity)?;
        if entity.entity_type == "episode-completion" {
            let Some(record) = records.get(parts[1].as_str().unwrap()) else {
                return Err(ProtocolError("invalid_bootstrap_dependency_graph"));
            };
            let record = record.as_object().expect("validated native Record value");
            let episode_number =
                super::canonical::validate_safe_integer(&parts[2], 1, i32::MAX as i64)?;
            let total_episodes = record["totalEpisodes"].as_f64().map(|value| value as i64);
            if record["mediaType"] == "电影"
                || total_episodes.is_none()
                || episode_number > total_episodes.unwrap()
            {
                return Err(ProtocolError("invalid_bootstrap_dependency_graph"));
            }
        }
        if entity.entity_type == "collection-member"
            && (!collections.contains(parts[1].as_str().unwrap())
                || !records.contains_key(parts[2].as_str().unwrap()))
        {
            return Err(ProtocolError("invalid_bootstrap_dependency_graph"));
        }
    }
    let stage_a_ordered_mutations = ordered
        .iter()
        .filter(|entity| rank(&entity.entity_type).unwrap() <= 1)
        .cloned()
        .collect::<Vec<_>>();
    let stage_b_ordered_mutations = ordered
        .iter()
        .filter(|entity| rank(&entity.entity_type).unwrap() >= 2)
        .cloned()
        .collect::<Vec<_>>();
    Ok(BootstrapPlan {
        stage_a_chunks: chunks(&stage_a_ordered_mutations),
        stage_b_chunks: chunks(&stage_b_ordered_mutations),
        stage_a_ordered_mutations,
        stage_b_ordered_mutations,
    })
}
