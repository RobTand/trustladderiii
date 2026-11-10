//! One reviewer validator for controller acceptance and worker repair.
mod decision;
use crate::adapters::workers::review::{BlockingFinding, CitationKind};
use decision::{Finding, Verdict};
use serde_json::Value;

/// Validate the wire boundary before reducing evidence to finite reviewer facts.
pub fn validate(inputs: &Value, reply: &Value) -> crate::Result<()> {
    crate::adapters::workers::protocol::reviewer_schema(inputs, false)?;
    crate::adapters::workers::protocol::reviewer_schema(reply, true)?;
    validate_findings(inputs, reply).map(|_| ())
}

pub(crate) fn reply_violations(inputs: &Value, reply: &Value) -> crate::Result<Vec<&'static str>> {
    crate::adapters::workers::protocol::reviewer_schema(inputs, false)?;
    crate::adapters::workers::protocol::reviewer_schema(reply, true)?;
    let verdict = parse_verdict(reply)?;
    let mut violations = Vec::new();
    if let Err(rule) = decision::head(verdict, reply["head_sha"] == inputs["head_sha"]) {
        violations.push(rule.message());
    }
    if verdict != Verdict::OutOfRoom {
        let findings: Vec<BlockingFinding> =
            serde_json::from_value(reply["blocking_findings"].clone())
                .map_err(|_| crate::Error::Invalid("invalid worker reply field"))?;
        for finding in &findings {
            if let Err(rule) = decision::finding(finding_facts(inputs, finding)) {
                violations.push(rule.message());
            }
        }
    }
    Ok(violations)
}

fn parse_verdict(reply: &Value) -> crate::Result<Verdict> {
    match reply["verdict"].as_str() {
        Some("approve") => Ok(Verdict::Approve),
        Some("changes") => Ok(Verdict::Changes),
        Some("out_of_room") => Ok(Verdict::OutOfRoom),
        _ => Err(crate::Error::Invalid("worker reply breaks role schema")),
    }
}

pub(crate) fn validate_findings(
    inputs: &Value,
    reply: &Value,
) -> crate::Result<Vec<crate::adapters::workers::review::BlockingFinding>> {
    use crate::Error;
    let verdict = parse_verdict(reply)?;
    decision::head(verdict, reply["head_sha"] == inputs["head_sha"])
        .map_err(|v| Error::Invalid(v.message()))?;
    if verdict == Verdict::OutOfRoom {
        return Ok(Vec::new());
    }
    let findings: Vec<crate::adapters::workers::review::BlockingFinding> =
        serde_json::from_value(reply["blocking_findings"].clone())
            .map_err(|_| Error::Invalid("invalid worker reply field"))?;
    for finding in &findings {
        decision::finding(finding_facts(inputs, finding))
            .map_err(|v| Error::Invalid(v.message()))?;
    }
    Ok(findings)
}

fn finding_facts(inputs: &Value, finding: &BlockingFinding) -> Finding {
    let diff = inputs
        .get("scope_diff")
        .unwrap_or(&inputs["diff"])
        .as_str()
        .unwrap_or_default();
    let entries = match finding.citation.kind {
        CitationKind::Goal => &inputs["goals"],
        CitationKind::StandingRule => &inputs["standing_rules"],
    };
    let scope = finding.citation.kind == CitationKind::StandingRule
        && finding.citation.id == crate::adapters::workers::review::SCOPE_RULE;
    Finding {
        known: entries.as_array().is_some_and(|entries| {
            entries
                .iter()
                .any(|entry| entry["id"] == finding.citation.id)
        }),
        scope,
        quoted: !scope || quotes_changed_hunk(diff, &finding.evidence),
    }
}

fn quotes_changed_hunk(diff: &str, evidence: &str) -> bool {
    diff.lines().any(|line| {
        (line.starts_with('+') || line.starts_with('-'))
            && !line.starts_with("+++")
            && !line.starts_with("---")
            && line[1..].chars().filter(|c| !c.is_whitespace()).count() >= 8
            && evidence.lines().any(|quote| quote.trim() == line)
    })
}
