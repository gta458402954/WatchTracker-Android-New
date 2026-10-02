use std::fs;

use serde_json::{json, Value};

use super::ordinary_mutation::{
    coalesce_ordinary_payloads_v1, map_ordinary_mutation_v1, sort_ordinary_mutations_v1,
    DeleteDescriptorV1, LocalCollectionMemberV1, LocalCollectionV1, LocalEntityValueV1,
    LocalEpisodeCompletionV1, LocalRecordV1, OrdinaryCausalBaseV1, OrdinaryMutationRequestV1,
    OrdinaryPayloadV1,
};
use super::semantic::{canonical_semantic_value, validate_native_entity};
use super::types::{CommitMutationV1, CommitRef};

fn fixture() -> Value {
    serde_json::from_str(
        &fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../contracts/s2-lite/v1/ordinary-mutation-semantic-golden-v1.json"
        ))
        .unwrap(),
    )
    .unwrap()
}

fn string(value: &Value, field: &str) -> String {
    value[field].as_str().unwrap().to_string()
}

fn optional_string(value: &Value, field: &str) -> Option<String> {
    value[field].as_str().map(str::to_string)
}

fn optional_i32(value: &Value, field: &str) -> Option<i32> {
    value[field].as_i64().map(|item| item as i32)
}

fn typed_entity(value: &Value) -> LocalEntityValueV1 {
    let entity_type = value["entityType"].as_str().unwrap();
    let value = &value["value"];
    match entity_type {
        "record" => LocalEntityValueV1::Record(Box::new(LocalRecordV1 {
            id: string(value, "id"),
            original_name: string(value, "originalName"),
            chinese_name: string(value, "chineseName"),
            progress: string(value, "progress"),
            total_episodes: optional_i32(value, "totalEpisodes"),
            episode_tracking_enabled: value["episodeTrackingEnabled"].as_bool().unwrap(),
            next_episode: optional_i32(value, "nextEpisode"),
            movie_progress: optional_i32(value, "movieProgress"),
            movie_duration: optional_i32(value, "movieDuration"),
            release_year: optional_string(value, "releaseYear"),
            poster_path: optional_string(value, "posterPath"),
            status: string(value, "status"),
            platform: string(value, "platform"),
            rating: optional_i32(value, "rating"),
            start_date: optional_string(value, "startDate"),
            end_date: optional_string(value, "endDate"),
            notes: string(value, "notes"),
            created_at: string(value, "createdAt"),
            updated_at: optional_string(value, "updatedAt"),
            imdb_id: optional_string(value, "imdbId"),
            is_locked: value["isLocked"].as_bool(),
            genres: optional_string(value, "genres"),
            origin_country: optional_string(value, "originCountry"),
            imdb_rating: value["imdbRating"].as_f64(),
            tmdb_status: optional_string(value, "tmdbStatus"),
            interest_level: optional_i32(value, "interestLevel"),
            episode_runtime: optional_i32(value, "episodeRuntime"),
            media_type: string(value, "mediaType"),
            content_tags: optional_string(value, "contentTags"),
            tmdb_media_kind: optional_string(value, "tmdbMediaKind"),
            tmdb_id: value["tmdbId"].as_str().map(|item| item.parse().unwrap()),
            tmdb_parent_id: value["tmdbParentId"]
                .as_str()
                .map(|item| item.parse().unwrap()),
            tmdb_season_number: optional_i32(value, "tmdbSeasonNumber"),
            series_record_kind: optional_string(value, "seriesRecordKind"),
            rev: value["rev"].as_str().unwrap().parse().unwrap(),
            rev_actor: string(value, "revActor"),
        })),
        "collection" => LocalEntityValueV1::Collection(LocalCollectionV1 {
            id: string(value, "id"),
            name: string(value, "name"),
            normalized_name: string(value, "normalizedName"),
            description: optional_string(value, "description"),
            source_kind: string(value, "sourceKind"),
            source_key: optional_string(value, "sourceKey"),
            collection_kind: string(value, "collectionKind"),
            order_mode: string(value, "orderMode"),
            created_at: string(value, "createdAt"),
            updated_at: string(value, "updatedAt"),
            rev: value["rev"].as_str().unwrap().parse().unwrap(),
            rev_actor: string(value, "revActor"),
        }),
        "collection-member" => LocalEntityValueV1::CollectionMember(LocalCollectionMemberV1 {
            id: string(value, "id"),
            collection_id: string(value, "collectionId"),
            record_id: string(value, "recordId"),
            position: value["position"].as_str().unwrap().parse().unwrap(),
            source_kind: string(value, "sourceKind"),
            created_at: string(value, "createdAt"),
            updated_at: string(value, "updatedAt"),
            rev: value["rev"].as_str().unwrap().parse().unwrap(),
            rev_actor: string(value, "revActor"),
        }),
        "episode-completion" => LocalEntityValueV1::EpisodeCompletion(LocalEpisodeCompletionV1 {
            id: string(value, "id"),
            record_id: string(value, "recordId"),
            episode_number: value["episodeNumber"].as_i64().unwrap() as i32,
            completed_at: optional_string(value, "completedAt"),
            created_at: string(value, "createdAt"),
            updated_at: string(value, "updatedAt"),
            rev: value["rev"].as_str().unwrap().parse().unwrap(),
            rev_actor: string(value, "revActor"),
        }),
        _ => panic!("unknown fixture entity type"),
    }
}

