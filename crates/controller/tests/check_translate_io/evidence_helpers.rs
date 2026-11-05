use super::*;
const MAX_ITEMS: usize = 32;
const RECORD_BYTES: usize = 8000;
fn fields(value: &Value, names: &[&str]) -> Value {
    let mut out = serde_json::Map::new();
    for name in names {
        if let Some(value) = value.get(name) {
            out.insert((*name).into(), recorded_value(value, Some(name), 8));
        }
    }
    Value::Object(out)
}
fn recorded_value(value: &Value, field: Option<&str>, depth: usize) -> Value {
    if depth == 0 {
        return json!("[nested value omitted]");
    }
    if field.is_some_and(crate::memory::sensitive_key) && !proof_hash(value, field) {
        return json!("[redacted]");
    }
    match value {
        Value::String(text) => {
            // Test IDs may exceed a display line; preserve their context until redaction.
            let limit = if field == Some("test") { 1024 } else { 512 };
            if text.len() > limit {
                json!("[long text omitted]")
            } else {
                json!(text.replace('\0', ""))
            }
        }
        Value::Array(values) => Value::Array(
            values
                .iter()
                .take(MAX_ITEMS)
                .map(|value| recorded_value(value, None, depth - 1))
                .collect(),
        ),
        Value::Object(values) => Value::Object(
            values
                .iter()
                .take(MAX_ITEMS)
                .map(|(key, value)| {
                    let name = if key.len() > 512 {
                        "[long key omitted]".into()
                    } else {
                        key.replace('\0', "")
                    };
                    (name, recorded_value(value, Some(key), depth - 1))
                })
                .collect(),
        ),
        _ => value.clone(),
    }
}
fn complete_check(record: &Value, red_green: bool) -> bool {
    record["proof_missing"] != true
        && (!red_green || record["red_green_missing"] != true)
        && record["results"].as_array().is_some_and(|results| {
            !results.is_empty()
                && results.iter().all(|result| {
                    result["verdict"].as_str().is_some()
                        && (!red_green
                            || result["kind"] != "demonstrating"
                            || result["tests"].as_array().is_some_and(|tests| {
                                !tests.is_empty()
                                    && tests.iter().all(|test| {
                                        test["base"].as_str().is_some()
                                            && test["head"].as_str().is_some()
                                    })
                            }))
                        && (result.get("forcing").is_none()
                            || result["tests"].as_array().is_some_and(|tests| {
                                !tests.is_empty()
                                    && tests
                                        .iter()
                                        .all(|test| test["head_forced"].as_str().is_some())
                            }))
                })
        })
}
pub(crate) fn check_record(record: &Value) -> Value {
    let mut out = fields(
        record,
        &[
            "head_sha",
            "base_sha",
            "action_key",
            "report",
            "pass",
            "url",
            "receipt_sha256",
            "truncated",
        ],
    );
    out["proof_missing"] = json!(!complete_check(record, false));
    out["red_green_missing"] = json!(!complete_check(record, true));
    if let Some(results) = record["results"].as_array() {
        out["failed_results"] = record
            .get("failed_results")
            .cloned()
            .unwrap_or_else(|| json!(results.iter().filter(|r| r["verdict"] != "pass").count()));
        out["forced_passed"] = record.get("forced_passed").cloned().unwrap_or_else(|| {
            json!(
                results
                    .iter()
                    .filter(|r| r.get("forcing").is_some())
                    .flat_map(|r| r["tests"].as_array().into_iter().flatten())
                    .filter(|t| t["head_forced"] == "passed")
                    .count()
            )
        });
        out["forced_other"] = record.get("forced_other").cloned().unwrap_or_else(|| {
            json!(
                results
                    .iter()
                    .filter(|r| r.get("forcing").is_some())
                    .flat_map(|r| r["tests"].as_array().into_iter().flatten())
                    .filter(|t| t["head_forced"] != "passed")
                    .count()
            )
        });
        out["result_count"] = record
            .get("result_count")
            .cloned()
            .unwrap_or(json!(results.len()));
        out["results"] = Value::Array(
            results
                .iter()
                .take(MAX_ITEMS)
                .map(|result| {
                    let mut item = fields(result, &["name", "kind", "verdict", "url", "proof"]);
                    if let Some(forcing) = result.get("forcing") {
                        // Environment values and patch bodies are opaque inputs, not proof.
                        let mut summary = fields(forcing, &["symptom"]);
                        summary["environment_count"] = forcing
                            .get("environment_count")
                            .cloned()
                            .unwrap_or_else(|| {
                                json!(forcing["environment"].as_object().map_or(0, |v| v.len()))
                            });
                        summary["patch_count"] =
                            forcing.get("patch_count").cloned().unwrap_or_else(|| {
                                json!(forcing["patches"].as_array().map_or(0, |v| v.len()))
                            });
                        item["forcing"] = summary;
                    }
                    if let Some(tests) = result["tests"].as_array() {
                        item["test_count"] = result
                            .get("test_count")
                            .cloned()
                            .unwrap_or(json!(tests.len()));
                        item["tests"] = Value::Array(
                            tests
                                .iter()
                                .take(MAX_ITEMS)
                                .map(|test| {
                                    fields(
                                        test,
                                        &["test", "base", "head", "head_forced", "passed", "url"],
                                    )
                                })
                                .collect(),
                        );
                    }
                    item
                })
                .collect(),
        );
    }
    // Keep complete entries and explicit counts. Never slice serialized JSON.
    while encode_proof(&out).expect("proof JSON").len() > RECORD_BYTES {
        out["truncated"] = json!(true);
        let results = out["results"].as_array_mut().expect("check results");
        let last = results.last_mut().expect("one bounded result fits");
        if let Some(tests) = last["tests"].as_array_mut().filter(|tests| tests.len() > 1) {
            tests.pop();
        } else if results.len() > 1 {
            results.pop();
        } else {
            break;
        }
    }
    out
}
fn encode_proof(value: &Value) -> Result<String> {
    Ok(serde_json::to_string(value)?
        .replace('`', "\\u0060")
        .replace('<', "\\u003c")
        .replace('>', "\\u003e")
        .replace('@', "\\u0040"))
}
fn proof_hash(value: &Value, field: Option<&str>) -> bool {
    if let (Some(field), Some(text)) = (field, value.as_str())
        && matches!(
            field,
            "head_sha" | "base_sha" | "action_key" | "receipt_sha256" | "log_sha256" | "tree"
        )
        && matches!(text.len(), 40 | 64)
        && text.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return true;
    }
    false
}
