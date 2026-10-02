use super::types::{CommitDot, CommitRef, EntityKey};
use chrono::NaiveDate;
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::cmp::Ordering;
use uuid::{Variant, Version};

const MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;

pub const I64_MIN: i128 = i64::MIN as i128;
pub const I64_MAX: i128 = i64::MAX as i128;
pub const U64_MAX: u128 = u64::MAX as u128;

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ProtocolError(pub &'static str);

pub type Result<T> = std::result::Result<T, ProtocolError>;

pub fn jcs_bytes<T: Serialize>(value: &T) -> Result<Vec<u8>> {
    let value =
        serde_value::to_value(value).map_err(|_| ProtocolError("jcs_serialization_failed"))?;
    let mut output = String::new();
    write_jcs_value(&value, &mut output)?;
    Ok(output.into_bytes())
}

fn write_jcs_string(value: &str, output: &mut String) -> Result<()> {
    let encoded =
        serde_json::to_string(value).map_err(|_| ProtocolError("jcs_serialization_failed"))?;
    output.push_str(&encoded);
    Ok(())
}

fn write_jcs_float(value: f64, output: &mut String) -> Result<()> {
    if !value.is_finite() {
        return Err(ProtocolError("jcs_non_finite_number"));
    }
    if value == 0.0 {
        output.push('0');
    } else {
        output.push_str(ryu_js::Buffer::new().format_finite(value));
    }
    Ok(())
}

fn write_jcs_unsigned(value: u64, output: &mut String) -> Result<()> {
    if value > MAX_SAFE_INTEGER {
        return Err(ProtocolError("jcs_unsafe_integer"));
    }
    output.push_str(&value.to_string());
    Ok(())
}

fn write_jcs_signed(value: i64, output: &mut String) -> Result<()> {
    if value.unsigned_abs() > MAX_SAFE_INTEGER {
        return Err(ProtocolError("jcs_unsafe_integer"));
    }
    output.push_str(&value.to_string());
    Ok(())
}

fn compare_utf16(a: &str, b: &str) -> Ordering {
    a.encode_utf16().cmp(b.encode_utf16())
}

fn write_jcs_value(value: &serde_value::Value, output: &mut String) -> Result<()> {
    use serde_value::Value as SValue;
    match value {
        SValue::Bool(value) => output.push_str(if *value { "true" } else { "false" }),
        SValue::U8(value) => write_jcs_unsigned((*value).into(), output)?,
        SValue::U16(value) => write_jcs_unsigned((*value).into(), output)?,
        SValue::U32(value) => write_jcs_unsigned((*value).into(), output)?,
        SValue::U64(value) => write_jcs_unsigned(*value, output)?,
        SValue::I8(value) => write_jcs_signed((*value).into(), output)?,
        SValue::I16(value) => write_jcs_signed((*value).into(), output)?,
        SValue::I32(value) => write_jcs_signed((*value).into(), output)?,
        SValue::I64(value) => write_jcs_signed(*value, output)?,
        SValue::F32(value) => write_jcs_float(f64::from(*value), output)?,
        SValue::F64(value) => write_jcs_float(*value, output)?,
        SValue::Char(value) => write_jcs_string(&value.to_string(), output)?,
        SValue::String(value) => write_jcs_string(value, output)?,
        SValue::Unit | SValue::Option(None) => output.push_str("null"),
        SValue::Option(Some(value)) | SValue::Newtype(value) => write_jcs_value(value, output)?,
        SValue::Seq(values) => {
            output.push('[');
            for (index, value) in values.iter().enumerate() {
                if index != 0 {
                    output.push(',');
                }
                write_jcs_value(value, output)?;
            }
            output.push(']');
        }
        SValue::Map(values) => {
            let mut entries = values
                .iter()
                .map(|(key, value)| match key {
                    SValue::String(key) => Ok((key.as_str(), value)),
                    _ => Err(ProtocolError("jcs_non_string_object_key")),
                })
                .collect::<Result<Vec<_>>>()?;
            entries.sort_by(|(left, _), (right, _)| compare_utf16(left, right));
            output.push('{');
            for (index, (key, value)) in entries.iter().enumerate() {
                if index != 0 {
                    output.push(',');
                }
                write_jcs_string(key, output)?;
                output.push(':');
                write_jcs_value(value, output)?;
            }
            output.push('}');
        }
        SValue::Bytes(_) => return Err(ProtocolError("jcs_unsupported_value")),
    }
    Ok(())
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

pub fn sha256_jcs<T: Serialize>(value: &T) -> Result<String> {
    Ok(sha256_hex(&jcs_bytes(value)?))
}

pub fn validate_canonical_uuid(value: &str) -> Result<()> {
    let parsed =
        uuid::Uuid::parse_str(value).map_err(|_| ProtocolError("invalid_canonical_uuid"))?;
    if parsed.to_string() != value
        || parsed.get_variant() != Variant::RFC4122
        || parsed.get_version().is_none()
    {
        return Err(ProtocolError("invalid_canonical_uuid"));
    }
    Ok(())
}

pub fn validate_canonical_uuid_v4(value: &str) -> Result<()> {
    validate_canonical_uuid(value)?;
    let parsed =
        uuid::Uuid::parse_str(value).map_err(|_| ProtocolError("invalid_canonical_uuid_v4"))?;
    if parsed.get_version() != Some(Version::Random) {
        return Err(ProtocolError("invalid_canonical_uuid_v4"));
    }
    Ok(())
}

fn canonical_unsigned_decimal(value: &str) -> bool {
    value == "0"
        || (!value.is_empty()
            && !value.starts_with('0')
            && value.as_bytes().iter().all(u8::is_ascii_digit))
}

pub fn parse_writer_seq(value: &str) -> Result<u64> {
    if !canonical_unsigned_decimal(value) {
        return Err(ProtocolError("invalid_writer_seq"));
    }
    let parsed = value
        .parse::<u64>()
        .map_err(|_| ProtocolError("invalid_writer_seq"))?;
    if parsed == 0 {
        return Err(ProtocolError("invalid_writer_seq"));
    }
    Ok(parsed)
}

pub fn parse_int64_decimal(value: &str, minimum: i64, maximum: i64) -> Result<i64> {
    if value == "-0"
        || value.starts_with('+')
        || value.is_empty()
        || (value.starts_with('0') && value.len() > 1)
        || (value.starts_with("-0") && value.len() > 2)
        || !value
            .strip_prefix('-')
            .unwrap_or(value)
            .as_bytes()
            .iter()
            .all(u8::is_ascii_digit)
    {
        return Err(ProtocolError("invalid_int64_decimal_string"));
    }
    let parsed = value
        .parse::<i64>()
        .map_err(|_| ProtocolError("int64_out_of_range"))?;
    if parsed < minimum || parsed > maximum {
        return Err(ProtocolError("int64_out_of_range"));
    }
    Ok(parsed)
}

pub fn validate_safe_integer(value: &Value, minimum: i64, maximum: i64) -> Result<i64> {
    let parsed = value
        .as_f64()
        .ok_or(ProtocolError("invalid_safe_integer"))?;
    if !parsed.is_finite()
        || parsed.fract() != 0.0
        || parsed.abs() > MAX_SAFE_INTEGER as f64
        || parsed < minimum as f64
        || parsed > maximum as f64
    {
        return Err(ProtocolError("invalid_safe_integer"));
    }
    Ok(parsed as i64)
}

pub fn validate_float64(value: &Value, minimum: f64, maximum: f64) -> Result<f64> {
    let parsed = value.as_f64().ok_or(ProtocolError("invalid_float64"))?;
    if !parsed.is_finite() || parsed < minimum || parsed > maximum {
        return Err(ProtocolError("invalid_float64"));
    }
    Ok(parsed)
}

pub fn validate_date(value: &str) -> Result<()> {
    let bytes = value.as_bytes();
    if bytes.len() != 10
        || bytes[4] != b'-'
        || bytes[7] != b'-'
        || ![0..4, 5..7, 8..10]
            .into_iter()
            .all(|range| bytes[range].iter().all(u8::is_ascii_digit))
    {
        return Err(ProtocolError("invalid_date"));
    }
    let year = value[0..4].parse::<i32>().expect("ASCII digits");
    let month = value[5..7].parse::<u32>().expect("ASCII digits");
    let day = value[8..10].parse::<u32>().expect("ASCII digits");
    if year == 0 || NaiveDate::from_ymd_opt(year, month, day).is_none() {
        return Err(ProtocolError("invalid_date"));
    }
    Ok(())
}

pub fn validate_timestamp(value: &str) -> Result<()> {
    let bytes = value.as_bytes();
    if bytes.len() != 24
        || bytes[10] != b'T'
        || bytes[13] != b':'
        || bytes[16] != b':'
        || bytes[19] != b'.'
        || bytes[23] != b'Z'
        || ![0..4, 5..7, 8..10, 11..13, 14..16, 17..19, 20..23]
            .into_iter()
            .all(|range| bytes[range].iter().all(u8::is_ascii_digit))
    {
        return Err(ProtocolError("invalid_timestamp"));
    }
    validate_date(&value[0..10]).map_err(|_| ProtocolError("invalid_timestamp"))?;
    let hour = value[11..13].parse::<u32>().expect("ASCII digits");
    let minute = value[14..16].parse::<u32>().expect("ASCII digits");
    let second = value[17..19].parse::<u32>().expect("ASCII digits");
    if hour > 23 || minute > 59 || second > 59 {
        return Err(ProtocolError("invalid_timestamp"));
    }
    Ok(())
}

pub fn validate_commit_dot(dot: &CommitDot) -> Result<()> {
    validate_canonical_uuid_v4(&dot.writer_id)?;
    parse_writer_seq(&dot.writer_seq)?;
    validate_canonical_uuid_v4(&dot.commit_id)
}

pub fn validate_commit_ref(reference: &CommitRef) -> Result<()> {
    validate_commit_dot(&reference.dot())?;
    if reference.content_hash.len() != 64
        || !reference
            .content_hash
            .as_bytes()
            .iter()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(byte))
    {
        return Err(ProtocolError("invalid_content_hash"));
    }
    Ok(())
}

pub fn compare_commit_dot_v1(a: &CommitDot, b: &CommitDot) -> Ordering {
    a.writer_id
        .as_bytes()
        .cmp(b.writer_id.as_bytes())
        .then_with(|| {
            parse_writer_seq(&a.writer_seq)
                .expect("validated CommitDot")
                .cmp(&parse_writer_seq(&b.writer_seq).expect("validated CommitDot"))
        })
        .then_with(|| a.commit_id.as_bytes().cmp(b.commit_id.as_bytes()))
}

pub fn compare_commit_ref_v1(a: &CommitRef, b: &CommitRef) -> Ordering {
    compare_commit_dot_v1(&a.dot(), &b.dot())
        .then_with(|| a.content_hash.as_bytes().cmp(b.content_hash.as_bytes()))
}

pub fn canonical_entity_key_bytes(key: &EntityKey) -> Result<Vec<u8>> {
    jcs_bytes(key)
}

pub fn compare_entity_key_v1(a: &EntityKey, b: &EntityKey) -> Ordering {
    canonical_entity_key_bytes(a)
        .expect("validated EntityKey")
        .cmp(&canonical_entity_key_bytes(b).expect("validated EntityKey"))
}

fn validate_identity_part(value: &Value) -> Result<()> {
    let text = value.as_str().ok_or(ProtocolError("invalid_entity_key"))?;
    let boundary_whitespace =
        |character: char| matches!(character as u32, 0x09 | 0x0a | 0x0b | 0x0c | 0x0d | 0x20);
    if text.is_empty()
        || text.chars().count() > 512
        || text.contains('\0')
        || text.chars().next().is_some_and(boundary_whitespace)
        || text.chars().next_back().is_some_and(boundary_whitespace)
    {
        return Err(ProtocolError("invalid_entity_key"));
    }
    Ok(())
}

pub fn validate_entity_key(value: &Value) -> Result<()> {
    let parts = value
        .as_array()
        .ok_or(ProtocolError("invalid_entity_key"))?;
    match parts.first().and_then(Value::as_str) {
        Some("record" | "collection") if parts.len() == 2 => validate_identity_part(&parts[1]),
        Some("episode-completion") if parts.len() == 3 => {
            validate_identity_part(&parts[1])?;
            validate_safe_integer(&parts[2], 1, i32::MAX as i64).map(|_| ())
        }
        Some("collection-member") if parts.len() == 3 => {
            validate_identity_part(&parts[1])?;
            validate_identity_part(&parts[2])
        }
        _ => Err(ProtocolError("invalid_entity_key")),
    }
}

pub fn canonical_collection_name_v1(value: &str) -> Result<String> {
    let mut output = String::new();
    let mut pending_space = false;
    for character in value.chars() {
        let code = character as u32;
        let is_s2_whitespace = matches!(code, 0x09 | 0x0a | 0x0b | 0x0c | 0x0d | 0x20);
        if is_s2_whitespace {
            if !output.is_empty() {
                pending_space = true;
            }
            continue;
        }
        if code <= 0x1f || code == 0x7f {
            return Err(ProtocolError("invalid_collection_name"));
        }
        if pending_space {
            output.push(' ');
            pending_space = false;
        }
        output.push(character);
    }
    if output.is_empty() || output.chars().count() > 80 {
        return Err(ProtocolError("invalid_collection_name"));
    }
    Ok(output)
}

pub fn normalize_collection_name_v1(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_ascii_uppercase() {
                character.to_ascii_lowercase()
            } else {
                character
            }
        })
        .collect()
}
