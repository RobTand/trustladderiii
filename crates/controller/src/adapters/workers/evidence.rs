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
            "provenance":{"kind":"method"},
            "tool":"run_tests", "executor":"prismabuild_pytest", "runner_features":["base_head", "repeat", "parallel"],
            "artifacts":["cas_receipt", "logs", "sha", "terminal_verdict"], "methods":["declared_symptom"], "protected_files":[]}));
    }
    // Source obligations persist; each role replaces the previous method choices.
    // Unmarked legacy contracts remain mandatory.
    for (inherited, source) in [
        (true, &inputs["ingest"]["evidence_contract"]),
        (true, &inputs["audit_evidence_contract"]),
        (false, &reply["evidence_contract"]),
    ] {
        for requirement in source.as_array().into_iter().flatten() {
            if inherited && requirement["provenance"]["kind"] == "method" {
                continue;
            }
            if !contract.contains(requirement) {
                contract.push(requirement.clone());
            }
        }
    }
    if role == "planner"
        && reply["checks"]
            .as_array()
            .is_some_and(|checks| checks.iter().any(|check| check.get("proof").is_some()))
    {
        let requirement = json!({
            "requirement":"Run the head observer against the base subject and the head subject.",
            "provenance":{"kind":"method"}, "tool":"run_tests", "executor":"prismabuild_pytest",
            "runner_features":["base_head"], "artifacts":["cas_receipt", "terminal_verdict"],
            "methods":["subject_observer"], "protected_files":[]});
        if !contract.contains(&requirement) {
            contract.push(requirement);
        }
    }
    let mut errors = branch_errors;
    if role != "planner" && reply.get("evidence_contract").is_none() {
        return Err(Error::Invalid("worker evidence contract is missing"));
    }
    for requirement in &contract {
        for method in requirement["methods"].as_array().into_iter().flatten() {
            if matches!(
                method.as_str(),
                Some("helper_target_observer" | "helper_target_observer.setup_function")
            ) {
                errors.push(format!("Unsupported evidence requirement: {} (removed proof encoding {}; source obligation retained).",
                    requirement["requirement"].as_str().unwrap_or("missing"), method.as_str().unwrap_or("missing")));
            }
        }
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

#[cfg(test)]
mod proof_obligation_tests {
    use super::*;

    #[test]
    fn declared_symptom_choice_is_replaceable_and_source_proof_persists() {
        for required_method in ["red_green", "declared_symptom"] {
            let capability = json!({"tool":"run_tests", "executor":"prismabuild_pytest",
                "runner_features":["base_head","repeat","parallel"],
                "artifacts":["cas_receipt","logs","sha","terminal_verdict"],
                "methods":["red_green","declared_symptom"], "protected_files":[]});
            let mut source = capability.clone();
            source["requirement"] = json!("Prove the timeout.");
            source["methods"] = json!([required_method]);
            source["provenance"] =
                json!({"kind":"source", "source":"issue.body", "quote":"Prove the timeout."});
            let mut inherited = source.clone();
            inherited["methods"] = json!(["helper_target_observer"]);
            inherited["provenance"] = json!({"kind":"method"});
            let inputs = json!({"tool_capabilities":[capability], "ingest":{"evidence_contract":[source,inherited]},
                "audit_evidence_contract":[inherited]});
            let mut reply = json!({"kind":"leaf", "evidence_contract":[], "checks":[{"declared_symptom":{
                "node":"tests/test_original.py::test_original", "exception":"builtins.TimeoutError",
                "phase":"call", "repeat":2, "parallel":0}}]});
            preflight("planner", &inputs, &mut reply).unwrap();
            assert_eq!(reply["kind"], "leaf", "{reply}");
            let contract = reply["evidence_contract"].as_array().unwrap();
            assert!(contract.contains(&source));
            assert!(!contract.contains(&inherited));
            let chosen: Vec<_> = contract.iter().filter(|entry| **entry != source).collect();
            assert_eq!(chosen.len(), 1);
            assert_eq!(chosen[0]["methods"], json!(["declared_symptom"]));
            assert_eq!(chosen[0]["provenance"], json!({"kind":"method"}));
            let next_inputs = json!({"tool_capabilities":inputs["tool_capabilities"], "ingest":reply,
                "audit_evidence_contract":reply["evidence_contract"]});
            let mut replacement = json!({"kind":"leaf", "evidence_contract":[], "checks":[]});
            preflight("planner", &next_inputs, &mut replacement).unwrap();
            assert_eq!(replacement["kind"], "leaf", "{replacement}");
            assert_eq!(replacement["evidence_contract"], json!([source]));
        }
    }

    #[test]
    fn t613_replaces_method_choices_without_erasing_source_proof() {
        let capability = json!({"tool":"run_tests", "executor":"prismabuild_pytest",
            "runner_features":["base_head"], "artifacts":["cas_receipt","terminal_verdict"],
            "methods":["red_green"], "protected_files":[]});
        let mut source = capability.clone();
        source["requirement"] = json!("Show assertion RED and GREEN.");
        source["provenance"] = json!({"kind":"source", "source":"issue.body", "quote":"Show assertion RED and GREEN."});
        let mut method = source.clone();
        method["provenance"] = json!({"kind":"method"});
        method["methods"] = json!(["helper_target_observer"]);
        let inputs = json!({"tool_capabilities":[capability], "ingest":{"evidence_contract":[source,method]},
            "audit_evidence_contract":[method]});
        let mut reply = json!({"kind":"leaf", "evidence_contract":[]});
        preflight("planner", &inputs, &mut reply).unwrap();
        assert_eq!(reply["kind"], "leaf", "{reply}");
        assert_eq!(reply["evidence_contract"], json!([source]));
        let mut explicit = inputs.clone();
        explicit["ingest"]["evidence_contract"][0]["methods"] = json!(["helper_target_observer"]);
        let mut refused = json!({"kind":"leaf", "evidence_contract":[]});
        preflight("planner", &explicit, &mut refused).unwrap();
        assert_eq!(refused["kind"], "cant_meet");
        assert_eq!(
            refused["evidence_contract"],
            json!([explicit["ingest"]["evidence_contract"][0]])
        );
        explicit["tool_capabilities"][0]["methods"] =
            json!(["red_green", "helper_target_observer"]);
        let mut alternative = json!({"kind":"leaf", "evidence_contract":[], "checks":[]});
        preflight("planner", &explicit, &mut alternative).unwrap();
        assert_eq!(alternative["kind"], "cant_meet");
        assert!(
            alternative["report"]
                .as_str()
                .unwrap()
                .contains("removed proof encoding helper_target_observer")
        );
    }
}

use regex::Regex;

const LIMIT: usize = 80_000;
const REPAIR: &str =
    "Shorten the brief summary to leave room for the issue's verbatim code and test identifiers";
const OBJECTION: &str = "The ingest brief drops verbatim source code or supplied test identifiers. Preserve the issue's code exactly, including its test path, class and method, before planning.";

use tl3_contracts::roles::ingest_retention::{Action, decide};

struct SourceBlock {
    text: String,
    body: String,
    node_ids: Vec<String>,
}

fn normalized_lines(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n")
}

/// Retain source bytes, bounded by the preceding paragraph or previous fence.
fn source_blocks(source: &str) -> Vec<SourceBlock> {
    let path = Regex::new(r"`([^`\n ]+\.py)`").unwrap();
    let class = Regex::new(r"^class\s+([A-Za-z_][A-Za-z_0-9]*)\b").unwrap();
    let test = Regex::new(r"^(?:async\s+)?def\s+(test_[A-Za-z_0-9]+)\s*\(").unwrap();
    let retain = |start, content, body_end, end, previous_end| {
        let prefix = source[previous_end..start].trim_end();
        let mut paragraph = previous_end;
        let mut position = previous_end;
        for line in prefix.split_inclusive('\n') {
            position += line.len();
            if line.trim().is_empty() {
                paragraph = position;
            }
        }
        let context = &source[paragraph..start];
        let mut nodes = Vec::new();
        if let Some(file) = path.captures_iter(context).last() {
            let mut owner: Option<(usize, String)> = None;
            for code in source[content..body_end].lines() {
                let text = code.trim_start();
                let indentation = code.len() - text.len();
                if !text.is_empty()
                    && !text.starts_with('#')
                    && owner
                        .as_ref()
                        .is_some_and(|(level, _)| indentation <= *level)
                {
                    owner = None;
                }
                if let Some(name) = class.captures(text) {
                    owner = Some((indentation, name[1].to_owned()));
                } else if let Some(name) = test.captures(text) {
                    let scope = owner
                        .as_ref()
                        .map_or(String::new(), |(_, name)| format!("{name}::"));
                    nodes.push(format!("{}::{scope}{}", &file[1], &name[1]));
                }
            }
        }
        SourceBlock {
            text: source[paragraph..end].to_owned(),
            body: normalized_lines(&source[content..body_end]),
            node_ids: nodes,
        }
    };
    let mut blocks = Vec::new();
    let mut offset = 0;
    let mut previous_end = 0;
    let mut opened: Option<(char, usize, usize, usize)> = None;
    for line in source.split_inclusive('\n') {
        let trimmed = line.trim_start_matches(' ');
        let indent = line.len() - trimmed.len();
        if indent <= 3 {
            let marker = trimmed.chars().next().unwrap_or(' ');
            let width = trimmed.chars().take_while(|c| *c == marker).count();
            if let Some((fence, minimum, start, content)) = opened {
                if marker == fence && width >= minimum && trimmed[width..].trim().is_empty() {
                    blocks.push(retain(
                        start,
                        content,
                        offset,
                        offset + line.len(),
                        previous_end,
                    ));
                    previous_end = offset + line.len();
                    opened = None;
                }
            } else if matches!(marker, '`' | '~')
                && width >= 3
                && (marker != '`' || !trimmed[width..].contains('`'))
            {
                opened = Some((marker, width, offset, offset + line.len()));
            }
        }
        offset += line.len();
    }
    if let Some((_, _, start, content)) = opened {
        blocks.push(retain(
            start,
            content,
            source.len(),
            source.len(),
            previous_end,
        ));
    }
    blocks
}

fn preserve_ingest(role: &str, inputs: &Value, reply: &mut Value) -> Result<()> {
    let ingest = if role == "audit" {
        &inputs["ingest"]
    } else {
        &*reply
    };
    if ingest["kind"] != "brief" {
        return Ok(());
    }
    let source = inputs["issue"]["body"].as_str().unwrap_or_default();
    let blocks = source_blocks(source);
    if blocks.is_empty() {
        return Ok(());
    }
    let brief = ingest["brief"]
        .as_str()
        .ok_or(Error::Invalid("invalid ingest brief"))?;
    let present = normalized_lines(brief);
    let mut retained: Vec<String> = Vec::new();
    let mut missing = Vec::new();
    for block in blocks {
        if !present.contains(&block.body) && !retained.iter().any(|unit| unit.contains(&block.body))
        {
            retained.push(normalized_lines(&block.text));
            missing.push(block.text);
        }
        for node in block.node_ids {
            if !present.contains(&node) && !retained.iter().any(|unit| unit.contains(&node)) {
                retained.push(node.clone());
                missing.push(format!("Source test identifiers:\n{node}"));
            }
        }
    }
    let appendix = format!("\n\nVerbatim issue material:\n{}", missing.join("\n\n"));
    let fits = brief.chars().count() + appendix.chars().count() <= LIMIT;
    match decide(role == "audit", missing.is_empty(), fits) {
        Action::Keep => return Ok(()),
        Action::Repair => return Err(Error::Invalid(REPAIR)),
        Action::Object => {
            reply["verdict"] = json!("disagree");
            let objections = reply["objections"]
                .as_array_mut()
                .ok_or(Error::Invalid("audit objections missing"))?;
            if !objections.contains(&json!(OBJECTION)) {
                objections.push(json!(OBJECTION));
            }
            return Ok(());
        }
        Action::Append => (),
    }
    reply["brief"] = json!(format!("{brief}{appendix}"));
    Ok(())
}

pub fn check_ingest_reply(
    role: &str,
    inputs: &Value,
    reply: &Value,
) -> Result<tl3_contracts::roles::ingest_reply::PreservedIngestReply> {
    let mut reply = reply.clone();
    let violations = match preserve_ingest(role, inputs, &mut reply) {
        Ok(()) => Vec::new(),
        Err(Error::Invalid(message)) => vec![message.to_owned()],
        Err(error) => return Err(error),
    };
    Ok(tl3_contracts::roles::ingest_reply::PreservedIngestReply { reply, violations })
}

#[cfg(test)]
mod ingest_tests {
    use super::*;
    use proptest::{prelude::*, test_runner::TestRunner};

    #[test]
    fn fenced_source_survives_json_and_repeated_handoffs() {
        TestRunner::new(ProptestConfig::with_cases(64))
            .run(&("[^`~\\r\\n]{0,80}", 3usize..7, any::<bool>()), |(text, width, tilde)| {
                let fence = if tilde { "~" } else { "`" }.repeat(width);
                let code = format!("class SuppliedTest:\n    def test_example(self):\n        assert {text:?}\n");
                let source = format!("Add `tests/test_supplied.py` with exactly this test.\n\n{fence}python\n{code}{fence}\n");
                let request = json!({"issue":{"body":source}});
                let bytes = serde_json::to_vec(&request).unwrap();
                let decoded: Value = serde_json::from_slice(&bytes).unwrap();
                let mut reply = json!({"kind":"brief","brief":"Implement it."});
                preserve_ingest("ingest", &decoded, &mut reply).unwrap();
                let brief = reply["brief"].as_str().unwrap();
                prop_assert!(brief.contains(&code));
                prop_assert!(brief.contains("tests/test_supplied.py::SuppliedTest::test_example"));
                let once = reply.clone();
                preserve_ingest("ingest", &decoded, &mut reply).unwrap();
                prop_assert_eq!(&once, &reply);
                let mut audit = json!({"verdict":"agree","objections":[]});
                preserve_ingest("audit", &json!({"issue":decoded["issue"],"ingest":reply}), &mut audit).unwrap();
                prop_assert_eq!(audit["verdict"].as_str(), Some("agree"));
                Ok(())
            }).unwrap();
    }

    #[test]
    fn retention_is_atomic_at_unicode_wire_limit_and_audit_keeps_objections() {
        let inputs = json!({"issue":{"body":"Add `tests/test_a.py`.\n\n```python\ndef test_a(): pass\n```\n"}});
        let mut brief = json!({"kind":"brief","brief":"é".repeat(LIMIT)});
        let original = brief.clone();
        assert_eq!(
            preserve_ingest("ingest", &inputs, &mut brief)
                .unwrap_err()
                .to_string(),
            REPAIR
        );
        assert_eq!(brief, original);
        let mut audit = json!({"verdict":"disagree","objections":["Existing finding"]});
        let inputs = json!({"issue":inputs["issue"],"ingest":brief});
        preserve_ingest("audit", &inputs, &mut audit).unwrap();
        preserve_ingest("audit", &inputs, &mut audit).unwrap();
        assert_eq!(audit["objections"], json!(["Existing finding", OBJECTION]));
        let mut early = json!({"kind":"early_end","outcome":"duplicate"});
        let original = early.clone();
        preserve_ingest("ingest", &inputs, &mut early).unwrap();
        assert_eq!(early, original);
    }

    #[test]
    fn fence_width_and_multiple_source_blocks_are_preserved() {
        let source = "```python\none\n````\n\n~~~text\ntwo\n~~~\n";
        assert_eq!(source_blocks(source).len(), 2);
        assert_eq!(source_blocks("```python\nunterminated").len(), 1);
        assert!(source_blocks("```python`invalid\nnot a fence\n").is_empty());
        assert!(source_blocks("    ```python\nindented\n    ```\n").is_empty());
    }

    #[test]
    fn retained_units_allow_different_wrapping_and_line_endings() {
        let inputs = json!({"issue":{"body":"Add `tests/test_a.py`.\r\n\r\n```python\r\ndef test_a(): pass\r\n```\r\n"}});
        let mut reply = json!({"kind":"brief","brief":"Supplied code:\ndef test_a(): pass\n\nNode: tests/test_a.py::test_a"});
        let original = reply.clone();
        preserve_ingest("ingest", &inputs, &mut reply).unwrap();
        assert_eq!(
            reply, original,
            "body and identifier are independently complete"
        );
        let mut audit = json!({"verdict":"agree","objections":[]});
        preserve_ingest(
            "audit",
            &json!({"issue":inputs["issue"],"ingest":reply}),
            &mut audit,
        )
        .unwrap();
        assert_eq!(audit["verdict"], "agree");
    }

    #[test]
    fn repeated_bodies_and_identifiers_are_retained_once() {
        let block = "Add `tests/test_a.py`.\n\n```python\ndef test_a(): pass\n```\n";
        let inputs = json!({"issue":{"body":format!("{block}\n{block}")}});
        let mut reply = json!({"kind":"brief","brief":"Summary"});
        preserve_ingest("ingest", &inputs, &mut reply).unwrap();
        let brief = reply["brief"].as_str().unwrap();
        assert_eq!(brief.matches("def test_a(): pass").count(), 1);
        assert_eq!(brief.matches("tests/test_a.py::test_a").count(), 1);
        let once = reply.clone();
        preserve_ingest("ingest", &inputs, &mut reply).unwrap();
        assert_eq!(reply, once);
        let mut already_code = json!({"kind":"brief","brief":"def test_a(): pass\n"});
        preserve_ingest("ingest", &inputs, &mut already_code).unwrap();
        assert_eq!(
            already_code["brief"]
                .as_str()
                .unwrap()
                .matches("def test_a(): pass")
                .count(),
            1
        );
        assert!(
            already_code["brief"]
                .as_str()
                .unwrap()
                .contains("tests/test_a.py::test_a")
        );
        let mut already_node = json!({"kind":"brief","brief":"tests/test_a.py::test_a"});
        preserve_ingest("ingest", &inputs, &mut already_node).unwrap();
        let brief = already_node["brief"].as_str().unwrap();
        assert_eq!(brief.matches("tests/test_a.py::test_a").count(), 1);
        assert_eq!(brief.matches("def test_a(): pass").count(), 1);
    }

    #[test]
    fn source_paragraphs_stop_at_crlf_blanks_and_previous_fences() {
        for separator in ["\r\n", "\r\n\r\n"] {
            let source = format!(
                "Earlier context\r\n\r\n```text\r\none\r\n```{separator}Second context\r\n\r\n~~~text\r\ntwo\r\n~~~\r\n"
            );
            let mut reply = json!({"kind":"brief","brief":"Summary"});
            preserve_ingest("ingest", &json!({"issue":{"body":source}}), &mut reply).unwrap();
            let brief = reply["brief"].as_str().unwrap();
            assert_eq!(brief.matches("Earlier context").count(), 1);
            assert_eq!(brief.matches("one\r\n").count(), 1);
            assert_eq!(brief.matches("Second context").count(), 1);
        }
    }

    #[test]
    fn unclosed_fence_is_retained_and_audited_through_eof() {
        let inputs = json!({"issue":{"body":"```python\nassert '🦀'"}});
        let original = json!({"kind":"brief","brief":"Summary"});
        let mut reply = original.clone();
        preserve_ingest("ingest", &inputs, &mut reply).unwrap();
        assert!(reply["brief"].as_str().unwrap().contains("assert '🦀'"));
        let mut audit = json!({"verdict":"agree","objections":[]});
        preserve_ingest(
            "audit",
            &json!({"issue":inputs["issue"],"ingest":original}),
            &mut audit,
        )
        .unwrap();
        assert_eq!(audit["verdict"], "disagree");
    }

    #[test]
    fn astral_exact_fit_and_one_over_are_atomic() {
        let inputs = json!({"issue":{"body":"```python\nassert '🦀'\n```\n"}});
        let appendix = "\n\nVerbatim issue material:\n```python\nassert '🦀'\n```\n";
        for extra in [0, 1] {
            let mut reply = json!({"kind":"brief","brief":"🦀".repeat(LIMIT - appendix.chars().count() + extra)});
            let original = reply.clone();
            let result = preserve_ingest("ingest", &inputs, &mut reply);
            if extra == 0 {
                result.unwrap();
                assert_eq!(reply["brief"].as_str().unwrap().chars().count(), LIMIT);
                assert!(reply["brief"].as_str().unwrap().ends_with(appendix));
            } else {
                assert_eq!(result.unwrap_err().to_string(), REPAIR);
                assert_eq!(reply, original);
            }
        }
    }
}
