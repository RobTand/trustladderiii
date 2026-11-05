//! Temporary harness-compatible diagnostics; consolidate with WR1 plan_rules.
use crate::contracts::ProofDeclaration;
use serde_json::Value;
use std::{collections::BTreeSet, sync::LazyLock};

pub(super) fn parse(value: &Value) -> Result<ProofDeclaration, &'static str> {
    let object = value
        .as_object()
        .ok_or("A proof needs subjects, observer_support and red_message.")?;
    if object.len() != 3
        || !object.contains_key("red_message")
        || !value["subjects"].is_array()
        || !value["observer_support"].is_array()
    {
        return Err("A proof needs subjects, observer_support and red_message.");
    }
    let subjects = value["subjects"].as_array().unwrap();
    let support = value["observer_support"].as_array().unwrap();
    if subjects.len() > 16 || support.len() > 16 {
        return Err(
            "A proof names at most 16 subject units and at most 16 observer support units.",
        );
    }
    for unit in subjects.iter().chain(support) {
        let valid = unit.as_str().is_some_and(|unit| {
            let mut parts = unit.split("::");
            let path = parts.next().unwrap_or_default();
            unit.chars().count() <= 1000 && path.ends_with(".py") && !path.starts_with(['/', '-'])
                && !path.split('/').any(|part| matches!(part, ".." | ".git" | ""))
                && !unit.contains(['\\', '\0', '\n', '\r'])
                // The role schema already limits qualified definitions to ASCII identifiers.
                && parts.all(|name| { let mut chars = name.chars();
                    chars.next().is_some_and(|c| c == '_' || c.is_ascii_alphabetic())
                        && chars.all(|c| c == '_' || c.is_ascii_alphanumeric()) })
        });
        if !valid {
            return Err(
                "Proof units need a relative Python path and optional qualified definition.",
            );
        }
    }
    for units in [subjects, support] {
        let distinct: BTreeSet<_> = units.iter().map(|unit| unit.as_str().unwrap()).collect();
        if distinct.len() != units.len() {
            return Err("A proof cannot repeat source units.");
        }
    }
    if !value["red_message"]
        .as_str()
        .is_some_and(|message| (1..=160).contains(&message.chars().count()))
    {
        return Err(
            "A proof declares the exact assertion message supplying RED, 1 to 160 characters.",
        );
    }
    serde_json::from_value(value.clone())
        .map_err(|_| "Proof units need a relative Python path and optional qualified definition.")
}

pub(super) fn python_repr(text: &str) -> String {
    static NONPRINTING: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r"[\p{C}\p{Z}]").unwrap());
    let quote = if text.contains('\'') && !text.contains('"') {
        '"'
    } else {
        '\''
    };
    let mut out = String::from(quote);
    for c in text.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c == quote => {
                out.push('\\');
                out.push(c);
            }
            c if c != ' ' && NONPRINTING.is_match(c.encode_utf8(&mut [0; 4])) => {
                use std::fmt::Write;
                let n = c as u32;
                if n <= 0xff {
                    write!(out, "\\x{n:02x}").unwrap();
                } else if n <= 0xffff {
                    write!(out, "\\u{n:04x}").unwrap();
                } else {
                    write!(out, "\\U{n:08x}").unwrap();
                }
            }
            c => out.push(c),
        }
    }
    out.push(quote);
    out
}
