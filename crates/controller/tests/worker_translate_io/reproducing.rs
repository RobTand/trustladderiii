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
