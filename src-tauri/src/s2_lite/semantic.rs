use super::canonical::{
    canonical_collection_name_v1, normalize_collection_name_v1, parse_int64_decimal, sha256_hex,
    validate_date, validate_entity_key, validate_float64, validate_safe_integer,
    validate_timestamp, ProtocolError, Result,
};
use serde_json::{Map, Value};
use std::collections::BTreeSet;

const RECORD_FIELDS: &[&str] = &[
    "id",
    "originalName",
    "chineseName",
    "progress",
    "totalEpisodes",
    "episodeTrackingEnabled",
    "nextEpisode",
    "movieProgress",
    "movieDuration",
    "releaseYear",
    "posterPath",
    "status",
    "platform",
    "rating",
    "startDate",
    "endDate",
    "notes",
    "createdAt",
    "updatedAt",
    "imdbId",
    "isLocked",
    "genres",
    "originCountry",
    "imdbRating",
    "tmdbStatus",
    "interestLevel",
    "episodeRuntime",
    "mediaType",
    "contentTags",
    "tmdbMediaKind",
    "tmdbId",
    "tmdbParentId",
    "tmdbSeasonNumber",
    "seriesRecordKind",
    "rev",
    "revActor",
];
const EPISODE_FIELDS: &[&str] = &[
    "id",
    "recordId",
    "episodeNumber",
    "completedAt",
    "createdAt",
    "updatedAt",
    "rev",
    "revActor",
];
const COLLECTION_FIELDS: &[&str] = &[
    "id",
    "name",
    "normalizedName",
    "description",
    "sourceKind",
    "sourceKey",
    "collectionKind",
    "orderMode",
    "createdAt",
    "updatedAt",
    "rev",
    "revActor",
];
const MEMBER_FIELDS: &[&str] = &[
    "id",
    "collectionId",
    "recordId",
    "position",
    "sourceKind",
    "createdAt",
    "updatedAt",
    "rev",
    "revActor",
];

pub fn business_field_order(entity_type: &str) -> Option<&'static [&'static str]> {
    match entity_type {
        "record" => Some(&[
            "originalName",
            "chineseName",
            "progress",
            "totalEpisodes",
            "episodeTrackingEnabled",
            "nextEpisode",
            "movieProgress",
            "movieDuration",
            "releaseYear",
            "posterPath",
            "status",
            "platform",
            "rating",
            "startDate",
            "endDate",
            "notes",
            "imdbId",
            "isLocked",
            "genres",
            "originCountry",
            "imdbRating",
            "tmdbStatus",
            "interestLevel",
            "episodeRuntime",
            "mediaType",
            "contentTags",
            "tmdbMediaKind",
            "tmdbId",
            "tmdbParentId",
            "tmdbSeasonNumber",
            "seriesRecordKind",
        ]),
        "episode-completion" => Some(&["recordId", "episodeNumber", "completedAt"]),
        "collection" => Some(&[
            "name",
            "normalizedName",
            "description",
            "sourceKind",
            "sourceKey",
            "collectionKind",
            "orderMode",
        ]),
        "collection-member" => Some(&["collectionId", "recordId", "position", "sourceKind"]),
        _ => None,
    }
}

fn object(value: &Value) -> Result<&Map<String, Value>> {
    value
        .as_object()
        .ok_or(ProtocolError("invalid_entity_value"))
}

fn field<'a>(value: &'a Map<String, Value>, key: &str) -> Result<&'a Value> {
    value.get(key).ok_or(ProtocolError("invalid_entity_fields"))
}

fn exact_fields(value: &Map<String, Value>, fields: &[&str]) -> Result<()> {
    let actual = value.keys().map(String::as_str).collect::<BTreeSet<_>>();
    let expected = fields.iter().copied().collect::<BTreeSet<_>>();
    if actual != expected {
        return Err(ProtocolError("invalid_entity_fields"));
    }
    Ok(())
}

fn boundary_s2_whitespace(value: &str) -> bool {
    let is_s2 =
        |character: char| matches!(character as u32, 0x09 | 0x0a | 0x0b | 0x0c | 0x0d | 0x20);
    value.chars().next().is_some_and(is_s2) || value.chars().next_back().is_some_and(is_s2)
}

