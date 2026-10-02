use super::bootstrap::build_bootstrap_plan_v1;
use super::canonical::{
    canonical_collection_name_v1, canonical_entity_key_bytes, compare_commit_dot_v1,
    compare_commit_ref_v1, compare_entity_key_v1, jcs_bytes, normalize_collection_name_v1,
    parse_int64_decimal, parse_writer_seq, sha256_hex, sha256_jcs, validate_canonical_uuid,
    validate_canonical_uuid_v4, validate_date, validate_entity_key, validate_float64,
    validate_safe_integer, validate_timestamp,
};
use super::conflict::{
    build_entity_conflict_core_v1, build_relation_conflict_core_v1, entity_conflict_id_v1,
    relation_conflict_id_v1,
};
use super::semantic::{canonical_semantic_value, validate_native_entity, validate_tombstone};
use super::types::{
    BootstrapEntity, CommitDot, CommitRef, EntityConflictAlternativeInput, EntityConflictKind,
    RelationConflictKind, RelationParticipant,
};
use serde_json::{json, Value};
use std::cmp::Ordering;

fn fixture() -> Value {
    serde_json::from_str(include_str!(
        "../../../contracts/s2-lite/v1/conflict-golden-v1.json"
    ))
    .expect("valid shared fixture")
}

fn jcs_oracle() -> Value {
    serde_json::from_str(include_str!(
        "../../../contracts/s2-lite/v1/jcs-oracle-v1.json"
    ))
    .expect("valid shared JCS oracle")
}

fn reference(fixture: &Value, name: &str) -> CommitRef {
    serde_json::from_value(fixture["refs"][name].clone()).expect("valid fixture CommitRef")
}

fn resolve_state(fixture: &Value, state: &Value) -> Value {
    if state["state"] == "tombstone" {
        json!({ "state": "tombstone" })
    } else {
        json!({
            "state": "live",
            "value": fixture["semanticValues"][state["value"].as_str().unwrap()].clone()
        })
    }
}

#[test]
fn conflict_codec_matches_all_nine_language_neutral_vectors() {
    let fixture = fixture();
    let mut count = 0;
    for vector in fixture["entityCases"].as_array().unwrap() {
        let alternatives = vector["alternatives"]
            .as_array()
            .unwrap()
            .iter()
            .map(|alternative| EntityConflictAlternativeInput {
                commit_ref: reference(&fixture, alternative["ref"].as_str().unwrap()),
                semantic_state: resolve_state(&fixture, &alternative["semanticState"]),
                changed_fields: alternative["changedFields"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|field| field.as_str().unwrap().to_string())
                    .collect(),
                base_frontier: alternative["baseRefs"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|name| reference(&fixture, name.as_str().unwrap()))
                    .collect(),
            })
            .collect();
        let kind = EntityConflictKind::parse(vector["conflictKind"].as_str().unwrap()).unwrap();
        let core = build_entity_conflict_core_v1(vector["entityKey"].clone(), kind, alternatives)
            .expect("construct entity core from logical input");
        assert_eq!(
            jcs_bytes(&core).unwrap(),
            vector["expectedJcs"].as_str().unwrap().as_bytes(),
            "{}",
            vector["name"]
        );
        assert_eq!(
            entity_conflict_id_v1(&core).unwrap(),
            vector["expectedSha256"].as_str().unwrap(),
            "{}",
            vector["name"]
        );
        assert_eq!(
            sha256_hex(&jcs_bytes(&core).unwrap()),
            vector["expectedSha256"]
        );
        count += 1;
    }
    for vector in fixture["relationCases"].as_array().unwrap() {
        let participants = vector["participants"]
            .as_array()
            .unwrap()
            .iter()
            .map(|participant| RelationParticipant {
                entity_key: participant["entityKey"].clone(),
                provenance_frontier: participant["refs"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|name| reference(&fixture, name.as_str().unwrap()))
                    .collect(),
            })
            .collect();
        let kind = RelationConflictKind::parse(vector["relationKind"].as_str().unwrap()).unwrap();
        let core = build_relation_conflict_core_v1(kind, vector["facts"].clone(), participants)
            .expect("construct relation core from logical input");
        assert_eq!(
            jcs_bytes(&core).unwrap(),
            vector["expectedJcs"].as_str().unwrap().as_bytes(),
            "{}",
            vector["name"]
        );
        assert_eq!(
            relation_conflict_id_v1(&core).unwrap(),
            vector["expectedSha256"].as_str().unwrap(),
            "{}",
            vector["name"]
        );
        count += 1;
    }
    assert_eq!(count, 9);
}

