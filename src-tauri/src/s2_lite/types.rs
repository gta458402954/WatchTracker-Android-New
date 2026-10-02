use serde::{Deserialize, Serialize};
use serde_json::Value;

pub type EntityKey = Value;
pub type WriterSeqDecimalString = String;
pub type Int64DecimalString = String;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CommitDot {
    pub writer_id: String,
    pub writer_seq: WriterSeqDecimalString,
    pub commit_id: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CommitRef {
    pub writer_id: String,
    pub writer_seq: WriterSeqDecimalString,
    pub commit_id: String,
    pub content_hash: String,
}

impl CommitRef {
    pub fn dot(&self) -> CommitDot {
        CommitDot {
            writer_id: self.writer_id.clone(),
            writer_seq: self.writer_seq.clone(),
            commit_id: self.commit_id.clone(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EntityConflictKind {
    LiveTombstone,
    LockedConcurrent,
    DifferentBase,
    OverlappingField,
    DerivedDomain,
}

impl EntityConflictKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::LiveTombstone => "live-tombstone",
            Self::LockedConcurrent => "locked-concurrent",
            Self::DifferentBase => "different-base",
            Self::OverlappingField => "overlapping-field",
            Self::DerivedDomain => "derived-domain",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "live-tombstone" => Some(Self::LiveTombstone),
            "locked-concurrent" => Some(Self::LockedConcurrent),
            "different-base" => Some(Self::DifferentBase),
            "overlapping-field" => Some(Self::OverlappingField),
            "derived-domain" => Some(Self::DerivedDomain),
            _ => None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct EntityConflictAlternativeInput {
    pub commit_ref: CommitRef,
    pub semantic_state: Value,
    pub changed_fields: Vec<String>,
    pub base_frontier: Vec<CommitRef>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SemanticAlternative {
    pub dot: CommitDot,
    pub semantic_state: Value,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EntityConflictCore {
    pub domain: &'static str,
    pub entity_key: EntityKey,
    pub conflict_kind: &'static str,
    pub frontier_dots: Vec<CommitDot>,
    pub conflict_fields: Vec<String>,
    pub semantic_alternatives: Vec<SemanticAlternative>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RelationConflictKind {
    CollectionDeletedMemberLive,
    RecordDeletedMemberLive,
    RecordDeletedEpisodeLive,
    EpisodeExceedsTotal,
}

impl RelationConflictKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::CollectionDeletedMemberLive => "collection-deleted-member-live",
            Self::RecordDeletedMemberLive => "record-deleted-member-live",
            Self::RecordDeletedEpisodeLive => "record-deleted-episode-live",
            Self::EpisodeExceedsTotal => "episode-exceeds-total",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "collection-deleted-member-live" => Some(Self::CollectionDeletedMemberLive),
            "record-deleted-member-live" => Some(Self::RecordDeletedMemberLive),
            "record-deleted-episode-live" => Some(Self::RecordDeletedEpisodeLive),
            "episode-exceeds-total" => Some(Self::EpisodeExceedsTotal),
            _ => None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct RelationParticipant {
    pub entity_key: EntityKey,
    pub provenance_frontier: Vec<CommitRef>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RelationConflictCore {
    pub domain: &'static str,
    pub relation_kind: &'static str,
    pub entity_keys: Vec<EntityKey>,
    pub version_refs: Vec<CommitRef>,
    pub semantic_relation_facts: Value,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BootstrapEntity {
    pub entity_type: String,
    pub entity_key: EntityKey,
    pub value: Value,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BootstrapPlan {
    pub stage_a_ordered_mutations: Vec<BootstrapEntity>,
    pub stage_a_chunks: Vec<Vec<BootstrapEntity>>,
    pub stage_b_ordered_mutations: Vec<BootstrapEntity>,
    pub stage_b_chunks: Vec<Vec<BootstrapEntity>>,
}

pub trait LegacySemanticAdapterV1 {
    fn adapt_live_entity(
        &self,
        entity_type: &str,
        legacy_value: &Value,
    ) -> std::result::Result<BootstrapEntity, String>;
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CommitMutationV1 {
    pub local_mutation_id: String,
    pub entity_type: String,
    pub entity_key: EntityKey,
    pub operation: String,
    pub value: Value,
    pub base_frontier: Vec<CommitRef>,
    pub changed_fields: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CommitSourceV1 {
    pub r#type: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CommitV1 {
    pub protocol: String,
    pub protocol_version: u8,
    pub s2_semantic_profile_version: u8,
    pub required_features: Vec<String>,
    pub writer_id: String,
    pub writer_seq: WriterSeqDecimalString,
    pub commit_id: String,
    pub content_hash: String,
    pub previous_writer_commit: Option<CommitRef>,
    pub basis_clock: Vec<CommitRef>,
    pub commit_kind: String,
    pub created_at: String,
    pub source: CommitSourceV1,
    pub resolves: Vec<String>,
    pub mutations: Vec<CommitMutationV1>,
}

impl CommitV1 {
    pub fn commit_ref(&self) -> CommitRef {
        CommitRef {
            writer_id: self.writer_id.clone(),
            writer_seq: self.writer_seq.clone(),
            commit_id: self.commit_id.clone(),
            content_hash: self.content_hash.clone(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum HistoricalValidityState {
    Pending,
    Valid,
    Invalid,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoricalValidity {
    pub state: HistoricalValidityState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EntityVersionV1 {
    pub entity_key: EntityKey,
    pub operation: String,
    pub full_value: Value,
    pub semantic_state: Value,
    pub canonical_semantic_value: Option<Value>,
    pub changed_fields: Vec<String>,
    pub base_frontier: Vec<CommitRef>,
    pub commit_ref: CommitRef,
    pub commit_dot: CommitDot,
    pub causal_basis: Vec<CommitRef>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MetadataVariantV1 {
    pub commit_ref: CommitRef,
    pub metadata: Value,
}

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "state")]
pub enum MaterializedEntityV1 {
    Absent,
    Resolved {
        #[serde(rename = "semanticState")]
        semantic_state: Value,
        #[serde(rename = "businessValue")]
        business_value: Option<Value>,
        #[serde(rename = "provenanceFrontier")]
        provenance_frontier: Vec<CommitRef>,
        #[serde(rename = "metadataVariants")]
        metadata_variants: Vec<MetadataVariantV1>,
    },
    Conflict {
        #[serde(rename = "conflictId")]
        conflict_id: String,
        #[serde(rename = "conflictKind")]
        conflict_kind: String,
        #[serde(rename = "entityKey")]
        entity_key: EntityKey,
        frontier: Vec<CommitRef>,
        #[serde(rename = "conflictFields")]
        conflict_fields: Vec<String>,
        #[serde(rename = "semanticAlternatives")]
        semantic_alternatives: Vec<SemanticAlternative>,
    },
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WriterForkV1 {
    pub writer_id: String,
    pub writer_seq: WriterSeqDecimalString,
    pub alternatives: Vec<CommitRef>,
    pub safe_writer_frontier: WriterSeqDecimalString,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RelationConflictV1 {
    pub relation_conflict_id: String,
    pub core: RelationConflictCore,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RelationDetectionV1 {
    pub conflicts: Vec<RelationConflictV1>,
    pub blocked_by_entity_conflict: Vec<EntityKey>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DuplicateDiagnosticV1 {
    pub kind: String,
    pub value: Value,
    pub entity_keys: Vec<EntityKey>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ValidityEntryV1 {
    pub commit_ref: CommitRef,
    pub validity: HistoricalValidity,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FrontierEntryV1 {
    pub entity_key: EntityKey,
    pub frontier: Vec<CommitRef>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MaterializedEntryV1 {
    pub entity_key: EntityKey,
    pub value: MaterializedEntityV1,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VerifiedReplayV1 {
    pub validity: Vec<ValidityEntryV1>,
    pub forks: Vec<WriterForkV1>,
    pub unsafe_commit_refs: Vec<CommitRef>,
    pub forensic_versions: Vec<EntityVersionV1>,
    pub versions: Vec<EntityVersionV1>,
    pub frontiers: Vec<FrontierEntryV1>,
    pub materialized: Vec<MaterializedEntryV1>,
    pub relations: RelationDetectionV1,
    pub duplicate_diagnostics: Vec<DuplicateDiagnosticV1>,
}
