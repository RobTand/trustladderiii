use crate::{
    Error, Result,
    adapters::prismabuild::{Check, Kind},
};
use serde_json::{Value, json};
use std::collections::BTreeSet;

pub(crate) struct Fixes {
    pub checks: Vec<Check>,
    pub evidence: Value,
}

pub(crate) fn check(reply: &Value, feedback: &Value, reviewed: Option<&str>) -> Result<Fixes> {
    let findings = crate::findings::current_review_findings(feedback, reviewed);
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
                    if !exact_pytest_node_id(test) {
                        return Err(Error::Invalid("expected an exact pytest node id"));
                    }
                    Ok(test.to_owned())
                })
                .collect::<Result<Vec<_>>>()?;
            if tests.is_empty() || tests.iter().collect::<BTreeSet<_>>().len() != tests.len() {
                return Err(Error::Invalid("a reproduction needs distinct tests"));
            }
            item["tests"] = json!(tests);
            checks.push(Check {
                name: format!("reproducing/{id}"),
                kind: Kind::Reproducing,
                tests,
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

fn exact_pytest_node_id(test: &str) -> bool {
    let mut parts = test.split("::");
    let path = parts.next().unwrap_or_default();
    let names = parts.collect::<Vec<_>>();
    !names.is_empty()
        && names.iter().all(|name| !name.is_empty())
        && path.ends_with(".py")
        && !path.starts_with(['/', '-'])
        && !path.split('/').any(|part| part == "..")
        && !path.contains('\\')
        && !test.contains('\0')
}

#[cfg(test)]
mod reply_defects {
    use super::*;
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
            let reply = json!({"reproductions":[{"finding_id":"f", "tests":[vector["node_id"]]}]});
            if check(&reply, &feedback, Some("head")).is_ok() != vector["valid"].as_bool().unwrap()
            {
                mismatches.push(vector.clone());
            }
        }
        assert!(
            mismatches.is_empty(),
            "node-id disagreements: {mismatches:?}"
        );
    }
}