#[test]
fn jcs_matches_shared_utf16_exact_byte_oracle_and_rejects_non_finite() {
    for vector in jcs_oracle()["cases"].as_array().unwrap() {
        assert_eq!(
            jcs_bytes(&vector["input"]).unwrap(),
            vector["expectedJcs"].as_str().unwrap().as_bytes(),
            "{}",
            vector["name"]
        );
    }
    for invalid in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        assert_eq!(jcs_bytes(&invalid).unwrap_err().0, "jcs_non_finite_number");
    }
    #[derive(serde::Serialize)]
    struct NestedNonFinite {
        nested: Vec<f64>,
    }
    assert!(jcs_bytes(&NestedNonFinite {
        nested: vec![f64::NAN],
    })
    .is_err());
}

#[test]
fn raw_json_float_parsing_produces_ecmascript_jcs_number_bytes() {
    let cases = [
        ("0", "0"),
        ("-0", "0"),
        ("0.1", "0.1"),
        ("0.2", "0.2"),
        ("0.3", "0.3"),
        ("1.2345678901234567", "1.2345678901234567"),
        ("2.3307731538713474", "2.3307731538713474"),
        ("9.999999999999998", "9.999999999999998"),
        ("10", "10"),
        ("1.0000000000000002", "1.0000000000000002"),
        ("0.10000000000000002", "0.10000000000000002"),
        ("4.9406564584124654e-324", "5e-324"),
        ("2.2250738585072014e-308", "2.2250738585072014e-308"),
        ("7.84551240822557", "7.84551240822557"),
        ("3.141592653589793", "3.141592653589793"),
    ];
    for (raw, expected) in cases {
        let parsed: Value = serde_json::from_str(raw).unwrap();
        assert_eq!(jcs_bytes(&parsed).unwrap(), expected.as_bytes(), "{raw}");
    }

    let mut state = 0x5eed_1234_u32;
    let mut output = Vec::new();
    for index in 0..4096 {
        state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        let value = (f64::from(state) / 4_294_967_296.0) * 10.0;
        let raw = ryu_js::Buffer::new().format_finite(value).to_string();
        let parsed: Value = serde_json::from_str(&raw).unwrap();
        if index != 0 {
            output.push(b'\n');
        }
        output.extend(jcs_bytes(&parsed).unwrap());
    }
    assert_eq!(
        sha256_hex(&output),
        "d33fd9c27b6203f60b03a2be8cdc32c7c86c18c4857b65dd91107f485c158b79"
    );
}

#[test]
fn reviewer_float_counterexample_survives_raw_parse_through_conflict_hashing() {
    let fixture = fixture();
    let mut source_a = native_record(&fixture, "2026-09-06T10:00:00.000Z");
    source_a["id"] = json!("r-lock");
    source_a["notes"] = json!("");
    source_a["isLocked"] = json!(true);
    let raw_a = serde_json::to_string(&source_a).unwrap();
    let mut source_b = native_record(&fixture, "2026-09-06T10:00:00.000Z");
    source_b["id"] = json!("r-lock");
    let mut raw_b = serde_json::to_string(&source_b).unwrap();
    raw_b = raw_b.replace("\"imdbRating\":null", "\"imdbRating\":2.3307731538713474");
    let record_a: Value = serde_json::from_str(&raw_a).unwrap();
    let record_b: Value = serde_json::from_str(&raw_b).unwrap();
    validate_native_entity(&json!(["record", "r-lock"]), &record_a).unwrap();
    validate_native_entity(&json!(["record", "r-lock"]), &record_b).unwrap();
    let core = build_entity_conflict_core_v1(
        json!(["record", "r-lock"]),
        EntityConflictKind::LockedConcurrent,
        vec![
            EntityConflictAlternativeInput {
                commit_ref: reference(&fixture, "RA"),
                semantic_state: json!({
                    "state": "live",
                    "value": canonical_semantic_value(&record_a).unwrap()
                }),
                changed_fields: vec!["isLocked".into()],
                base_frontier: vec![reference(&fixture, "P")],
            },
            EntityConflictAlternativeInput {
                commit_ref: reference(&fixture, "RB"),
                semantic_state: json!({
                    "state": "live",
                    "value": canonical_semantic_value(&record_b).unwrap()
                }),
                changed_fields: vec!["notes".into(), "imdbRating".into()],
                base_frontier: vec![reference(&fixture, "P")],
            },
        ],
    )
    .unwrap();
    let expected =
        include_str!("../../../contracts/s2-lite/v1/float-roundtrip-conflict-v1.jcs").trim_end();
    assert_eq!(jcs_bytes(&core).unwrap(), expected.as_bytes());
    assert_eq!(
        entity_conflict_id_v1(&core).unwrap(),
        "94e8c8387bad34bb0d2e64f683ddc886998b2b4c3ff8fff6a2f9a6e6a0a1e90c"
    );
}