fn delete_descriptor(value: &Value) -> DeleteDescriptorV1 {
    let id = string(value, "id");
    let deleted_at = string(value, "deletedAt");
    let rev = value["rev"].as_str().unwrap().parse().unwrap();
    let rev_actor = string(value, "revActor");
    match value["entityType"].as_str().unwrap() {
        "record" => DeleteDescriptorV1::Record {
            id,
            deleted_at,
            rev,
            rev_actor,
        },
        "collection" => DeleteDescriptorV1::Collection {
            id,
            deleted_at,
            rev,
            rev_actor,
        },
        "collection-member" => DeleteDescriptorV1::CollectionMember {
            id,
            collection_id: string(value, "collectionId"),
            record_id: string(value, "recordId"),
            deleted_at,
            rev,
            rev_actor,
        },
        "episode-completion" => DeleteDescriptorV1::EpisodeCompletion {
            id,
            record_id: string(value, "recordId"),
            episode_number: value["episodeNumber"].as_i64().unwrap() as i32,
            deleted_at,
            rev,
            rev_actor,
        },
        _ => panic!("unknown delete fixture entity type"),
    }
}

fn refs(fixture: &Value, names: &Value) -> Vec<CommitRef> {
    names
        .as_array()
        .unwrap()
        .iter()
        .map(|name| {
            serde_json::from_value(fixture["refs"][name.as_str().unwrap()].clone()).unwrap()
        })
        .collect()
}

fn request(fixture: &Value, case: &Value) -> OrdinaryMutationRequestV1 {
    let input = &case["input"];
    let payload = &input["payload"];
    let payload = if payload["operation"] == "upsert" {
        OrdinaryPayloadV1::Upsert(typed_entity(
            &fixture["localValues"][payload["localValueRef"].as_str().unwrap()],
        ))
    } else {
        OrdinaryPayloadV1::Tombstone(delete_descriptor(
            &fixture["deleteDescriptors"][payload["deleteDescriptorRef"].as_str().unwrap()],
        ))
    };
    let causal_base = match input["causalBase"]["state"].as_str().unwrap() {
        "absent" => OrdinaryCausalBaseV1::Absent,
        "tombstone" => OrdinaryCausalBaseV1::Tombstone,
        "live" => {
            let entity = typed_entity(
                &fixture["localValues"][input["causalBase"]["valueRef"].as_str().unwrap()],
            );
            OrdinaryCausalBaseV1::Live(
                canonical_semantic_value(&entity.wire_value().unwrap()).unwrap(),
            )
        }
        _ => panic!("unknown causal base"),
    };
    OrdinaryMutationRequestV1 {
        local_mutation_id: string(input, "localMutationId"),
        payload,
        causal_base,
        base_frontier: refs(fixture, &input["baseFrontier"]),
    }
}

