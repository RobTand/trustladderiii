use crate::{
    Error, Result,
    adapters::prismabuild::{Check, Kind},
};
use serde_json::{Value, json};
use std::collections::BTreeSet;
use tl3_harness_core::node_id::{Mode, NodeId};

pub(crate) struct Fixes {
    pub checks: Vec<Check>,
    pub evidence: Value,
}

pub(crate) fn check(reply: &Value, current: crate::findings::CurrentReview<'_>) -> Result<Fixes> {
    let crate::findings::CurrentReview {
        head: reviewed,
        findings,
    } = current;
    let mappings = reply["reproductions"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    if findings.len() != mappings.len() {
        return Err(Error::Invalid(
            "reproductions must cover every current finding",
        ));
    }
    let mut seen = BTreeSet::new();
    let mut checks = Vec::new();
    let mut evidence = Vec::new();
    for mapping in mappings {
        let id = mapping["finding_id"]
            .as_str()
            .ok_or(Error::Invalid("missing finding id"))?;
        let finding = findings
            .iter()
            .find(|finding| finding["id"] == id)
            .ok_or(Error::Invalid("reproduction names an unknown finding"))?;
        if !seen.insert(id.to_owned()) {
            return Err(Error::Invalid("a reproduction repeats a finding"));
        }
        let mut item = json!({"id": id, "report": finding["report"]});
        if let Some(tests) = mapping["tests"].as_array() {
            let tests = tests
                .iter()
                .map(|test| {
                    let test = test
                        .as_str()
                        .ok_or(Error::Invalid("invalid reproducing test"))?;
                    let test = NodeId::parse(test, Mode::Exact)
                        .map_err(|_| Error::Invalid("expected an exact pytest node id"))?;
                    Ok(test.as_str().to_owned())
                })
                .collect::<Result<Vec<_>>>()?;
            if tests.is_empty() || tests.iter().collect::<BTreeSet<_>>().len() != tests.len() {
                return Err(Error::Invalid("a reproduction needs distinct tests"));
            }
            let proof = mapping
                .get("proof")
                .filter(|proof| {
                    proof["red_message"]
                        .as_str()
                        .is_some_and(|reason| (1..=160).contains(&reason.chars().count()))
                })
                .ok_or(Error::Invalid("a reproduction needs proof.red_message"))?;
            let declaration = serde_json::from_value(proof.clone())
                .map_err(|_| Error::Invalid("a reproduction needs a valid proof declaration"))?;
            item["tests"] = json!(tests);
            item["proof"] = proof.clone();
            checks.push(Check {
                name: format!("reproducing/{id}"),
                kind: Kind::Reproducing,
                tests,
                forcing: None,
                proof: Some(declaration),
                declared_symptom: None,
            });
        } else {
            item["reason"] = mapping["reason"].clone();
        }
        evidence.push(item);
    }
    Ok(Fixes {
        checks,
        evidence: json!({"reviewed_sha": reviewed, "findings": evidence}),
    })
}

#[cfg(test)]
mod reply_defects {
    use super::*;
    #[test]
    fn reproduction_keeps_declared_assertion_reason() {
        let feedback =
            json!([{"kind":"review_finding", "id":"f", "reviewed_sha":"head", "report":"fix"}]);
        let proof = json!({"subjects":[], "observer_support":[], "red_message":"empty greeting"});
        let mut reply = json!({"reproductions":[{"finding_id":"f", "tests":["test_greeting.py::test_empty"], "proof":proof}]});
        let current = || crate::findings::current_review(&feedback, 1, Some("head"));
        let fixes = check(&reply, current()).unwrap();
        assert_eq!(
            serde_json::to_value(fixes.checks[0].proof.as_ref().unwrap()).unwrap(),
            proof
        );
        assert_eq!(fixes.evidence["findings"][0]["proof"], proof);
        reply["reproductions"][0]
            .as_object_mut()
            .unwrap()
            .remove("proof");
        assert!(
            check(&reply, current()).is_err(),
            "an assertion reproducer needs a declared reason"
        );
    }

    #[test]
    fn kind_tagged_vectors_are_valid_rust_json() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../harness/test-vectors/core");
        for kind in [
            "node-id",
            "selector-identity",
            "summary-bound",
            "red-match",
            "limits",
            "judge",
            "phase-fold",
        ] {
            let raw = std::fs::read_to_string(root.join(format!("{kind}.json"))).unwrap();
            let vectors: Value = serde_json::from_str(&raw)
                .unwrap_or_else(|error| panic!("{kind}: invalid shared JSON: {error}"));
            assert_eq!(vectors["kind"], kind);
            assert!(!vectors["cases"].as_array().unwrap().is_empty(), "{kind}");
        }
    }

    #[test]
    fn kind_tagged_node_ids_agree_with_harness_vectors() {
        let vectors: Value = serde_json::from_str(include_str!(
            "../../../../../harness/test-vectors/core/node-id.json"
        ))
        .unwrap();
        assert_eq!(vectors["kind"], "node-id");
        let cases = vectors["cases"].as_array().unwrap();
        let mut checked = 0;
        for case in cases {
            if case["input"]["op"] != "grammar" {
                continue;
            }
            let node = case["input"]["node_id"].as_str().unwrap();
            assert_eq!(
                NodeId::parse(node, Mode::Exact).is_ok(),
                case["expected"]["exact"].as_bool().unwrap(),
                "node-id/{}: {node:?}",
                case["id"]
            );
            checked += 1;
        }
        assert!(checked > 0, "node-id grammar vectors must execute");
    }

    #[test]
    fn exact_node_ids_agree_with_harness_vectors() {
        let vectors: Value = serde_json::from_str(include_str!(
            "../../../../../harness/test-vectors/pytest-node-ids.json"
        ))
        .unwrap();
        let feedback =
            json!([{"kind":"review_finding", "id":"f", "reviewed_sha":"head", "report":"fix"}]);
        let mut mismatches = Vec::new();
        for vector in vectors.as_array().unwrap() {
            let reply = json!({"reproductions":[{"finding_id":"f", "tests":[vector["node_id"]],
                "proof":{"subjects":[], "observer_support":[], "red_message":"expected regression"}}]});
            let current = crate::findings::current_review(&feedback, 1, Some("head"));
            if check(&reply, current).is_ok() != vector["valid"].as_bool().unwrap() {
                mismatches.push(vector.clone());
            }
        }
        assert!(
            mismatches.is_empty(),
            "node-id disagreements: {mismatches:?}"
        );
    }
}