#[test]
fn commit_dot_comparator_trap_and_numeric_sequence() {
    let fixture = fixture();
    let ra = reference(&fixture, "RA").dot();
    let rb = reference(&fixture, "RB").dot();
    assert_eq!(compare_commit_dot_v1(&ra, &rb), Ordering::Less);
    assert!(jcs_bytes(&ra).unwrap() > jcs_bytes(&rb).unwrap());
    let two = CommitDot {
        writer_seq: "2".into(),
        ..ra.clone()
    };
    let ten = CommitDot {
        writer_seq: "10".into(),
        ..ra
    };
    assert_eq!(compare_commit_dot_v1(&two, &ten), Ordering::Less);
}

#[test]
fn integer_boundaries_remain_exact() {
    assert_eq!(
        parse_int64_decimal("9007199254740991", i64::MIN, i64::MAX).unwrap(),
        9_007_199_254_740_991
    );
    assert_eq!(
        parse_int64_decimal("9007199254740992", i64::MIN, i64::MAX).unwrap(),
        9_007_199_254_740_992
    );
    assert_eq!(
        parse_int64_decimal("9007199254740993", i64::MIN, i64::MAX).unwrap(),
        9_007_199_254_740_993
    );
    assert_ne!("9007199254740992", "9007199254740993");
    assert_eq!(
        parse_int64_decimal("9223372036854775807", i64::MIN, i64::MAX).unwrap(),
        i64::MAX
    );
    assert_eq!(
        parse_int64_decimal("-9223372036854775808", i64::MIN, i64::MAX).unwrap(),
        i64::MIN
    );
    assert_eq!(parse_writer_seq("18446744073709551615").unwrap(), u64::MAX);
    assert!(parse_writer_seq("18446744073709551616").is_err());
    assert_ne!(
        sha256_jcs(&json!({"value": "9007199254740992"})).unwrap(),
        sha256_jcs(&json!({"value": "9007199254740993"})).unwrap()
    );
}

#[test]
fn canonical_scalar_contract_is_strict() {
    validate_canonical_uuid("11111111-1111-4111-8111-111111111111").unwrap();
    validate_canonical_uuid_v4("11111111-1111-4111-8111-111111111111").unwrap();
    assert!(validate_canonical_uuid("11111111-1111-4111-8111-11111111111A").is_err());
    assert!(validate_canonical_uuid_v4("11111111-1111-1111-8111-111111111111").is_err());
    for invalid in ["0", "01", "+1", " 1"] {
        assert!(parse_writer_seq(invalid).is_err());
    }
    for invalid in ["-0", "+1", "01", " 1"] {
        assert!(parse_int64_decimal(invalid, i64::MIN, i64::MAX).is_err());
    }
    validate_safe_integer(&json!(9_007_199_254_740_991_i64), i64::MIN, i64::MAX).unwrap();
    validate_float64(&json!(10.0), 0.0, 10.0).unwrap();
    assert!(validate_float64(&json!(11.0), 0.0, 10.0).is_err());
    validate_date("2024-02-29").unwrap();
    assert!(validate_date("2023-02-29").is_err());
    validate_timestamp("2026-09-06T10:00:00.000Z").unwrap();
    assert!(validate_timestamp("2026-09-06T18:00:00.000+08:00").is_err());
    assert_ne!(
        jcs_bytes(&Value::Null).unwrap(),
        jcs_bytes(&json!("")).unwrap()
    );
    assert_ne!(
        jcs_bytes(&json!("é")).unwrap(),
        jcs_bytes(&json!("e\u{301}")).unwrap()
    );
    let negative_zero: Value = serde_json::from_str("-0").unwrap();
    assert_eq!(jcs_bytes(&negative_zero).unwrap(), b"0");
    assert_eq!(normalize_collection_name_v1("FaVÉ"), "favÉ");
}

#[test]
fn collection_s2ws_covers_all_six_code_points() {
    assert_eq!(canonical_collection_name_v1(" A  B ").unwrap(), "A B");
    assert_eq!(canonical_collection_name_v1("A \t B").unwrap(), "A B");
    assert_eq!(canonical_collection_name_v1("\tA\nB\r").unwrap(), "A B");
    for whitespace in [
        '\u{0009}', '\u{000a}', '\u{000b}', '\u{000c}', '\u{000d}', '\u{0020}',
    ] {
        let input = format!("{whitespace}A{whitespace}{whitespace}B{whitespace}");
        assert_eq!(canonical_collection_name_v1(&input).unwrap(), "A B");
    }
    assert_eq!(
        canonical_collection_name_v1(" \t\n\u{000b}\u{000c}\r A \t B \r ").unwrap(),
        "A B"
    );
    assert!(canonical_collection_name_v1(" ").is_err());
    assert!(canonical_collection_name_v1("\u{0001}A").is_err());
    assert!(canonical_collection_name_v1("A\u{007f}").is_err());
    let mut collection = bootstrap_case(1, 1, 0, 0).pop().unwrap();
    collection.value["name"] = json!(" A  B ");
    collection.value["normalizedName"] = json!(" a  b ");
    assert!(validate_native_entity(&collection.entity_key, &collection.value).is_err());
}

