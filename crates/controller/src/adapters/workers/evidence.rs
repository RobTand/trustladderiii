//! Recheck worker evidence against the capabilities saved with its request.
use crate::{Error, Result};
use serde_json::{Value, json};

pub(super) fn preflight(role: &str, inputs: &Value, reply: &mut Value) -> Result<()> {
    let Some(capabilities) = inputs["tool_capabilities"].as_array() else {
        // #372: replies to requests started by the previous release still parse.
        return Ok(());
    };
    if !matches!(role, "ingest" | "audit" | "planner") {
        return Ok(());
    }
    let mut contract = Vec::new();
    for source in [
        &inputs["ingest"]["evidence_contract"],
        &inputs["audit_evidence_contract"],
        &reply["evidence_contract"],
    ] {
        for requirement in source.as_array().into_iter().flatten() {
            if !contract.contains(requirement) {
                contract.push(requirement.clone());
            }
        }
    }
    if role == "planner"
        && reply["checks"]
            .as_array()
            .is_some_and(|checks| checks.iter().any(|c| c.get("helper_evidence").is_some()))
    {
        let requirement = json!({
            "requirement":"Observe the real old and new test helper with identical new regression assertions.",
            "tool":"run_tests", "executor":"prismabuild_pytest", "runner_features":["base_head"],
            "artifacts":["cas_receipt", "terminal_verdict"], "methods":["helper_target_observer"], "protected_files":[]});
        if !contract.contains(&requirement) {
            contract.push(requirement);
        }
    }
    let mut errors = Vec::new();
    if role != "planner" && reply.get("evidence_contract").is_none() {
        return Err(Error::Invalid("worker evidence contract is missing"));
    }
    for requirement in &contract {
        let candidates: Vec<Vec<String>> = capabilities
            .iter()
            .map(|capability| {
                let mut missing = Vec::new();
                for key in ["tool", "executor"] {
                    if requirement[key] != capability[key] {
                        missing.push(format!(
                            "{key}={}",
                            requirement[key].as_str().unwrap_or("missing")
                        ));
                    }
                }
                for key in ["runner_features", "artifacts", "methods", "protected_files"] {
                    for value in requirement[key].as_array().into_iter().flatten() {
                        if !capability[key]
                            .as_array()
                            .is_some_and(|values| values.contains(value))
                        {
                            missing.push(format!("{key}={}", value.as_str().unwrap_or("missing")));
                        }
                    }
                }
                missing
            })
            .collect();
        if candidates.iter().any(Vec::is_empty) {
            continue;
        }
        let missing = candidates
            .into_iter()
            .min_by_key(Vec::len)
            .unwrap_or_else(|| {
                vec![
                    format!("tool={}", requirement["tool"].as_str().unwrap_or("missing")),
                    format!(
                        "executor={}",
                        requirement["executor"].as_str().unwrap_or("missing")
                    ),
                ]
            });
        let report = format!(
            "Unsupported evidence requirement: {} ({}).",
            requirement["requirement"].as_str().unwrap_or("missing"),
            missing.join(", ")
        );
        if !errors.contains(&report) {
            errors.push(report);
        }
    }
    reply["evidence_contract"] = json!(contract);
    reply["evidence_errors"] = json!(errors);
    if role == "planner" && !errors.is_empty() {
        if reply["kind"] == "split" {
            let mut files = Vec::new();
            for piece in reply["pieces"].as_array().into_iter().flatten() {
                for path in piece["files"].as_array().into_iter().flatten() {
                    if !files.contains(path) {
                        files.push(path.clone());
                    }
                }
            }
            reply["files"] = json!(files);
        }
        reply.as_object_mut().unwrap().retain(|key, _| {
            matches!(
                key.as_str(),
                "files"
                    | "forecast"
                    | "violations"
                    | "decision_context"
                    | "evidence_contract"
                    | "evidence_errors"
                    | "protected_paths"
                    | "memory_ids"
                    | "memory_flags"
            )
        });
        reply["kind"] = json!("cant_meet");
        if reply.get("files").is_none() {
            reply["files"] = json!([]);
        }
        reply["report"] = json!(errors.join("\n"));
    }
    Ok(())
}
