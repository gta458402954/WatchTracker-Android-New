//! Frozen producer mapping for ordinary S2 Lite v1 mutations.
//!
//! This module maps typed WatchTracker business values into the already frozen
//! Native entity/tombstone representation. It does not build commits, allocate
//! UUIDs, inspect S1 staging bases, or perform publication.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::canonical::{
    compare_entity_key_v1, jcs_bytes, validate_canonical_uuid_v4, validate_commit_ref,
    ProtocolError, Result,
};
use super::semantic::{
    business_field_order, canonical_semantic_value, validate_native_entity, validate_tombstone,
};
use super::types::{CommitMutationV1, CommitRef, EntityKey};

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LocalRecordV1 {
    pub id: String,
    pub original_name: String,
    pub chinese_name: String,
    pub progress: String,
    pub total_episodes: Option<i32>,
    pub episode_tracking_enabled: bool,
    pub next_episode: Option<i32>,
    pub movie_progress: Option<i32>,
    pub movie_duration: Option<i32>,
    pub release_year: Option<String>,
    pub poster_path: Option<String>,
    pub status: String,
    pub platform: String,
    pub rating: Option<i32>,
    pub start_date: Option<String>,
    pub end_date: Option<String>,
    pub notes: String,
    pub created_at: String,
    pub updated_at: Option<String>,
    pub imdb_id: Option<String>,
    pub is_locked: Option<bool>,
    pub genres: Option<String>,
    pub origin_country: Option<String>,
    pub imdb_rating: Option<f64>,
    pub tmdb_status: Option<String>,
    pub interest_level: Option<i32>,
    pub episode_runtime: Option<i32>,
    pub media_type: String,
    pub content_tags: Option<String>,
    pub tmdb_media_kind: Option<String>,
    pub tmdb_id: Option<i64>,
    pub tmdb_parent_id: Option<i64>,
    pub tmdb_season_number: Option<i32>,
    pub series_record_kind: Option<String>,
    pub rev: i64,
    pub rev_actor: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LocalCollectionV1 {
    pub id: String,
    pub name: String,
    pub normalized_name: String,
    pub description: Option<String>,
    pub source_kind: String,
    pub source_key: Option<String>,
    pub collection_kind: String,
    pub order_mode: String,
    pub created_at: String,
    pub updated_at: String,
    pub rev: i64,
    pub rev_actor: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LocalCollectionMemberV1 {
    pub id: String,
    pub collection_id: String,
    pub record_id: String,
    pub position: i64,
    pub source_kind: String,
    pub created_at: String,
    pub updated_at: String,
    pub rev: i64,
    pub rev_actor: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LocalEpisodeCompletionV1 {
    pub id: String,
    pub record_id: String,
    pub episode_number: i32,
    pub completed_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub rev: i64,
    pub rev_actor: String,
}

#[derive(Clone, Debug, PartialEq)]
pub enum LocalEntityValueV1 {
    Record(Box<LocalRecordV1>),
    Collection(LocalCollectionV1),
    CollectionMember(LocalCollectionMemberV1),
    EpisodeCompletion(LocalEpisodeCompletionV1),
}

#[derive(Clone, Debug, PartialEq)]
pub enum DeleteDescriptorV1 {
    Record {
        id: String,
        deleted_at: String,
        rev: i64,
        rev_actor: String,
    },
    Collection {
        id: String,
        deleted_at: String,
        rev: i64,
        rev_actor: String,
    },
    CollectionMember {
        id: String,
        collection_id: String,
        record_id: String,
        deleted_at: String,
        rev: i64,
        rev_actor: String,
    },
    EpisodeCompletion {
        id: String,
        record_id: String,
        episode_number: i32,
        deleted_at: String,
        rev: i64,
        rev_actor: String,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub enum OrdinaryPayloadV1 {
    Upsert(LocalEntityValueV1),
    Tombstone(DeleteDescriptorV1),
}

/// The live value is the canonical business-only value emitted by the causal
/// reducer, not an S1 staging snapshot.
#[derive(Clone, Debug, PartialEq)]
pub enum OrdinaryCausalBaseV1 {
    Absent,
    Tombstone,
    Live(Value),
}

#[derive(Clone, Debug, PartialEq)]
pub struct OrdinaryMutationRequestV1 {
    pub local_mutation_id: String,
    pub payload: OrdinaryPayloadV1,
    pub causal_base: OrdinaryCausalBaseV1,
    pub base_frontier: Vec<CommitRef>,
}

fn optional_finite_float64(value: Option<f64>) -> Result<Value> {
    match value {
        None => Ok(Value::Null),
        Some(value) if value.is_finite() => serde_json::Number::from_f64(value)
            .map(Value::Number)
            .ok_or(ProtocolError("invalid_float64")),
        Some(_) => Err(ProtocolError("invalid_float64")),
    }
}

fn record_wire(value: &LocalRecordV1) -> Result<Value> {
    let imdb_rating = optional_finite_float64(value.imdb_rating)?;
    Ok(json!({
        "id": value.id, "originalName": value.original_name, "chineseName": value.chinese_name,
        "progress": value.progress, "totalEpisodes": value.total_episodes,
        "episodeTrackingEnabled": value.episode_tracking_enabled, "nextEpisode": value.next_episode,
        "movieProgress": value.movie_progress, "movieDuration": value.movie_duration,
        "releaseYear": value.release_year, "posterPath": value.poster_path, "status": value.status,
        "platform": value.platform, "rating": value.rating, "startDate": value.start_date,
        "endDate": value.end_date, "notes": value.notes, "createdAt": value.created_at,
        "updatedAt": value.updated_at, "imdbId": value.imdb_id, "isLocked": value.is_locked,
        "genres": value.genres, "originCountry": value.origin_country, "imdbRating": imdb_rating,
        "tmdbStatus": value.tmdb_status, "interestLevel": value.interest_level,
        "episodeRuntime": value.episode_runtime, "mediaType": value.media_type,
        "contentTags": value.content_tags, "tmdbMediaKind": value.tmdb_media_kind,
        "tmdbId": value.tmdb_id.map(|item| item.to_string()),
        "tmdbParentId": value.tmdb_parent_id.map(|item| item.to_string()),
        "tmdbSeasonNumber": value.tmdb_season_number, "seriesRecordKind": value.series_record_kind,
        "rev": value.rev.to_string(), "revActor": value.rev_actor
    }))
}

impl LocalEntityValueV1 {
    pub fn entity_type(&self) -> &'static str {
        match self {
            Self::Record(_) => "record",
            Self::Collection(_) => "collection",
            Self::CollectionMember(_) => "collection-member",
            Self::EpisodeCompletion(_) => "episode-completion",
        }
    }

    pub fn entity_key(&self) -> EntityKey {
        match self {
            Self::Record(value) => json!(["record", value.id]),
            Self::Collection(value) => json!(["collection", value.id]),
            Self::CollectionMember(value) => {
                json!(["collection-member", value.collection_id, value.record_id])
            }
            Self::EpisodeCompletion(value) => {
                json!(["episode-completion", value.record_id, value.episode_number])
            }
        }
    }

    pub fn wire_value(&self) -> Result<Value> {
        match self {
            Self::Record(value) => record_wire(value),
            Self::Collection(value) => Ok(json!({
                "id": value.id, "name": value.name, "normalizedName": value.normalized_name,
                "description": value.description, "sourceKind": value.source_kind,
                "sourceKey": value.source_key, "collectionKind": value.collection_kind,
                "orderMode": value.order_mode, "createdAt": value.created_at,
                "updatedAt": value.updated_at, "rev": value.rev.to_string(), "revActor": value.rev_actor
            })),
            Self::CollectionMember(value) => Ok(json!({
                "id": value.id, "collectionId": value.collection_id, "recordId": value.record_id,
                "position": value.position.to_string(), "sourceKind": value.source_kind,
                "createdAt": value.created_at, "updatedAt": value.updated_at,
                "rev": value.rev.to_string(), "revActor": value.rev_actor
            })),
            Self::EpisodeCompletion(value) => Ok(json!({
                "id": value.id, "recordId": value.record_id, "episodeNumber": value.episode_number,
                "completedAt": value.completed_at, "createdAt": value.created_at,
                "updatedAt": value.updated_at, "rev": value.rev.to_string(), "revActor": value.rev_actor
            })),
        }
    }
}

impl DeleteDescriptorV1 {
    pub fn entity_type(&self) -> &'static str {
        match self {
            Self::Record { .. } => "record",
            Self::Collection { .. } => "collection",
            Self::CollectionMember { .. } => "collection-member",
            Self::EpisodeCompletion { .. } => "episode-completion",
        }
    }

    pub fn entity_key(&self) -> EntityKey {
        match self {
            Self::Record { id, .. } => json!(["record", id]),
            Self::Collection { id, .. } => json!(["collection", id]),
            Self::CollectionMember {
                collection_id,
                record_id,
                ..
            } => json!(["collection-member", collection_id, record_id]),
            Self::EpisodeCompletion {
                record_id,
                episode_number,
                ..
            } => json!(["episode-completion", record_id, episode_number]),
        }
    }

    pub fn wire_value(&self) -> Value {
        match self {
            Self::Record {
                id,
                deleted_at,
                rev,
                rev_actor,
            }
            | Self::Collection {
                id,
                deleted_at,
                rev,
                rev_actor,
            } => json!({
                "id": id, "deletedAt": deleted_at, "rev": rev.to_string(), "revActor": rev_actor
            }),
            Self::CollectionMember {
                id,
                collection_id,
                record_id,
                deleted_at,
                rev,
                rev_actor,
            } => json!({
                "id": id, "collectionId": collection_id, "recordId": record_id,
                "deletedAt": deleted_at, "rev": rev.to_string(), "revActor": rev_actor
            }),
            Self::EpisodeCompletion {
                id,
                record_id,
                episode_number,
                deleted_at,
                rev,
                rev_actor,
            } => json!({
                "id": id, "recordId": record_id, "episodeNumber": episode_number,
                "deletedAt": deleted_at, "rev": rev.to_string(), "revActor": rev_actor
            }),
        }
    }
}

impl OrdinaryPayloadV1 {
    pub fn entity_key(&self) -> EntityKey {
        match self {
            Self::Upsert(value) => value.entity_key(),
            Self::Tombstone(value) => value.entity_key(),
        }
    }
}

fn validate_base_frontier(frontier: &[CommitRef]) -> Result<()> {
    for reference in frontier {
        validate_commit_ref(reference)?;
    }
    Ok(())
}

fn expected_changed_fields(
    entity_type: &str,
    next: &Value,
    base: &OrdinaryCausalBaseV1,
) -> Result<Vec<String>> {
    let order = business_field_order(entity_type).ok_or(ProtocolError("invalid_entity_type"))?;
    if let OrdinaryCausalBaseV1::Live(prior) = base {
        let prior = prior
            .as_object()
            .ok_or(ProtocolError("invalid_ordinary_causal_base"))?;
        if prior.len() != order.len() || order.iter().any(|field| !prior.contains_key(*field)) {
            return Err(ProtocolError("invalid_ordinary_causal_base"));
        }
        let next = next
            .as_object()
            .ok_or(ProtocolError("invalid_entity_value"))?;
        let mut changed = Vec::new();
        for field in order {
            if jcs_bytes(&prior[*field])? != jcs_bytes(&next[*field])? {
                changed.push((*field).to_string());
            }
        }
        Ok(changed)
    } else {
        Ok(order.iter().map(|field| (*field).to_string()).collect())
    }
}

/// Maps one already coalesced change. `None` is the required representation
/// of an ordinary metadata-only/no-op upsert.
pub fn map_ordinary_mutation_v1(
    request: &OrdinaryMutationRequestV1,
) -> Result<Option<CommitMutationV1>> {
    validate_canonical_uuid_v4(&request.local_mutation_id)?;
    validate_base_frontier(&request.base_frontier)?;
    let entity_key = request.payload.entity_key();
    let (entity_type, operation, value, changed_fields) = match &request.payload {
        OrdinaryPayloadV1::Upsert(local) => {
            let entity_type = local.entity_type();
            let value = local.wire_value()?;
            validate_native_entity(&entity_key, &value)?;
            let semantic = canonical_semantic_value(&value)?;
            let changed = expected_changed_fields(entity_type, &semantic, &request.causal_base)?;
            if changed.is_empty() {
                return Ok(None);
            }
            (entity_type, "upsert", value, changed)
        }
        OrdinaryPayloadV1::Tombstone(delete) => {
            let entity_type = delete.entity_type();
            let value = delete.wire_value();
            validate_tombstone(entity_type, &entity_key, &value)?;
            (
                entity_type,
                "tombstone",
                value,
                vec!["$tombstone".to_string()],
            )
        }
    };
    Ok(Some(CommitMutationV1 {
        local_mutation_id: request.local_mutation_id.clone(),
        entity_type: entity_type.to_string(),
        entity_key,
        operation: operation.to_string(),
        value,
        base_frontier: request.base_frontier.clone(),
        changed_fields,
    }))
}

pub fn sort_ordinary_mutations_v1(mutations: &mut [CommitMutationV1]) -> Result<()> {
    mutations.sort_by(|left, right| compare_entity_key_v1(&left.entity_key, &right.entity_key));
    for pair in mutations.windows(2) {
        if compare_entity_key_v1(&pair[0].entity_key, &pair[1].entity_key).is_eq() {
            return Err(ProtocolError("duplicate_entity_key"));
        }
    }
    Ok(())
}

/// Coalesces mutable pre-freeze work by EntityKey. The last change wins; UUIDs
/// are deliberately absent here and are allocated only after this step.
pub fn coalesce_ordinary_payloads_v1(payloads: &[OrdinaryPayloadV1]) -> Vec<OrdinaryPayloadV1> {
    let mut retained: Vec<OrdinaryPayloadV1> = Vec::new();
    for payload in payloads {
        let key = payload.entity_key();
        if let Some(index) = retained
            .iter()
            .position(|candidate| compare_entity_key_v1(&candidate.entity_key(), &key).is_eq())
        {
            retained[index] = payload.clone();
        } else {
            retained.push(payload.clone());
        }
    }
    retained.sort_by(|left, right| compare_entity_key_v1(&left.entity_key(), &right.entity_key()));
    retained
}