#[test]
fn date_timestamp_acceptance_matrix_is_fixed_width_ascii() {
    for valid in ["0001-01-01", "2000-02-29", "2024-02-29", "9999-12-31"] {
        validate_date(valid).unwrap();
    }
    for invalid in [
        "0000-01-01",
        "2023-02-29",
        "1900-02-29",
        "2026-02-30",
        "-001-01-01",
        "+001-01-01",
        "2026-+1-01",
        "2026--1-01",
        "2026-01-+1",
        "2026-01--1",
        "2026-1-01",
        "2026-01-1",
        "٢٠٢٦-09-06",
        " 2026-09-06",
    ] {
        assert!(validate_date(invalid).is_err(), "{invalid}");
    }
    validate_timestamp("2026-09-06T23:59:59.999Z").unwrap();
    for invalid in [
        "2026-09-06T24:00:00.000Z",
        "2026-09-06T10:60:00.000Z",
        "2026-09-06T10:00:60.000Z",
        "2026-09-06T10:00:00.00Z",
        "2026-09-06T+1:00:00.000Z",
        "2026-09-06T-1:00:00.000Z",
        "2026-09-06T10:+0:00.000Z",
        "2026-09-06T10:-0:00.000Z",
        "2026-09-06T10:00:+0.000Z",
        "2026-09-06T10:00:-0.000Z",
        "2026-09-06T10:00:00.+00Z",
        "2026-09-06T10:00:00.-00Z",
        "2026-09-06T10:00:00.000+00:00",
        "٢٠٢٦-09-06T10:00:00.000Z",
    ] {
        assert!(validate_timestamp(invalid).is_err(), "{invalid}");
    }
}

#[test]
fn safe_integer_validation_uses_numeric_semantics_not_number_storage() {
    for value in [
        json!(0),
        json!(0.0),
        serde_json::from_str("-0.0").unwrap(),
        json!(1),
        json!(1.0),
    ] {
        validate_safe_integer(&value, 0, i64::MAX).unwrap();
    }
    assert!(validate_safe_integer(&json!(1.5), 0, i64::MAX).is_err());
    validate_safe_integer(&json!(9_007_199_254_740_991_i64), 0, i64::MAX).unwrap();
    assert!(validate_safe_integer(&json!(9_007_199_254_740_992_i64), 0, i64::MAX).is_err());
    assert!(validate_safe_integer(&json!(-1), 0, i64::MAX).is_err());
    assert!(validate_safe_integer(&json!(2_147_483_648_i64), 1, i32::MAX as i64).is_err());
}

fn native_record(fixture: &Value, updated_at: &str) -> Value {
    let mut map = fixture["semanticValues"]["recordChanged"]
        .as_object()
        .unwrap()
        .clone();
    map.insert("id".into(), json!("record-1"));
    map.insert("createdAt".into(), json!("2026-09-06T10:00:00.000Z"));
    map.insert("updatedAt".into(), json!(updated_at));
    map.insert("rev".into(), json!("9223372036854775807"));
    map.insert("revActor".into(), json!(""));
    Value::Object(map)
}

#[test]
fn semantic_profile_is_native_strict_and_metadata_is_not_semantic() {
    let fixture = fixture();
    let a = native_record(&fixture, "2026-09-06T10:00:00.000Z");
    let b = native_record(&fixture, "2026-09-06T10:00:01.000Z");
    validate_native_entity(&json!(["record", "record-1"]), &a).unwrap();
    validate_native_entity(&json!(["record", "record-1"]), &b).unwrap();
    assert_eq!(
        canonical_semantic_value(&a).unwrap(),
        canonical_semantic_value(&b).unwrap()
    );
    let mut movie = b.clone();
    let movie_map = movie.as_object_mut().unwrap();
    movie_map.insert("tmdbMediaKind".into(), json!("movie"));
    movie_map.insert("tmdbId".into(), json!("9007199254740993"));
    movie_map.insert("seriesRecordKind".into(), json!("single-work"));
    validate_native_entity(&json!(["record", "record-1"]), &movie).unwrap();
    assert!(validate_native_entity(
        &json!(["record", "record-1"]),
        &native_record(&fixture, "2026-09-06T18:00:00.000+08:00")
    )
    .is_err());

    let episode_id = sha256_hex(b"episode-completion:v1\0record-1\x007");
    let episode = json!({
        "id": episode_id, "recordId": "record-1", "episodeNumber": 7,
        "completedAt": null, "createdAt": "2026-09-06T10:00:00.000Z",
        "updatedAt": "2026-09-06T10:00:00.000Z", "rev": "0", "revActor": ""
    });
    validate_native_entity(&json!(["episode-completion", "record-1", 7]), &episode).unwrap();
    let tombstone = json!({
        "id": episode["id"], "recordId": "record-1", "episodeNumber": 7,
        "deletedAt": "2026-09-06T10:00:00.000Z", "rev": "1", "revActor": ""
    });
    validate_tombstone(
        "episode-completion",
        &json!(["episode-completion", "record-1", 7]),
        &tombstone,
    )
    .unwrap();
}

