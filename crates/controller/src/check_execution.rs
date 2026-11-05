//! #826: launcher output is operator evidence, separate from test feedback.
use crate::{Result, contracts::bounded::Text};
use serde::{Deserialize, Serialize};
use tl3_kernel::NodeId;
use tl3_store::{Commit, RowChange, Snapshot};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    SetupFailure,
    UnknownIncomplete,
}

/// A diagnostic read only from a verified, bounded PB action log.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Diagnostic {
    pub outcome: Outcome,
    pub phase: Text<1, 64>,
    pub termination_reason: Text<1, 64>,
    pub test_started: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub test: Option<Text<0, 2048>>,
    pub stderr_tail: Text<1, 1024>,
}

impl Diagnostic {
    pub(crate) fn from_log(raw: &[u8]) -> Option<Self> {
        std::str::from_utf8(raw)
            .ok()?
            .lines()
            .rev()
            .filter_map(|line| line.strip_prefix("TL3_EXECUTION_FAILURE "))
            .filter(|line| line.len() <= 16_384)
            .find_map(|line| serde_json::from_str(line).ok())
    }
}

/// Read the diagnostic for the operation that raised this incident, including
/// a fact written earlier in this transaction. Never copy it into test messages.
pub(crate) fn append(
    snapshot: &Snapshot,
    commit: &Commit,
    node: NodeId,
    summary: &mut String,
) -> Result<()> {
    let id = node.0.to_string();
    let failure = commit
        .rows
        .iter()
        .rev()
        .find_map(|row| match row {
            RowChange::Put {
                table,
                id: row,
                value,
            } if table == "check_execution_failures" && row == &id => Some(value),
            _ => None,
        })
        .or_else(|| {
            snapshot
                .rows
                .iter()
                .find(|row| row.table == "check_execution_failures" && row.id == id)
                .map(|row| &row.value)
        });
    let generation = snapshot
        .rows
        .iter()
        .find(|row| row.table == "nodes" && row.id == id)
        .and_then(|row| row.value["generation"].as_u64());
    if let Some(failure) = failure
        && generation.is_some()
        && failure["generation"].as_u64() == generation
    {
        let diagnostic: Diagnostic = crate::decode_stored(failure["diagnostic"].clone())?;
        // Redact before clipping so the display limit cannot split a secret.
        let stderr = crate::redaction::text(diagnostic.stderr_tail.as_str())?;
        let stderr: String = stderr
            .chars()
            .skip(stderr.chars().count().saturating_sub(512))
            .collect();
        let detail = format!(
            " Check action {}: {} / {}; container stderr tail: {}",
            failure["action_key"].as_str().unwrap_or("unknown"),
            diagnostic.phase.as_str(),
            diagnostic.termination_reason.as_str(),
            crate::evidence::label(&stderr)?
        );
        // Apply the controller's existing secret policy before the operations incident is published.
        summary.push_str(&crate::redaction::text(&detail)?);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// #826: malformed or oversized log text cannot become operator evidence.
    #[test]
    fn only_bounded_execution_diagnostics_are_decoded() {
        let mut value = json!({"outcome":"setup_failure", "phase":"container_start",
            "termination_reason":"incomplete_report", "test_started":false, "stderr_tail":"shim refused"});
        let log = |value: &serde_json::Value| format!("noise\nTL3_EXECUTION_FAILURE {value}\n");
        assert!(Diagnostic::from_log(log(&value).as_bytes()).is_some());
        for bad in [json!("x".repeat(1025)), json!(null)] {
            value["stderr_tail"] = bad;
            assert!(Diagnostic::from_log(log(&value).as_bytes()).is_none());
        }
        assert!(Diagnostic::from_log(b"TL3_EXECUTION_FAILURE \xff\n").is_none());
        assert!(Diagnostic::from_log(b"TL3_EXECUTION_FAILURE {}\n").is_none());
    }
}