fn text(
    value: &Value,
    maximum: usize,
    allow_empty: bool,
    preserve_whitespace: bool,
) -> Result<&str> {
    let string = value.as_str().ok_or(ProtocolError("invalid_string"))?;
    let length = string.chars().count();
    if string.contains('\0')
        || (!allow_empty && length == 0)
        || length > maximum
        || (!preserve_whitespace && boundary_s2_whitespace(string))
    {
        return Err(ProtocolError("invalid_string"));
    }
    Ok(string)
}

fn id(value: &Value) -> Result<&str> {
    text(value, 512, false, false)
}

fn nullable<'a, T>(
    value: &'a Value,
    validate: impl FnOnce(&'a Value) -> Result<T>,
) -> Result<Option<T>> {
    if value.is_null() {
        Ok(None)
    } else {
        validate(value).map(Some)
    }
}

fn enum_value<'a>(value: &'a Value, allowed: &[&str]) -> Result<&'a str> {
    let string = value.as_str().ok_or(ProtocolError("invalid_enum"))?;
    if !allowed.contains(&string) {
        return Err(ProtocolError("invalid_enum"));
    }
    Ok(string)
}

fn timestamp(value: &Value) -> Result<&str> {
    let string = value.as_str().ok_or(ProtocolError("invalid_timestamp"))?;
    validate_timestamp(string)?;
    Ok(string)
}

fn date(value: &Value) -> Result<&str> {
    let string = value.as_str().ok_or(ProtocolError("invalid_date"))?;
    validate_date(string)?;
    Ok(string)
}

fn int64(value: &Value, minimum: i64) -> Result<i64> {
    parse_int64_decimal(
        value
            .as_str()
            .ok_or(ProtocolError("invalid_int64_decimal_string"))?,
        minimum,
        i64::MAX,
    )
}

