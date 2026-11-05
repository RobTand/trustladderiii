//! #826: launcher output is operator evidence, separate from test feedback.
use crate::Result;
use tl3_kernel::NodeId;
use tl3_store::{Commit, RowChange, Snapshot};

pub use crate::contracts::execution::{Diagnostic, Outcome};

impl Diagnostic {
    pub(crate) fn from_log(raw: &[u8]) -> Option<Self> {
        let line = raw
            .split(|byte| *byte == b'\n')
            .rev()
            .find_map(|line| line.strip_prefix(b"TL3_EXECUTION_FAILURE "))?;
        if line.len() <= 16_384
            && let Ok(diagnostic) = serde_json::from_slice(line)
        {
            return Some(diagnostic);
        }
        // Never log invalid producer text: it can itself carry a secret.
        eprintln!("invalid TL3_EXECUTION_FAILURE record in verified action log");
        Some(Diagnostic {
            outcome: Outcome::UnknownIncomplete,
            phase: crate::contracts::bounded::Text::new("diagnostic_decode")
                .expect("literal bound"),
            termination_reason: crate::contracts::bounded::Text::new(
                "invalid_execution_diagnostic",
            )
            .expect("literal bound"),
            test_started: false,
            test: None,
            stderr_tail: crate::contracts::bounded::Text::new(
                "[malformed execution diagnostic omitted]",
            )
            .expect("literal bound"),
        })
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

    /// #826: the portable pytest edge is pinned to the authoritative Rust policy too.
    #[test]
    fn harness_policy_matches_the_rust_build() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../harness/src/tl3_harness/_redaction_policy.py");
        assert_eq!(
            crate::redaction::generated_harness_policy(),
            std::fs::read_to_string(path).unwrap()
        );
    }

    /// #826 review: a malformed producer record is visible operator evidence.
    #[test]
    fn malformed_diagnostic_is_visible() {
        let diagnostic = Diagnostic::from_log(b"TL3_EXECUTION_FAILURE {}\n");
        assert!(
            diagnostic.is_some(),
            "malformed diagnostic disappeared silently"
        );
    }

    /// #826: malformed or oversized log text cannot become operator evidence.
    #[test]
    fn only_bounded_execution_diagnostics_are_decoded() {
        let mut value = json!({"outcome":"setup_failure", "phase":"container_start",
            "termination_reason":"incomplete_report", "test_started":false, "stderr_tail":"shim refused"});
        let log = |value: &serde_json::Value| format!("noise\nTL3_EXECUTION_FAILURE {value}\n");
        assert!(Diagnostic::from_log(log(&value).as_bytes()).is_some());
        for bad in [json!("x".repeat(1025)), json!(null)] {
            value["stderr_tail"] = bad;
            assert_eq!(
                Diagnostic::from_log(log(&value).as_bytes())
                    .unwrap()
                    .phase
                    .as_str(),
                "diagnostic_decode"
            );
        }
        assert!(Diagnostic::from_log(b"TL3_EXECUTION_FAILURE \xff\n").is_some());
        assert!(Diagnostic::from_log(b"TL3_EXECUTION_FAILURE {}\n").is_some());
    }
}