#[test]
fn native_entity_and_tombstone_reject_illegal_identity_before_hashing() {
    for key in [
        json!(["record", "record-1", "extra"]),
        json!(["collection", ""]),
        json!(["episode-completion", "record-1", -1]),
        json!(["episode-completion", "record-1", 0]),
        json!(["episode-completion", "record-1", 2_147_483_648_i64]),
        json!(["collection-member", "", "record-1"]),
        json!(["collection-member", "collection-1", ""]),
    ] {
        assert!(validate_entity_key(&key).is_err());
    }

    let illegal_episode_id = sha256_hex(b"episode-completion:v1\0record-1\x000");
    let illegal_episode = json!({
        "id": illegal_episode_id, "recordId": "record-1", "episodeNumber": 0,
        "completedAt": null, "createdAt": "2026-09-06T10:00:00.000Z",
        "updatedAt": "2026-09-06T10:00:00.000Z", "rev": "0", "revActor": ""
    });
    assert!(validate_native_entity(
        &json!(["episode-completion", "record-1", 0]),
        &illegal_episode
    )
    .is_err());
    let illegal_tombstone = json!({
        "id": illegal_episode["id"], "recordId": "record-1", "episodeNumber": 0,
        "deletedAt": "2026-09-06T10:00:00.000Z", "rev": "0", "revActor": ""
    });
    assert!(validate_tombstone(
        "episode-completion",
        &json!(["episode-completion", "record-1", 0]),
        &illegal_tombstone,
    )
    .is_err());
    let illegal_member_id = sha256_hex(b"collection-member:v1\0\0record-1");
    assert!(validate_native_entity(
        &json!(["collection-member", "", "record-1"]),
        &json!({
            "id": illegal_member_id, "collectionId": "", "recordId": "record-1",
            "position": "0", "sourceKind": "manual",
            "createdAt": "2026-09-06T10:00:00.000Z",
            "updatedAt": "2026-09-06T10:00:00.000Z", "rev": "0", "revActor": ""
        }),
    )
    .is_err());
}

#[test]
fn full_entity_validation_rejects_noncanonical_date_timestamp_fields() {
    let fixture = fixture();
    for start_date in ["-001-01-01", "+001-01-01", "2026-+1-01", "٢٠٢٦-09-06"] {
        let mut value = native_record(&fixture, "2026-09-06T10:00:00.000Z");
        value["startDate"] = json!(start_date);
        assert!(validate_native_entity(&json!(["record", "record-1"]), &value).is_err());
    }
    for updated_at in [
        "2026-09-06T+1:00:00.000Z",
        "2026-09-06T10:+0:00.000Z",
        "2026-09-06T10:00:+0.000Z",
        "2026-09-06T10:00:60.000Z",
        "2026-09-06T10:00:00.000+00:00",
    ] {
        let value = native_record(&fixture, updated_at);
        assert!(validate_native_entity(&json!(["record", "record-1"]), &value).is_err());
    }
}

fn bootstrap_case(
    record_count: usize,
    collection_count: usize,
    member_count: usize,
    episode_count: usize,
) -> Vec<BootstrapEntity> {
    let fixture = fixture();
    let mut values = Vec::new();
    for index in 0..record_count {
        let id = format!("r{index:04}");
        let mut value = native_record(&fixture, "2026-09-06T10:00:00.000Z");
        let map = value.as_object_mut().unwrap();
        map.insert("id".into(), json!(id));
        map.insert("totalEpisodes".into(), json!(10_000));
        values.push(BootstrapEntity {
            entity_type: "record".into(),
            entity_key: json!(["record", id]),
            value,
        });
    }
    for index in 0..collection_count {
        let id = format!("c{index}");
        values.push(BootstrapEntity {
            entity_type: "collection".into(),
            entity_key: json!(["collection", id]),
            value: json!({
                "id": id, "name": format!("Collection-c{index}"),
                "normalizedName": format!("collection-c{index}"), "description": null,
                "sourceKind": "manual", "sourceKey": null, "collectionKind": "manual",
                "orderMode": "manual", "createdAt": "2026-09-06T10:00:00.000Z",
                "updatedAt": "2026-09-06T10:00:00.000Z", "rev": "0", "revActor": ""
            }),
        });
    }
    for index in 0..member_count {
        let collection_id = format!("c{}", index % collection_count);
        let record_id = format!("r{:04}", index % record_count);
        let id =
            sha256_hex(format!("collection-member:v1\0{collection_id}\0{record_id}").as_bytes());
        values.push(BootstrapEntity {
            entity_type: "collection-member".into(),
            entity_key: json!(["collection-member", collection_id, record_id]),
            value: json!({
                "id": id, "collectionId": collection_id, "recordId": record_id,
                "position": index.to_string(), "sourceKind": "manual",
                "createdAt": "2026-09-06T10:00:00.000Z",
                "updatedAt": "2026-09-06T10:00:00.000Z", "rev": "0", "revActor": ""
            }),
        });
    }
    for index in 0..episode_count {
        let record_id = format!("r{:04}", index % record_count);
        let episode_number = index / record_count + 1;
        let id =
            sha256_hex(format!("episode-completion:v1\0{record_id}\0{episode_number}").as_bytes());
        values.push(BootstrapEntity {
            entity_type: "episode-completion".into(),
            entity_key: json!(["episode-completion", record_id, episode_number]),
            value: json!({
                "id": id, "recordId": record_id, "episodeNumber": episode_number,
                "completedAt": null, "createdAt": "2026-09-06T10:00:00.000Z",
                "updatedAt": "2026-09-06T10:00:00.000Z", "rev": "0", "revActor": ""
            }),
        });
    }
    values
}