fn validate_record(map: &Map<String, Value>) -> Result<()> {
    exact_fields(map, RECORD_FIELDS)?;
    id(field(map, "id")?)?;
    let original = text(field(map, "originalName")?, 4096, true, false)?;
    let chinese = text(field(map, "chineseName")?, 4096, true, false)?;
    if original.is_empty() && chinese.is_empty() {
        return Err(ProtocolError("record_title_required"));
    }
    text(field(map, "progress")?, 4096, true, false)?;
    let total = nullable(field(map, "totalEpisodes")?, |value| {
        validate_safe_integer(value, 1, i32::MAX as i64)
    })?;
    let tracking = field(map, "episodeTrackingEnabled")?
        .as_bool()
        .ok_or(ProtocolError("invalid_boolean"))?;
    let next = nullable(field(map, "nextEpisode")?, |value| {
        validate_safe_integer(value, 1, i32::MAX as i64)
    })?;
    if next.is_some_and(|episode| !tracking || total.map_or(true, |count| episode > count)) {
        return Err(ProtocolError("invalid_next_episode"));
    }
    let progress = nullable(field(map, "movieProgress")?, |value| {
        validate_safe_integer(value, 0, i32::MAX as i64)
    })?;
    let duration = nullable(field(map, "movieDuration")?, |value| {
        validate_safe_integer(value, 1, i32::MAX as i64)
    })?;
    if progress.is_some_and(|position| duration.is_some_and(|length| position > length)) {
        return Err(ProtocolError("invalid_movie_progress"));
    }
    nullable(field(map, "releaseYear")?, |value| {
        let string = value
            .as_str()
            .ok_or(ProtocolError("invalid_release_year"))?;
        if string.len() != 4
            || string == "0000"
            || !string.as_bytes().iter().all(u8::is_ascii_digit)
        {
            return Err(ProtocolError("invalid_release_year"));
        }
        Ok(())
    })?;
    nullable(field(map, "posterPath")?, |value| {
        text(value, 8192, false, false)
    })?;
    enum_value(field(map, "status")?, &["已看", "在看", "未看"])?;
    text(field(map, "platform")?, 1024, true, false)?;
    nullable(field(map, "rating")?, |value| {
        validate_safe_integer(value, 1, 10)
    })?;
    let start = nullable(field(map, "startDate")?, date)?;
    let end = nullable(field(map, "endDate")?, date)?;
    if start.is_some_and(|from| end.is_some_and(|to| from > to)) {
        return Err(ProtocolError("invalid_date_range"));
    }
    text(field(map, "notes")?, 1_048_576, true, true)?;
    timestamp(field(map, "createdAt")?)?;
    nullable(field(map, "updatedAt")?, timestamp)?;
    nullable(field(map, "imdbId")?, |value| {
        let string = value.as_str().ok_or(ProtocolError("invalid_imdb_id"))?;
        if !string.starts_with("tt")
            || string.len() < 3
            || string.len() > 20
            || !string.as_bytes()[2..].iter().all(u8::is_ascii_digit)
        {
            return Err(ProtocolError("invalid_imdb_id"));
        }
        Ok(())
    })?;
    nullable(field(map, "isLocked")?, |value| {
        value.as_bool().ok_or(ProtocolError("invalid_boolean"))
    })?;
    nullable(field(map, "genres")?, |value| {
        text(value, 16_384, false, false)
    })?;
    nullable(field(map, "originCountry")?, |value| {
        text(value, 1024, false, false)
    })?;
    nullable(field(map, "imdbRating")?, |value| {
        validate_float64(value, 0.0, 10.0)
    })?;
    nullable(field(map, "tmdbStatus")?, |value| {
        text(value, 1024, false, false)
    })?;
    nullable(field(map, "interestLevel")?, |value| {
        validate_safe_integer(value, 1, 5)
    })?;
    nullable(field(map, "episodeRuntime")?, |value| {
        validate_safe_integer(value, 1, i32::MAX as i64)
    })?;
    enum_value(
        field(map, "mediaType")?,
        &["电影", "剧集", "纪录片", "综艺", "动画"],
    )?;
    nullable(field(map, "contentTags")?, |value| {
        text(value, 16_384, false, false)
    })?;
    let media_kind = nullable(field(map, "tmdbMediaKind")?, |value| {
        enum_value(value, &["movie", "tv", "tv-season"])
    })?;
    let tmdb_id = nullable(field(map, "tmdbId")?, |value| int64(value, 1))?;
    let parent_id = nullable(field(map, "tmdbParentId")?, |value| int64(value, 1))?;
    let season = nullable(field(map, "tmdbSeasonNumber")?, |value| {
        validate_safe_integer(value, 0, i32::MAX as i64)
    })?;
    let record_kind = nullable(field(map, "seriesRecordKind")?, |value| {
        enum_value(value, &["season", "whole-series", "single-work"])
    })?;
    let empty = media_kind.is_none()
        && tmdb_id.is_none()
        && parent_id.is_none()
        && season.is_none()
        && record_kind.is_none();
    let movie = media_kind == Some("movie")
        && tmdb_id.is_some()
        && parent_id.is_none()
        && season.is_none()
        && record_kind == Some("single-work");
    let tv = media_kind == Some("tv")
        && tmdb_id.is_some()
        && parent_id.is_none()
        && season.is_none()
        && record_kind == Some("whole-series");
    let tv_season = media_kind == Some("tv-season")
        && tmdb_id.is_some()
        && parent_id.is_some()
        && season.is_some_and(|value| value > 0)
        && record_kind == Some("season");
    if !(empty || movie || tv || tv_season) {
        return Err(ProtocolError("invalid_tmdb_tuple"));
    }
    int64(field(map, "rev")?, 0)?;
    text(field(map, "revActor")?, 512, true, true)?;
    Ok(())
}

fn validate_episode(map: &Map<String, Value>) -> Result<()> {
    exact_fields(map, EPISODE_FIELDS)?;
    let raw_id = field(map, "id")?
        .as_str()
        .ok_or(ProtocolError("invalid_episode_id"))?;
    if raw_id.len() != 64
        || !raw_id
            .as_bytes()
            .iter()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(byte))
    {
        return Err(ProtocolError("invalid_episode_id"));
    }
    id(field(map, "recordId")?)?;
    validate_safe_integer(field(map, "episodeNumber")?, 1, i32::MAX as i64)?;
    nullable(field(map, "completedAt")?, timestamp)?;
    timestamp(field(map, "createdAt")?)?;
    timestamp(field(map, "updatedAt")?)?;
    int64(field(map, "rev")?, 0)?;
    text(field(map, "revActor")?, 512, true, true)?;
    Ok(())
}

