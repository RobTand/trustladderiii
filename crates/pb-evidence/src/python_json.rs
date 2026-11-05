//! Python's JSON spelling is identity data, not floating-point decision arithmetic.
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::fmt::Write;

pub fn sha(raw: &[u8]) -> String {
    format!("{:x}", Sha256::digest(raw))
}

pub fn truth(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(value) => *value,
        Value::Number(value) => decimal(&value.to_string()).0 != "0",
        Value::String(value) => !value.is_empty(),
        Value::Array(value) => !value.is_empty(),
        Value::Object(value) => !value.is_empty(),
    }
}

// A normalized decimal coefficient and power permit Python's numeric equality,
// including bool == int and 0.0 == 0, without floats in the decision core.
fn decimal(raw: &str) -> (String, i32) {
    let (mantissa, power) = raw
        .split_once(['e', 'E'])
        .map_or((raw, 0), |(a, b)| (a, b.parse::<i32>().unwrap_or(0)));
    let fractional = mantissa.split_once('.').map_or(0, |(_, b)| b.len() as i32);
    let negative = mantissa.starts_with('-');
    let mut digits = mantissa.trim_start_matches('-').replace('.', "");
    let mut exponent = power - fractional;
    while digits.len() > 1 && digits.ends_with('0') {
        digits.pop();
        exponent += 1;
    }
    digits = digits.trim_start_matches('0').to_owned();
    if digits.is_empty() {
        return ("0".into(), 0);
    }
    if negative {
        digits.insert(0, '-');
    }
    (digits, exponent)
}
fn numeric(value: &Value) -> Option<(String, i32)> {
    match value {
        Value::Bool(false) => Some(("0".into(), 0)),
        Value::Bool(true) => Some(("1".into(), 0)),
        Value::Number(number) => Some(decimal(&number.to_string())),
        _ => None,
    }
}
pub fn equal(left: &Value, right: &Value) -> bool {
    match (left, right) {
        (Value::Array(a), Value::Array(b)) => {
            a.len() == b.len() && a.iter().zip(b).all(|(a, b)| equal(a, b))
        }
        (Value::Object(a), Value::Object(b)) => {
            a.len() == b.len() && a.iter().all(|(k, a)| b.get(k).is_some_and(|b| equal(a, b)))
        }
        _ => match (numeric(left), numeric(right)) {
            (Some(a), Some(b)) => a == b,
            _ => left == right,
        },
    }
}
/// Integer decisions inspect decimal tokens, including integers beyond u64.
pub fn nonnegative_integer(value: &Value) -> bool {
    match value {
        Value::Number(number) => number.to_string().bytes().all(|b| b.is_ascii_digit()),
        _ => false,
    }
}

/// CPython rounds floating tokens once at decoding; integer lexemes stay exact.
pub fn normalize_floats(value: &mut Value) {
    match value {
        Value::Number(number) if number.to_string() == "-0" => *number = 0.into(),
        Value::Number(number) if number.to_string().contains(['.', 'e', 'E']) => {
            if let Some(rounded) = number.as_f64().and_then(serde_json::Number::from_f64) {
                *number = rounded;
            }
        }
        Value::Array(items) => items.iter_mut().for_each(normalize_floats),
        Value::Object(items) => items.values_mut().for_each(normalize_floats),
        _ => {}
    }
}

pub fn integer(value: &Value) -> Option<u64> {
    value.as_u64().filter(|_| value.is_u64() || value.is_i64())
}

fn float_text(raw: &str) -> String {
    let negative = raw.starts_with('-');
    let (digits, power) = decimal(raw);
    if digits == "0" {
        return if negative {
            "-0.0".into()
        } else {
            "0.0".into()
        };
    }
    let sign = if negative { "-" } else { "" };
    let digits = digits.trim_start_matches('-');
    let exponent = power + digits.len() as i32 - 1;
    if !(-4..16).contains(&exponent) {
        let rest = if digits.len() > 1 {
            format!(".{}", &digits[1..])
        } else {
            String::new()
        };
        format!(
            "{sign}{}{rest}e{}{:02}",
            &digits[..1],
            if exponent < 0 { '-' } else { '+' },
            exponent.unsigned_abs()
        )
    } else {
        let point = exponent + 1;
        if point <= 0 {
            format!("{sign}0.{}{digits}", "0".repeat((-point) as usize))
        } else if point as usize >= digits.len() {
            format!(
                "{sign}{digits}{}.0",
                "0".repeat(point as usize - digits.len())
            )
        } else {
            format!(
                "{sign}{}.{}",
                &digits[..point as usize],
                &digits[point as usize..]
            )
        }
    }
}
fn string(value: &str, ascii: bool, out: &mut String) {
    out.push('"');
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            ch if ch < ' ' || ascii && ch > '~' => {
                let mut words = [0; 2];
                for word in ch.encode_utf16(&mut words) {
                    let _ = write!(out, "\\u{word:04x}");
                }
            }
            ch => out.push(ch),
        }
    }
    out.push('"');
}
pub fn dumps(value: &Value, ascii: bool, compact: bool) -> String {
    fn render(value: &Value, ascii: bool, compact: bool, out: &mut String) {
        let comma = if compact { "," } else { ", " };
        let colon = if compact { ":" } else { ": " };
        match value {
            Value::Null => out.push_str("null"),
            Value::Bool(v) => out.push_str(if *v { "true" } else { "false" }),
            Value::Number(n) => out.push_str(&if n.is_f64() {
                float_text(&n.to_string())
            } else {
                n.to_string()
            }),
            Value::String(s) => string(s, ascii, out),
            Value::Array(items) => {
                out.push('[');
                for (i, v) in items.iter().enumerate() {
                    if i > 0 {
                        out.push_str(comma);
                    }
                    render(v, ascii, compact, out);
                }
                out.push(']');
            }
            Value::Object(items) => {
                out.push('{');
                for (i, (k, v)) in items.iter().enumerate() {
                    if i > 0 {
                        out.push_str(comma);
                    }
                    string(k, ascii, out);
                    out.push_str(colon);
                    render(v, ascii, compact, out);
                }
                out.push('}');
            }
        }
    }
    let mut out = String::new();
    render(value, ascii, compact, &mut out);
    out
}