fn chunk_lengths(chunks: &[Vec<BootstrapEntity>]) -> Vec<usize> {
    chunks.iter().map(Vec::len).collect()
}

fn assignment(input: &[BootstrapEntity]) -> Vec<(char, usize, Value)> {
    let plan = build_bootstrap_plan_v1(input).unwrap();
    plan.stage_a_chunks
        .iter()
        .enumerate()
        .flat_map(|(chunk, entities)| {
            entities
                .iter()
                .map(move |entity| ('A', chunk, entity.entity_key.clone()))
        })
        .chain(
            plan.stage_b_chunks
                .iter()
                .enumerate()
                .flat_map(|(chunk, entities)| {
                    entities
                        .iter()
                        .map(move |entity| ('B', chunk, entity.entity_key.clone()))
                }),
        )
        .collect()
}

#[test]
fn bootstrap_required_chunks_and_permutation_invariance() {
    let cases = [
        (bootstrap_case(300, 1, 300, 0), vec![256, 45], vec![256, 44]),
        (bootstrap_case(600, 1, 1, 0), vec![256, 256, 89], vec![1]),
        (
            bootstrap_case(300, 0, 0, 1000),
            vec![256, 44],
            vec![256, 256, 256, 232],
        ),
    ];
    for (input, expected_a, expected_b) in cases {
        let plan = build_bootstrap_plan_v1(&input).unwrap();
        assert_eq!(chunk_lengths(&plan.stage_a_chunks), expected_a);
        assert_eq!(chunk_lengths(&plan.stage_b_chunks), expected_b);
    }
    let forward = bootstrap_case(300, 1, 300, 60);
    let expected = assignment(&forward);
    let mut reverse = forward.clone();
    reverse.reverse();
    assert_eq!(
        build_bootstrap_plan_v1(&reverse).unwrap(),
        build_bootstrap_plan_v1(&forward).unwrap()
    );
    assert_eq!(assignment(&reverse), expected);
    let mut rotated = forward.clone();
    rotated.rotate_left(137);
    assert_eq!(assignment(&rotated), expected);
}

#[test]
fn bootstrap_and_conflict_codecs_fail_closed_on_malformed_facts() {
    assert!(build_bootstrap_plan_v1(&[BootstrapEntity {
        entity_type: "episode-completion".into(),
        entity_key: json!(["episode-completion", "missing", 1]),
        value: json!({}),
    }])
    .is_err());
    let fixture = fixture();
    let malformed = vec![
        EntityConflictAlternativeInput {
            commit_ref: reference(&fixture, "RA"),
            semantic_state: json!({"state": "live", "value": fixture["semanticValues"]["collectionAlpha"]}),
            changed_fields: vec![],
            base_frontier: vec![reference(&fixture, "P")],
        },
        EntityConflictAlternativeInput {
            commit_ref: reference(&fixture, "RB"),
            semantic_state: json!({"state": "live", "value": {}}),
            changed_fields: vec![],
            base_frontier: vec![reference(&fixture, "Q")],
        },
    ];
    assert!(build_entity_conflict_core_v1(
        json!(["collection", "c-live"]),
        EntityConflictKind::DifferentBase,
        malformed,
    )
    .is_err());
    assert!(build_relation_conflict_core_v1(
        RelationConflictKind::EpisodeExceedsTotal,
        json!({"recordId": "r1", "episodeNumber": 12, "totalEpisodes": 10}),
        vec![
            RelationParticipant {
                entity_key: json!(["record", "wrong"]),
                provenance_frontier: vec![reference(&fixture, "RA")],
            },
            RelationParticipant {
                entity_key: json!(["episode-completion", "r1", 12]),
                provenance_frontier: vec![reference(&fixture, "RB")],
            },
        ],
    )
    .is_err());
}

