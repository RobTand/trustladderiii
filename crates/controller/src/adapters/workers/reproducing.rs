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
    let findings = feedback
        .as_array()
        .into_iter()
        .flatten()
        .filter(|finding| {
            reviewed.is_some_and(|sha| finding["reviewed_sha"] == sha)
                && finding["kind"] == "review_finding"
        })
        .collect::<Vec<_>>();
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
                    let mut parts = test.split("::");
                    let path = parts.next().unwrap();
                    let names = parts.collect::<Vec<_>>();
                    if !super::protocol::relative_file(path)
                        || test.contains('\0')
                        || path.starts_with('-')
                        || !path.ends_with(".py")
                        || names.is_empty()
                        || names.iter().any(|name| name.is_empty())
                    {
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