fn expected(fixture: &Value, case: &Value) -> Option<Value> {
    let expected = &case["expected"];
    if expected.is_null() {
        return None;
    }
    let value = if let Some(name) = expected.get("valueRef").and_then(Value::as_str) {
        typed_entity(&fixture["localValues"][name])
            .wire_value()
            .unwrap()
    } else {
        delete_descriptor(
            &fixture["deleteDescriptors"][expected["deleteDescriptorRef"].as_str().unwrap()],
        )
        .wire_value()
    };
    Some(json!({
        "localMutationId": case["input"]["localMutationId"],
        "entityType": expected["entityType"],
        "entityKey": expected["entityKey"],
        "operation": expected["operation"],
        "value": value,
        "baseFrontier": refs(fixture, &expected["baseFrontier"]),
        "changedFields": expected["changedFields"]
    }))
}

#[test]
fn shared_ordinary_mutation_vectors_match_before_hashing() {
    let fixture = fixture();
    let cases = fixture["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 13);
    for case in cases {
        let actual = map_ordinary_mutation_v1(&request(&fixture, case)).unwrap();
        assert_eq!(
            actual.map(|value| serde_json::to_value(value).unwrap()),
            expected(&fixture, case),
            "{}",
            case["name"]
        );
    }
}

#[test]
fn shared_null_missing_coalescing_and_ordering_contracts_hold() {
    let fixture = fixture();
    let record = typed_entity(&fixture["localValues"]["recordA"]);
    validate_native_entity(&record.entity_key(), &record.wire_value().unwrap()).unwrap();
    let mut missing = record.wire_value().unwrap();
    missing.as_object_mut().unwrap().remove("updatedAt");
    assert_eq!(
        validate_native_entity(&record.entity_key(), &missing)
            .unwrap_err()
            .0,
        "invalid_entity_fields"
    );

    let payloads = fixture["coalescing"]["inputPayloadRefs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|name| {
            OrdinaryPayloadV1::Upsert(typed_entity(
                &fixture["localValues"][name.as_str().unwrap()],
            ))
        })
        .collect::<Vec<_>>();
    let coalesced = coalesce_ordinary_payloads_v1(&payloads);
    assert_eq!(coalesced.len(), 2);
    assert_eq!(coalesced[0].entity_key(), json!(["collection", "c1"]));
    assert_eq!(coalesced[1].entity_key(), json!(["record", "r1"]));
    assert_eq!(coalesced[1], payloads[2]);

    let partial = fixture["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|case| case["name"] == "record-partial-update")
        .unwrap();
    let mut frozen_request = request(&fixture, partial);
    frozen_request.local_mutation_id = fixture["coalescing"]["postFreeze"]["frozenMutationId"]
        .as_str()
        .unwrap()
        .to_string();
    let frozen = map_ordinary_mutation_v1(&frozen_request).unwrap().unwrap();
    let frozen_copy = serde_json::to_value(&frozen).unwrap();
    let mut successor_request = frozen_request.clone();
    successor_request.local_mutation_id = fixture["coalescing"]["postFreeze"]
        ["successorMutationId"]
        .as_str()
        .unwrap()
        .to_string();
    successor_request.payload = payloads[0].clone();
    successor_request.causal_base = OrdinaryCausalBaseV1::Live(
        canonical_semantic_value(
            &typed_entity(&fixture["localValues"]["recordB"])
                .wire_value()
                .unwrap(),
        )
        .unwrap(),
    );
    let successor = map_ordinary_mutation_v1(&successor_request)
        .unwrap()
        .unwrap();
    assert_ne!(frozen.local_mutation_id, successor.local_mutation_id);
    assert_eq!(serde_json::to_value(&frozen).unwrap(), frozen_copy);

    let selected = fixture["deterministicOrdering"]["inputCaseNames"]
        .as_array()
        .unwrap()
        .iter()
        .map(|name| {
            let case = fixture["cases"]
                .as_array()
                .unwrap()
                .iter()
                .find(|case| case["name"] == *name)
                .unwrap();
            map_ordinary_mutation_v1(&request(&fixture, case))
                .unwrap()
                .unwrap()
        })
        .collect::<Vec<CommitMutationV1>>();
    let mut sorted = selected;
    sort_ordinary_mutations_v1(&mut sorted).unwrap();
    let expected_names = fixture["deterministicOrdering"]["expectedCaseNames"]
        .as_array()
        .unwrap();
    for (mutation, name) in sorted.iter().zip(expected_names) {
        let case = fixture["cases"]
            .as_array()
            .unwrap()
            .iter()
            .find(|case| case["name"] == *name)
            .unwrap();
        assert_eq!(mutation.entity_key, case["expected"]["entityKey"]);
    }
}

fn fixture_float64(value: &Value) -> Option<f64> {
    match value.as_str() {
        Some("NONE") => None,
        Some("NON_FINITE_NAN") => Some(f64::NAN),
        Some("NON_FINITE_POS_INF") => Some(f64::INFINITY),
        Some("NON_FINITE_NEG_INF") => Some(f64::NEG_INFINITY),
        Some(other) => panic!("unknown Float64 fixture tag: {other}"),
        None => Some(value.as_f64().unwrap()),
    }
}

fn record_with_rating(fixture: &Value, rating: Option<f64>) -> LocalEntityValueV1 {
    let mut entity = typed_entity(&fixture["localValues"]["recordA"]);
    let LocalEntityValueV1::Record(record) = &mut entity else {
        unreachable!("recordA fixture")
    };
    record.imdb_rating = rating;
    entity
}

#[test]
fn shared_typed_float64_cases_reject_non_finite_before_json_conversion() {
    let fixture = fixture();
    for (index, case) in fixture["typedFloat64Cases"]
        .as_array()
        .unwrap()
        .iter()
        .enumerate()
    {
        let outgoing_rating = fixture_float64(&case["outgoing"]);
        let outgoing = record_with_rating(&fixture, outgoing_rating);
        let causal_base = if case["causalBaseRating"] == "ABSENT" {
            OrdinaryCausalBaseV1::Absent
        } else {
            let base_rating = if case["causalBaseRating"].is_null() {
                None
            } else {
                Some(case["causalBaseRating"].as_f64().unwrap())
            };
            OrdinaryCausalBaseV1::Live(
                canonical_semantic_value(
                    &record_with_rating(&fixture, base_rating)
                        .wire_value()
                        .unwrap(),
                )
                .unwrap(),
            )
        };
        let request = OrdinaryMutationRequestV1 {
            local_mutation_id: format!("30000000-0000-4000-8000-0000000001{index:02}"),
            payload: OrdinaryPayloadV1::Upsert(outgoing.clone()),
            causal_base,
            base_frontier: if case["causalBaseRating"] == "ABSENT" {
                vec![]
            } else {
                refs(&fixture, &json!(["base"]))
            },
        };
        if let Some(expected_error) = case.get("expectedError").and_then(Value::as_str) {
            assert_eq!(outgoing.wire_value().unwrap_err().0, expected_error);
            assert_eq!(
                map_ordinary_mutation_v1(&request).unwrap_err().0,
                expected_error,
                "{}",
                case["name"]
            );
            continue;
        }
        let mutation = map_ordinary_mutation_v1(&request)
            .unwrap()
            .expect("valid Float64 case emits a mutation");
        match case["expected"].as_str().unwrap() {
            "VALID_NULL" => {
                assert!(mutation.value["imdbRating"].is_null());
                assert!(mutation.changed_fields.contains(&"imdbRating".to_string()));
            }
            "VALID_NULL_CLEAR" => {
                assert!(mutation.value["imdbRating"].is_null());
                assert_eq!(mutation.changed_fields, ["imdbRating"]);
            }
            "VALID_NUMBER" => {
                assert_eq!(mutation.value["imdbRating"].as_f64(), outgoing_rating);
                assert_eq!(mutation.changed_fields, ["imdbRating"]);
            }
            other => panic!("unknown expected Float64 result: {other}"),
        }
    }
}