#[test]
fn bootstrap_validates_complete_current_state_before_ordering() {
    let valid = bootstrap_case(1, 1, 1, 1);
    build_bootstrap_plan_v1(&valid).unwrap();

    let mut unknown = valid[0].clone();
    unknown.entity_type = "future-type".into();
    assert!(build_bootstrap_plan_v1(&[unknown]).is_err());
    for invalid_value in [
        json!({}),
        json!({"state": "tombstone"}),
        json!({"state": "unresolved", "alternatives": []}),
    ] {
        let mut invalid = valid[0].clone();
        invalid.value = invalid_value;
        assert!(build_bootstrap_plan_v1(&[invalid]).is_err());
    }
    let mut mismatch = valid[0].clone();
    mismatch.entity_key = json!(["record", "wrong"]);
    assert!(build_bootstrap_plan_v1(&[mismatch]).is_err());
    let mut malformed = valid[0].clone();
    malformed.entity_key = json!(["record", "r0000", "extra"]);
    assert!(build_bootstrap_plan_v1(&[malformed]).is_err());
    let mut invalid_id = valid[3].clone();
    invalid_id.value["id"] = json!("0".repeat(64));
    assert!(build_bootstrap_plan_v1(&[invalid_id]).is_err());
    assert!(build_bootstrap_plan_v1(&[valid[3].clone()]).is_err());

    let mut episode_two = valid[3].clone();
    episode_two.entity_key = json!(["episode-completion", "r0000", 2]);
    episode_two.value["episodeNumber"] = json!(2);
    episode_two.value["id"] = json!(sha256_hex(b"episode-completion:v1\0r0000\x002"));
    let mut record_one = valid[0].clone();
    record_one.value["totalEpisodes"] = json!(1);
    assert!(build_bootstrap_plan_v1(&[record_one, episode_two]).is_err());
    let mut movie_record = valid[0].clone();
    movie_record.value["mediaType"] = json!("电影");
    assert!(build_bootstrap_plan_v1(&[movie_record, valid[3].clone()]).is_err());
    let mut unknown_total = valid[0].clone();
    unknown_total.value["totalEpisodes"] = Value::Null;
    assert!(build_bootstrap_plan_v1(&[unknown_total, valid[3].clone()]).is_err());
    assert!(build_bootstrap_plan_v1(&[valid[0].clone(), valid[0].clone()]).is_err());
    assert!(build_bootstrap_plan_v1(&[valid[0].clone(), valid[2].clone()]).is_err());
}

#[test]
fn every_relation_participant_requires_its_own_provenance() {
    let fixture = fixture();
    let cases = [
        (
            RelationConflictKind::CollectionDeletedMemberLive,
            json!({"collectionId": "c1", "recordId": "r1", "collectionState": "tombstone", "memberState": "live"}),
            [
                json!(["collection", "c1"]),
                json!(["collection-member", "c1", "r1"]),
            ],
        ),
        (
            RelationConflictKind::RecordDeletedMemberLive,
            json!({"collectionId": "c1", "recordId": "r1", "recordState": "tombstone", "memberState": "live"}),
            [
                json!(["record", "r1"]),
                json!(["collection-member", "c1", "r1"]),
            ],
        ),
        (
            RelationConflictKind::RecordDeletedEpisodeLive,
            json!({"recordId": "r1", "episodeNumber": 12, "recordState": "tombstone", "episodeState": "live"}),
            [
                json!(["record", "r1"]),
                json!(["episode-completion", "r1", 12]),
            ],
        ),
        (
            RelationConflictKind::EpisodeExceedsTotal,
            json!({"recordId": "r1", "episodeNumber": 12, "totalEpisodes": 10}),
            [
                json!(["record", "r1"]),
                json!(["episode-completion", "r1", 12]),
            ],
        ),
    ];
    for (kind, facts, keys) in cases {
        for empty in [[true, false], [false, true], [true, true]] {
            let participants = keys
                .iter()
                .enumerate()
                .map(|(index, key)| RelationParticipant {
                    entity_key: key.clone(),
                    provenance_frontier: if empty[index] {
                        Vec::new()
                    } else {
                        vec![reference(&fixture, if index == 0 { "RA" } else { "RB" })]
                    },
                })
                .collect();
            assert!(build_relation_conflict_core_v1(kind, facts.clone(), participants).is_err());
        }
    }
}

