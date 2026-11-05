use crate::error::{Refusal, Result, get};
use crate::python_json::{dumps, equal, integer, truth};
use serde_json::{Value, json};
use std::collections::BTreeSet;

pub fn identity(base_sha: &Value, selectors: &Value, forcing: &Value, symptom: &Value) -> Value {
    let mut forcing = forcing.clone();
    if forcing.get("environment").is_none() {
        forcing["environment"] = json!({});
    }
    if forcing.get("patches").is_none() {
        forcing["patches"] = json!([]);
    }
    let mut result = json!({"base_sha":base_sha,"selectors":selectors,"forcing":forcing});
    if !symptom.is_null() {
        result["declared_symptom"] = symptom.clone();
    }
    result
}
pub fn bound(candidate: &Value) -> Result<String> {
    Ok(dumps(
        &serde_json::to_value(crate::identity_fields::bind(
            get(candidate, "base_sha")?,
            get(candidate, "selectors")?,
            get(candidate, "forcing")?,
            &candidate["declared_symptom"],
        ))
        .expect("identity fields serialize"),
        true,
        true,
    ))
}
pub fn execution(candidate: &Value) -> Result<String> {
    let forcing = get(candidate, "forcing")?;
    let patches = get(forcing, "patches")?
        .as_array()
        .ok_or_else(|| Refusal::value("patches must be a list"))?;
    let mut independent = true;
    for (index, left) in patches.iter().enumerate() {
        let left = get(left, "target")?
            .as_str()
            .ok_or_else(|| Refusal::value("patch target must be a string"))?;
        for right in &patches[index + 1..] {
            let right = get(right, "target")?
                .as_str()
                .ok_or_else(|| Refusal::value("patch target must be a string"))?;
            independent &= crate::decision::independent(
                left.rsplit('.').next() == right.rsplit('.').next(),
                left.starts_with(&format!("{right}.")),
                right.starts_with(&format!("{left}.")),
            );
        }
    }
    let mut patches = patches.clone();
    if independent {
        patches.sort_by_cached_key(|patch| dumps(patch, true, true));
    }
    let selectors = get(candidate, "selectors")?
        .as_array()
        .ok_or_else(|| Refusal::value("selectors must be a list"))?;
    let selectors: BTreeSet<_> = selectors
        .iter()
        .map(|s| {
            s.as_str()
                .ok_or_else(|| Refusal::value("selector must be a string"))
        })
        .collect::<Result<_>>()?;
    Ok(dumps(
        &json!({"base_sha":get(candidate,"base_sha")?,"selectors":selectors,"declared_symptom":candidate["declared_symptom"],
        "forcing":{"environment":get(forcing,"environment")?,"patches":patches}}),
        true,
        true,
    ))
}
fn display(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Null => "None".into(),
        Value::Bool(true) => "True".into(),
        Value::Bool(false) => "False".into(),
        _ => dumps(value, false, false),
    }
}
pub fn unusable(result: &Value, selectors: &[String], outcome: &str) -> Result<Value> {
    if result["status"] != "completed" || !truth(&result["receipts"]) {
        return Ok(json!(format!(
            "missing completed, verified receipt: status={}; {}",
            display(result.get("status").unwrap_or(&json!("unknown"))),
            display(result.get("message").unwrap_or(&json!("no diagnostic")))
        )));
    }
    let tests = get(result, "tests")?
        .as_array()
        .ok_or_else(|| Refusal::value("tests must be a list"))?;
    let totals = get(result, "totals")?
        .as_object()
        .ok_or_else(|| Refusal::value("totals must be an object"))?;
    let total: Option<u128> = totals
        .values()
        .map(|v| integer(v).map(u128::from))
        .try_fold(0u128, |sum, n| n.and_then(|n| sum.checked_add(n)));
    if total != Some(tests.len() as u128) {
        return Ok(json!("incomplete node results"));
    }
    for selector in selectors {
        let nodes: Vec<_> = tests
            .iter()
            .filter(|node| {
                node["test"]
                    .as_str()
                    .is_some_and(|node| selects(selector, node))
            })
            .collect();
        if nodes.is_empty() {
            return Ok(json!(format!("missing node result for {selector}")));
        }
        for node in nodes {
            if node["outcome"] != outcome {
                return Ok(json!(format!(
                    "{} ended {}; expected {outcome}",
                    display(&node["test"]),
                    display(&node["outcome"])
                )));
            }
        }
    }
    Ok(Value::Null)
}
fn selects(requested: &str, collected: &str) -> bool {
    requested == collected
        || !requested.contains("::") && collected.starts_with(&format!("{requested}::"))
        || !requested.contains('[')
            && collected.starts_with(&format!("{requested}["))
            && collected.ends_with(']')
}
pub fn violations(
    failure: &Value,
    receipts: &[Value],
    base_sha: &Value,
    checks: &[Value],
) -> Result<Value> {
    if truth(failure) {
        return Ok(json!([get(failure, "message")?]));
    }
    let mut errors = Vec::new();
    for check in checks {
        if check.get("forcing").is_none() {
            continue;
        }
        let wanted = identity(
            base_sha,
            get(check, "node_ids")?,
            get(check, "forcing")?,
            &check["declared_symptom"],
        );
        let wanted = bound(&wanted)?;
        if !receipts
            .iter()
            .map(bound)
            .collect::<Result<Vec<_>>>()?
            .iter()
            .any(|receipt| receipt == &wanted)
        {
            let nodes = get(check, "node_ids")?
                .as_array()
                .ok_or_else(|| Refusal::value("node_ids must be a list"))?;
            errors.push(format!("Q55 check lacks a matching forcing rehearsal receipt for the exact base, selectors and forcing: {}",nodes.iter().map(display).collect::<Vec<_>>().join(", ")));
        }
    }
    Ok(json!(errors))
}

/// #821: JSON numeric equality is intentionally broader than identity equality.
pub fn exact_identity(left: &Value, right: &Value) -> Result<bool> {
    Ok(equal(&json!(bound(left)?), &json!(bound(right)?)))
}
