use super::*;
use decision::{Finding,Verdict};
pub(crate) fn validate_findings(
    inputs: &Value,
    reply: &Value,
) -> crate::Result<Vec<crate::adapters::workers::review::BlockingFinding>> {
    use crate::{Error, adapters::workers::review::CitationKind};
    let verdict = match reply["verdict"].as_str() {
        Some("approve") => Verdict::Approve,
        Some("changes") => Verdict::Changes,
        Some("out_of_room") => Verdict::OutOfRoom,
        _ => return Err(Error::Invalid("worker reply breaks role schema")),
    };
    decision::head(verdict, reply["head_sha"] == inputs["head_sha"])
        .map_err(|v| Error::Invalid(v.message()))?;
    if verdict == Verdict::OutOfRoom {
        return Ok(Vec::new());
    }
    let findings: Vec<crate::adapters::workers::review::BlockingFinding> =
        serde_json::from_value(reply["blocking_findings"].clone())
            .map_err(|_| Error::Invalid("invalid worker reply field"))?;
    let diff = inputs
        .get("scope_diff")
        .unwrap_or(&inputs["diff"])
        .as_str()
        .unwrap_or_default();
    for finding in &findings {
        let entries = match finding.citation.kind {
            CitationKind::Goal => &inputs["goals"],
            CitationKind::StandingRule => &inputs["standing_rules"],
        };
        let scope = finding.citation.kind == CitationKind::StandingRule
            && finding.citation.id == crate::adapters::workers::review::SCOPE_RULE;
        decision::finding(Finding {
            known: entries.as_array().is_some_and(|entries| {
                entries
                    .iter()
                    .any(|entry| entry["id"] == finding.citation.id)
            }),
            scope,
            quoted: !scope || quotes_changed_hunk(diff, &finding.evidence),
        })
        .map_err(|v| Error::Invalid(v.message()))?;
    }
    Ok(findings)
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
