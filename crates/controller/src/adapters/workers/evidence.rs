//! Recheck worker evidence against the capabilities saved with its request.
use crate::{Error, Result};
use serde_json::{Value, json};

pub(super) fn preflight(role: &str, inputs: &Value, reply: &mut Value) -> Result<()> {
    let goals = if role == "audit" {
        &inputs["ingest"]["goals"]
    } else {
        &reply["goals"]
    };
    let branch_errors = crate::goals::branch_objections(&inputs["publication_branch"], goals)?;
    if role == "audit" && !branch_errors.is_empty() {
        reply["verdict"] = json!("disagree");
        let objections = reply["objections"]
            .as_array_mut()
            .ok_or(Error::Invalid("audit objections missing"))?;
        for error in &branch_errors {
            if !objections.contains(&json!(error)) {
                objections.push(json!(error));
            }
        }
    }
    let Some(capabilities) = inputs["tool_capabilities"].as_array() else {
        // #372: replies to requests started by the previous release still parse.
        return Ok(());
    };
    if !matches!(role, "ingest" | "audit" | "planner") {
        return Ok(());
    }
    let mut contract = Vec::new();
    if role == "planner"
        && reply["checks"]
            .as_array()
            .is_some_and(|checks| checks.iter().any(|c| c.get("declared_symptom").is_some()))
    {
        contract.push(json!({
            "requirement":"Observe the declared exception at the original node and phase in a fixed campaign.",
            "tool":"run_tests", "executor":"prismabuild_pytest", "runner_features":["base_head", "repeat", "parallel"],
            "artifacts":["cas_receipt", "logs", "sha", "terminal_verdict"], "methods":["declared_symptom"], "protected_files":[]}));
    }
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
    if role == "planner"
        && reply["checks"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|check| {
                check["helper_evidence"]["failure_cases"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .any(|case| {
                        case["args"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .chain(
                                case["kwargs"]
                                    .as_object()
                                    .into_iter()
                                    .flat_map(|o| o.values()),
                            )
                            .any(|argument| argument.get("$setup").is_some())
                    })
            })
    {
        let requirement = json!({
            "requirement":"Construct helper arguments with unchanged existing repository setup functions.",
            "tool":"run_tests", "executor":"prismabuild_pytest", "runner_features":["base_head"],
            "artifacts":["cas_receipt", "terminal_verdict"], "methods":["helper_target_observer.setup_function"], "protected_files":[]});
        if !contract.contains(&requirement) {
            contract.push(requirement);
        }
    }
    let mut errors = branch_errors;
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

#[cfg(test)]
mod branch_tests {
    use super::*;

    #[test]
    fn t604_branch_notation_is_a_constraint_at_ingest_audit_and_publication() {
        let fixture = crate::git_tests::Repo::new();
        let context = json!({"repository":"RobTand/prismabuild", "issue":2646, "node":0,
            "template":"prismabuild-{issue}-tl3-{node}", "branch":"prismabuild-2646-tl3-0"});
        for pattern in [
            "prismabuild-<issue>-tl3-<node>",
            "prismabuild-<issue>[-<word>]",
        ] {
            let goals = json!([{"description":format!("Branch name follows PB's issue-link rule: {pattern}"),
                "verification":"review", "stage":"publishing", "publication":{"branch":pattern,
                "non_draft":true, "body_lines":[], "proof":false}}]);
            let mut inputs = json!({"publication_branch":context, "tool_capabilities":[]});
            let mut ingest = json!({"kind":"brief", "goals":goals, "evidence_contract":[]});
            preflight("ingest", &inputs, &mut ingest).unwrap();
            assert_eq!(ingest["evidence_errors"], json!([]));
            assert_eq!(
                ingest["goals"], goals,
                "keep the original pattern and provenance"
            );
            inputs["ingest"] = ingest;
            let mut audit = json!({"verdict":"agree", "objections":[], "evidence_contract":[]});
            preflight("audit", &inputs, &mut audit).unwrap();
            assert_eq!(audit["verdict"], "agree");
            let mut delivery = fixture.delivery();
            delivery.issue = 2646;
            delivery.evidence = Some(crate::evidence::Evidence {
                goals: goals.as_array().unwrap().clone(),
                ..Default::default()
            });
            crate::evidence::validate_publication(
                &delivery,
                "prismabuild-2646-tl3-0",
                false,
                "body",
            )
            .unwrap();
            for wrong in [
                "prismabuild-26460-tl3-0",
                "prismabuild-2646-",
                "other-2646-tl3-0",
            ] {
                assert!(
                    crate::evidence::validate_publication(&delivery, wrong, false, "body").is_err()
                );
            }
        }
    }

    #[test]
    fn t604_audit_cannot_accept_a_branch_conflict_even_if_worker_agrees() {
        let goals = json!([{"description":"Issue says: Use exactly prismabuild-2646.", "verification":"review",
            "stage":"publishing", "publication":{"branch":"prismabuild-2646", "non_draft":true,"body_lines":[],"proof":false}}]);
        let inputs = json!({"publication_branch":{"repository":"RobTand/prismabuild",
            "issue":2646, "node":200, "template":"prismabuild-{issue}-tl3-{node}",
            "branch":"prismabuild-2646-tl3-200"}, "tool_capabilities":[], "ingest":{"goals":goals}});
        let mut reply = json!({"verdict":"agree", "objections":[], "evidence_contract":[]});
        preflight("audit", &inputs, &mut reply).unwrap();
        assert_eq!(reply["verdict"], "disagree");
        assert!(matches!(
            super::super::protocol::map_reply("audit", &reply).unwrap(),
            super::super::protocol::MappedReply::Kernel(tl3_kernel::Reply::AuditDisagrees)
        ));
        let text = reply["objections"].to_string();
        assert!(text.contains("prismabuild-2646-tl3-200"), "{text}");
        assert!(text.contains("prismabuild-{issue}-tl3-{node}"), "{text}");
        assert_eq!(inputs["ingest"]["goals"], goals);
    }
}
