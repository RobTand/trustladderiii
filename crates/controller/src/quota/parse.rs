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
    // email._parseaddr ignores weekdays, accepts RFC 850 and asctime order,
    // and expands every numeric year below 100 before interpreting the date.
    // Normalize first: Chrono's own two-digit pivot and weekday check differ.
    let mut fields: Vec<_> = text.split_whitespace().collect();
    let first = *fields.first()?;
    if first.ends_with(',')
        || ["mon", "tue", "wed", "thu", "fri", "sat", "sun"]
            .contains(&first.to_ascii_lowercase().as_str())
    {
        fields.remove(0);
    } else if let Some((_, rest)) = first.rsplit_once(',') {
        fields[0] = rest;
    }
    if fields.len() == 3 {
        let date: Vec<_> = fields[0].split('-').collect();
        if date.len() == 3 {
            fields.splice(0..1, date);
        }
    }
    if fields.len() == 4 {
        fields.push("GMT");
    }
    if fields.len() < 5 {
        return None;
    }
    // asctime puts the month first and the year after the time.
    if fields[0].parse::<u32>().is_err() {
        fields.swap(0, 1);
    }
    if fields[2].contains(':') {
        fields.swap(2, 3);
    }
    let year: i32 = fields[2].trim_end_matches(',').parse().ok()?;
    let year = if year < 100 {
        year + if year < 69 { 2000 } else { 1900 }
    } else {
        year
    };
    let normalized = format!(
        "{} {} {year:04} {} {}",
        fields[0], fields[1], fields[3], fields[4]
    );
    if let Ok(date) = DateTime::parse_from_rfc2822(&normalized) {
        return Some(date.timestamp() as f64);
    }
    // A missing zone is UTC in the worker runtime, as for legacy asctime dates.
    for format in ["%d %b %Y %H:%M:%S GMT", "%d %b %Y %H:%M GMT"] {
        if let Ok(date) = NaiveDateTime::parse_from_str(&normalized, format) {
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