#[cfg(test)]
mod stored_evidence_tests {
    use super::*;
    use crate::findings::ReviewFixes;

    /// #838: every producer branch survives the store boundary byte for byte.
    #[test]
    fn producer_evidence_round_trips_without_rewriting_bytes() {
        let head = "a".repeat(40);
        let proof = json!({"subjects":[], "observer_support":[], "red_message":"empty greeting"});
        for mappings in [
            json!([]),
            json!([{"finding_id":"f", "reason":"文書の訂正"}]),
            json!([{"finding_id":"f", "reason":null}]),
            json!([{"finding_id":"f", "tests":["test_greeting.py::test_empty"], "proof":proof}]),
            json!([{"finding_id":"f", "tests":["test_greeting.py::test_empty"], "proof":proof}, {"finding_id":"g", "reason":"documentation"}]),
        ] {
            let feedback = Value::Array(mappings.as_array().unwrap().iter().map(|m| {
                json!({"kind":"review_finding", "id":m["finding_id"], "reviewed_sha":head, "report":"fix 🦀"})
            }).collect());
            let reply = json!({"reproductions":mappings});
            let current = crate::findings::current_review(&feedback, 1, Some(&head));
            let mut value = check(&reply, current).unwrap().evidence;
            for stored in [false, true] {
                if stored {
                    value["head_sha"] = json!(head);
                }
                let typed = ReviewFixes::parse(value.clone()).unwrap();
                assert_eq!(
                    serde_json::to_vec(&json!(typed)).unwrap(),
                    serde_json::to_vec(&value).unwrap()
                );
            }
        }
    }
}