fn validate_collection(map: &Map<String, Value>) -> Result<()> {
    exact_fields(map, COLLECTION_FIELDS)?;
    id(field(map, "id")?)?;
    let name = field(map, "name")?
        .as_str()
        .ok_or(ProtocolError("invalid_collection_name"))?;
    if canonical_collection_name_v1(name)? != name {
        return Err(ProtocolError("invalid_collection_name"));
    }
    let normalized_name = normalize_collection_name_v1(name);
    if field(map, "normalizedName")?.as_str() != Some(normalized_name.as_str()) {
        return Err(ProtocolError("invalid_normalized_name"));
    }
    nullable(field(map, "description")?, |value| {
        let description = text(value, 500, false, false)?;
        if description
            .chars()
            .any(|character| character as u32 <= 0x1f || character as u32 == 0x7f)
        {
            return Err(ProtocolError("invalid_collection_description"));
        }
        Ok(())
    })?;
    let source = enum_value(
        field(map, "sourceKind")?,
        &["manual", "tmdb-movie-collection", "tmdb-tv-show"],
    )?;
    let source_key = nullable(field(map, "sourceKey")?, |value| {
        text(value, 2048, false, false)
    })?;
    let kind = enum_value(
        field(map, "collectionKind")?,
        &["manual", "tv-series", "movie-series", "universe"],
    )?;
    let order = enum_value(field(map, "orderMode")?, &["manual", "chronological"])?;
    if (source == "manual" && source_key.is_some())
        || (source != "manual" && source_key.is_none())
        || (source == "tmdb-tv-show" && (kind != "tv-series" || order != "chronological"))
        || (source == "tmdb-movie-collection"
            && (kind != "movie-series" || order != "chronological"))
    {
        return Err(ProtocolError("invalid_collection_source"));
    }
    timestamp(field(map, "createdAt")?)?;
    timestamp(field(map, "updatedAt")?)?;
    int64(field(map, "rev")?, 0)?;
    text(field(map, "revActor")?, 512, true, true)?;
    Ok(())
}

fn validate_member(map: &Map<String, Value>) -> Result<()> {
    exact_fields(map, MEMBER_FIELDS)?;
    let raw_id = field(map, "id")?
        .as_str()
        .ok_or(ProtocolError("invalid_member_id"))?;
    if raw_id.len() != 64
        || !raw_id
            .as_bytes()
            .iter()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(byte))
    {
        return Err(ProtocolError("invalid_member_id"));
    }
    id(field(map, "collectionId")?)?;
    id(field(map, "recordId")?)?;
    int64(field(map, "position")?, 0)?;
    enum_value(field(map, "sourceKind")?, &["manual", "tmdb"])?;
    timestamp(field(map, "createdAt")?)?;
    timestamp(field(map, "updatedAt")?)?;
    int64(field(map, "rev")?, 0)?;
    text(field(map, "revActor")?, 512, true, true)?;
    Ok(())
}

pub fn validate_native_semantic_value(entity_type: &str, value: &Value) -> Result<()> {
    let map = object(value)?;
    match entity_type {
        "record" => validate_record(map),
        "episode-completion" => validate_episode(map),
        "collection" => validate_collection(map),
        "collection-member" => validate_member(map),
        _ => Err(ProtocolError("invalid_entity_type")),
    }
}

fn deterministic_id(domain: &str, components: &[String]) -> String {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(domain.as_bytes());
    for component in components {
        bytes.push(0);
        bytes.extend_from_slice(component.as_bytes());
    }
    sha256_hex(&bytes)
}

pub fn validate_native_entity(entity_key: &Value, value: &Value) -> Result<()> {
    validate_entity_key(entity_key)?;
    let key = entity_key
        .as_array()
        .ok_or(ProtocolError("invalid_entity_key"))?;
    let entity_type = key
        .first()
        .and_then(Value::as_str)
        .ok_or(ProtocolError("invalid_entity_key"))?;
    validate_native_semantic_value(entity_type, value)?;
    let map = object(value)?;
    match entity_type {
        "record" | "collection" => {
            if key.len() != 2 || field(map, "id")? != &key[1] {
                return Err(ProtocolError("entity_key_mismatch"));
            }
        }
        "episode-completion" => {
            if key.len() != 3
                || field(map, "recordId")? != &key[1]
                || validate_safe_integer(field(map, "episodeNumber")?, 1, i32::MAX as i64)?
                    != validate_safe_integer(&key[2], 1, i32::MAX as i64)?
            {
                return Err(ProtocolError("entity_key_mismatch"));
            }
            let expected = deterministic_id(
                "episode-completion:v1",
                &[
                    key[1]
                        .as_str()
                        .ok_or(ProtocolError("invalid_entity_key"))?
                        .to_string(),
                    validate_safe_integer(&key[2], 1, i32::MAX as i64)?.to_string(),
                ],
            );
            if field(map, "id")?.as_str() != Some(expected.as_str()) {
                return Err(ProtocolError("entity_key_mismatch"));
            }
        }
        "collection-member" => {
            if key.len() != 3
                || field(map, "collectionId")? != &key[1]
                || field(map, "recordId")? != &key[2]
            {
                return Err(ProtocolError("entity_key_mismatch"));
            }
            let expected = deterministic_id(
                "collection-member:v1",
                &[
                    key[1]
                        .as_str()
                        .ok_or(ProtocolError("invalid_entity_key"))?
                        .to_string(),
                    key[2]
                        .as_str()
                        .ok_or(ProtocolError("invalid_entity_key"))?
                        .to_string(),
                ],
            );
            if field(map, "id")?.as_str() != Some(expected.as_str()) {
                return Err(ProtocolError("entity_key_mismatch"));
            }
        }
        _ => return Err(ProtocolError("invalid_entity_type")),
    }
    Ok(())
}