#[test]
fn relation_entity_key_equality_canonicalizes_all_integer_float_forms() {
    let fixture = fixture();
    let variants = [
        ("12", "12"),
        ("12.0", "12"),
        ("12", "12.0"),
        ("12.0", "12.0"),
    ];
    let mut expected_bytes: Option<Vec<u8>> = None;
    let mut expected_id: Option<String> = None;
    let expected_key_bytes =
        canonical_entity_key_bytes(&json!(["episode-completion", "r1", 12])).unwrap();
    for (fact_episode, key_episode) in variants {
        let facts: Value = serde_json::from_str(&format!(
            "{{\"recordId\":\"r1\",\"episodeNumber\":{fact_episode},\"totalEpisodes\":10}}"
        ))
        .unwrap();
        let episode_key: Value =
            serde_json::from_str(&format!("[\"episode-completion\",\"r1\",{key_episode}]"))
                .unwrap();
        assert_eq!(
            canonical_entity_key_bytes(&episode_key).unwrap(),
            expected_key_bytes
        );
        let core = build_relation_conflict_core_v1(
            RelationConflictKind::EpisodeExceedsTotal,
            facts,
            vec![
                RelationParticipant {
                    entity_key: json!(["record", "r1"]),
                    provenance_frontier: vec![reference(&fixture, "RA")],
                },
                RelationParticipant {
                    entity_key: episode_key,
                    provenance_frontier: vec![reference(&fixture, "RB")],
                },
            ],
        )
        .unwrap();
        let bytes = jcs_bytes(&core).unwrap();
        let id = relation_conflict_id_v1(&core).unwrap();
        if let Some(expected) = &expected_bytes {
            assert_eq!(&bytes, expected);
            assert_eq!(&id, expected_id.as_ref().unwrap());
        } else {
            expected_bytes = Some(bytes);
            expected_id = Some(id);
        }
    }
    let frozen = fixture["relationCases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["name"] == "episode-exceeds-total")
        .unwrap();
    assert_eq!(
        expected_bytes.as_deref().unwrap(),
        frozen["expectedJcs"].as_str().unwrap().as_bytes()
    );
    assert_eq!(
        expected_id.as_deref().unwrap(),
        frozen["expectedSha256"].as_str().unwrap()
    );

    for invalid in ["12.5", "0", "2147483648"] {
        for (fact_episode, key_episode) in [(invalid, "12"), ("12", invalid)] {
            let facts: Value = serde_json::from_str(&format!(
                "{{\"recordId\":\"r1\",\"episodeNumber\":{fact_episode},\"totalEpisodes\":10}}"
            ))
            .unwrap();
            let episode_key: Value =
                serde_json::from_str(&format!("[\"episode-completion\",\"r1\",{key_episode}]"))
                    .unwrap();
            assert!(build_relation_conflict_core_v1(
                RelationConflictKind::EpisodeExceedsTotal,
                facts,
                vec![
                    RelationParticipant {
                        entity_key: json!(["record", "r1"]),
                        provenance_frontier: vec![reference(&fixture, "RA")],
                    },
                    RelationParticipant {
                        entity_key: episode_key,
                        provenance_frontier: vec![reference(&fixture, "RB")],
                    },
                ],
            )
            .is_err());
        }
    }
}

#[test]
fn comparator_properties_and_jcs_round_trip() {
    let refs = (0..60)
        .map(|index| CommitRef {
            writer_id: format!("{:08}-0000-4000-8000-{:012}", index % 4, index % 7),
            writer_seq: ((index * 17) % 41 + 1).to_string(),
            commit_id: format!("{:08}-0000-4000-8000-{:012}", 59 - index, index),
            content_hash: format!("{index:064x}"),
        })
        .collect::<Vec<_>>();
    for a in &refs {
        for b in &refs {
            assert_eq!(
                compare_commit_ref_v1(a, b),
                compare_commit_ref_v1(b, a).reverse()
            );
        }
    }
    let mut sorted = refs;
    sorted.sort_by(compare_commit_ref_v1);
    for triple in sorted.windows(3) {
        assert_ne!(
            compare_commit_ref_v1(&triple[0], &triple[2]),
            Ordering::Greater
        );
    }
    let mut keys = [
        json!(["record", "z"]),
        json!(["collection", "a"]),
        json!(["episode-completion", "r", 10]),
        json!(["episode-completion", "r", 2]),
    ];
    keys.sort_by(compare_entity_key_v1);
    assert!(keys
        .windows(2)
        .all(|pair| compare_entity_key_v1(&pair[0], &pair[1]) != Ordering::Greater));
    let value =
        json!({"z": [null, "", "e\u{301}", "é"], "a": {"integer": "9007199254740993", "safe": 11}});
    let encoded = jcs_bytes(&value).unwrap();
    let reparsed: Value = serde_json::from_slice(&encoded).unwrap();
    assert_eq!(jcs_bytes(&reparsed).unwrap(), encoded);
}
