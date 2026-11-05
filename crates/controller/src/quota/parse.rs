//! Python binary64 and HTTP-date compatibility at the untrusted header boundary.
use chrono::{DateTime, NaiveDateTime, TimeZone, Utc};
use serde_json::Value;
use std::sync::OnceLock;

pub(super) fn number(value: &Value) -> Option<f64> {
    match value {
        Value::Number(n) => n.as_f64(),
        Value::Bool(b) => Some(u8::from(*b) as f64),
        Value::String(text) => python_float(text),
        _ => None,
    }
}

pub(super) fn python_float(text: &str) -> Option<f64> {
    let text = text.trim();
    let lower = text.to_ascii_lowercase();
    match lower.as_str() {
        "inf" | "+inf" | "infinity" | "+infinity" => return Some(f64::INFINITY),
        "-inf" | "-infinity" => return Some(f64::NEG_INFINITY),
        "nan" | "+nan" | "-nan" => return Some(f64::NAN),
        _ => (),
    }
    static FLOAT: OnceLock<regex::Regex> = OnceLock::new();
    let syntax = FLOAT.get_or_init(|| regex::Regex::new(
        r"^[+-]?(?:[0-9](?:_?[0-9])*(?:\.(?:[0-9](?:_?[0-9])*)?)?|\.[0-9](?:_?[0-9])*)(?:[eE][+-]?[0-9](?:_?[0-9])*)?$"
    ).expect("fixed float grammar"));
    syntax
        .is_match(text)
        .then(|| text.replace('_', "").parse().ok())
        .flatten()
}

fn date(text: &str) -> Option<f64> {
    if let Ok(date) = DateTime::parse_from_rfc2822(text) {
        return Some(date.timestamp() as f64);
    }
    // Python's email parser accepts obsolete HTTP dates and ignores the weekday.
    let text = text.split_once(',').map_or(text, |(_, rest)| rest).trim();
    for format in [
        "%d-%b-%y %H:%M:%S GMT",
        "%d %b %Y %H:%M:%S GMT",
        "%a %b %e %H:%M:%S %Y",
        "%d %b %Y %H:%M:%S",
    ] {
        if let Ok(date) = NaiveDateTime::parse_from_str(text, format) {
            return Some(Utc.from_utc_datetime(&date).timestamp() as f64);
        }
    }
    None
}

pub(super) fn announced(
    headers: &std::collections::BTreeMap<String, Value>,
    now: f64,
) -> Option<f64> {
    if let Some(value) = headers.get("retry-after") {
        return number(value)
            .map(|delay| now + delay)
            .or_else(|| value.as_str().and_then(date));
    }
    headers.get("x-ratelimit-reset").and_then(number)
}

/// Decode the normalized binary rational, without floating-point ceil or casts
/// that silently saturate at the u64 wire limit.
pub(super) fn milliseconds(seconds: f64) -> Option<(u64, bool)> {
    let value = seconds * 1000.0;
    if !value.is_finite() || !(0.0..18_446_744_073_709_551_616.0).contains(&value) {
        return None;
    }
    let bits = value.to_bits();
    let exponent = ((bits >> 52) & 0x7ff) as i32 - 1023;
    let significand = (bits & ((1u64 << 52) - 1)) | (1u64 << 52);
    if value == 0.0 {
        return Some((0, false));
    }
    if exponent < 0 {
        return Some((0, true));
    }
    if exponent >= 52 {
        return Some((significand << (exponent - 52), false));
    }
    let shift = 52 - exponent;
    Some((
        significand >> shift,
        significand & ((1u64 << shift) - 1) != 0,
    ))
}

pub(super) fn ceil(seconds: f64) -> Option<u64> {
    let (whole, fraction) = milliseconds(seconds)?;
    decision_ceil(whole, fraction)
}

fn decision_ceil(whole: u64, fraction: bool) -> Option<u64> {
    super::decision::announced(429, true, true, true, whole, fraction)
}