pub fn canonical_semantic_value(value: &Value) -> Result<Value> {
    let mut result = object(value)?.clone();
    for field in ["id", "createdAt", "updatedAt", "rev", "revActor"] {
        result.remove(field);
    }
    Ok(Value::Object(result))
}

pub fn validate_tombstone(entity_type: &str, entity_key: &Value, value: &Value) -> Result<()> {
    validate_entity_key(entity_key)?;
    let map = object(value)?;
    let fields: &[&str] = match entity_type {
        "record" | "collection" => &["id", "deletedAt", "rev", "revActor"],
        "episode-completion" => &[
            "id",
            "recordId",
            "episodeNumber",
            "deletedAt",
            "rev",
            "revActor",
        ],
        "collection-member" => &[
            "id",
            "collectionId",
            "recordId",
            "deletedAt",
            "rev",
            "revActor",
        ],
        _ => return Err(ProtocolError("invalid_entity_type")),
    };
    exact_fields(map, fields)?;
    id(field(map, "id")?)?;
    timestamp(field(map, "deletedAt")?)?;
    int64(field(map, "rev")?, 0)?;
    text(field(map, "revActor")?, 512, true, true)?;
    let key = entity_key
        .as_array()
        .ok_or(ProtocolError("invalid_entity_key"))?;
    if key.first().and_then(Value::as_str) != Some(entity_type) {
        return Err(ProtocolError("entity_key_mismatch"));
    }
    match entity_type {
        "record" | "collection" => {
            if key.len() == 2 && field(map, "id")? == &key[1] {
                Ok(())
            } else {
                Err(ProtocolError("entity_key_mismatch"))
            }
        }
        "episode-completion" => {
            if key.len() != 3
                || field(map, "recordId")? != &key[1]
                || validate_safe_integer(field(map, "episodeNumber")?, 1, i32::MAX as i64)?
                    != validate_safe_integer(&key[2], 1, i32::MAX as i64)?
            {
                return Err(ProtocolError("entity_key_mismatch"));
            }
            let expected = deterministic_id(
                "episode-completion:v1",
                &[
                    key[1]
                        .as_str()
                        .ok_or(ProtocolError("invalid_entity_key"))?
                        .to_string(),
                    validate_safe_integer(&key[2], 1, i32::MAX as i64)?.to_string(),
                ],
            );
            if field(map, "id")?.as_str() == Some(expected.as_str()) {
                Ok(())
            } else {
                Err(ProtocolError("entity_key_mismatch"))
            }
        }
        "collection-member" => {
            if key.len() != 3
                || field(map, "collectionId")? != &key[1]
                || field(map, "recordId")? != &key[2]
            {
                return Err(ProtocolError("entity_key_mismatch"));
            }
            let expected = deterministic_id(
                "collection-member:v1",
                &[
                    key[1]
                        .as_str()
                        .ok_or(ProtocolError("invalid_entity_key"))?
                        .to_string(),
                    key[2]
                        .as_str()
                        .ok_or(ProtocolError("invalid_entity_key"))?
                        .to_string(),
                ],
            );
            if field(map, "id")?.as_str() == Some(expected.as_str()) {
                Ok(())
            } else {
                Err(ProtocolError("entity_key_mismatch"))
            }
        }
        _ => Err(ProtocolError("entity_key_mismatch")),
    }
}
