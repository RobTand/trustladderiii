use crate::error::{Refusal, Result};
use crate::python_json::{dumps, equal, integer, sha};
use serde_json::{Value, json};
use std::collections::BTreeSet;

pub fn selection_matches(value: &Value, selected: &[String]) -> bool {
    let selected = json!(selected);
    if value.is_array() {
        return equal(value, &selected);
    }
    let Some(items) = selected.as_array() else {
        return false;
    };
    let prefix: Vec<_> = items
        .iter()
        .take(3)
        .map(|test| {
            test.as_str()
                .unwrap_or("")
                .chars()
                .take(120)
                .collect::<String>()
        })
        .collect();
    let expected = json!({"schema":"tl3.selectors.v1","count":items.len(),"sha256":sha(dumps(&selected,true,true).as_bytes()),"prefix":prefix});
    integer(&value["count"]).is_some() && equal(value, &expected)
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct ExpectedRun {
    pub base: bool,
    pub base_sha: Value,
    pub tree: Value,
    pub selected: Vec<String>,
    pub proof: Value,
    pub forcing: Value,
    pub declared_symptom: Value,
}
pub fn check(mut result: Value, expected: &ExpectedRun) -> Result<Value> {
    let refusal = crate::decision::consistency(
        result["status"] == "completed",
        expected.base,
        equal(&result["proof"], &expected.proof),
        !expected.forcing.is_null(),
        equal(&result["forcing"], &expected.forcing),
        selection_matches(&result["selectors"], &expected.selected),
        !expected.declared_symptom.is_null(),
        equal(&result["declared_symptom"], &expected.declared_symptom),
    );
    if refusal != 0 {
        return Err(Refusal::transport(
            "summary",
            match refusal {
                1 => "proof_mismatch",
                2 => "forcing_or_selection_mismatch",
                _ => "symptom_mismatch",
            },
            None,
        ));
    }
    if expected.base {
        result["base_sha"] = expected.base_sha.clone();
        result["base_semantics"] = json!(if expected.proof["mode"] == "head-observer" {
            "base_subject_head_observer"
        } else {
            "untouched_base"
        });
        result["demonstrating_proof"] = json!("pending_checking");
    }
    if !expected.tree.is_null() {
        result["tree"] = expected.tree.clone();
    }
    Ok(result)
}

pub fn selectors(value: &Value) -> Result<Value> {
    let items = value
        .as_array()
        .filter(|items| items.len() <= 256)
        .ok_or_else(|| Refusal::value("Use a list of at most 256 test selectors"))?;
    let mut seen = BTreeSet::new();
    let mut result = Vec::new();
    for test in items {
        let test = test
            .as_str()
            .filter(|s| {
                !s.is_empty() && s.chars().count() <= 2048 && !s.contains(['\0', '\\', '\n', '\r'])
            })
            .ok_or_else(|| Refusal::value("Use relative pytest paths or node IDs"))?;
        let path = test.split("::").next().unwrap_or("");
        // PurePosixPath ignores redundant separators and dot components here.
        if path.starts_with('/')
            || test.starts_with('-')
            || path.is_empty()
            || path.split('/').any(|part| matches!(part, ".." | ".git"))
        {
            return Err(Refusal::value("Use relative pytest paths or node IDs"));
        }
        if seen.insert(test) {
            result.push(test);
        }
    }
    Ok(json!(result))
}
pub fn priority(value: &Value) -> Result<Value> {
    if value.is_null() {
        return Ok(Value::Null);
    }
    if value.is_i64() && value.as_i64().is_some_and(|n| (-10..=0).contains(&n)) {
        Ok(value.clone())
    } else {
        Err(Refusal::value(
            "Use priority -10 to 0, or leave it out for PrismaBuild's default",
        ))
    }
}
pub fn settings(value: &Value) -> Result<Value> {
    let image = value["image"].as_str().unwrap_or("");
    let pattern =
        regex::Regex::new(r"^(?:[\w./:-]+@)?sha256:[0-9a-f]{64}$").expect("constant image pattern");
    if !pattern.is_match(image) {
        return Err(Refusal::value("Use an immutable container image digest"));
    }
    if !value["tools_path"]
        .as_str()
        .is_some_and(|p| p.starts_with('/'))
    {
        return Err(Refusal::value("PrismaBuild tools_path must be absolute"));
    }
    let mut result = value.clone();
    for (key, default) in [("cpus", 1), ("mem_gb", 2), ("quiet_seconds", 120)] {
        let v = value.get(key).cloned().unwrap_or(json!(default));
        if !integer(&v).is_some_and(|n| n >= 1) {
            return Err(Refusal::value(
                "Test resource and quiet allowances must be positive integers",
            ));
        }
        result[key] = v;
    }
    Ok(result)
}
