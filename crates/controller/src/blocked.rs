//! Q50 (Rob, 2026-11-01): a maker that TL3's environment stopped replies
//! `blocked`. The kernel counts it as a failed run; its incident cites the
//! evidence the worker checked.

use crate::Result;
use tl3_kernel::NodeId;
use tl3_store::{Commit, RowChange};

pub(crate) const CLASS: &str = "environment_blocked";

/// Evidence for an incident raised by this commit's blocked reply. Only
/// worker-recorded facts are cited: the reply schema restricts them to an
/// action key, a tool-error id, a tool name and a fixed class. The maker's
/// report is model text, so it stays in the controller's records and never
/// reaches GitHub.
pub(crate) fn evidence(commit: &Commit, node: NodeId) -> Result<Option<String>> {
    let id = node.0.to_string();
    let Some(reply) = commit.rows.iter().rev().find_map(|row| match row {
        RowChange::Put {
            table,
            id: row,
            value,
        } if table == "maker_replies" && *row == id => Some(value),
        _ => None,
    }) else {
        return Ok(None);
    };
    if reply["kind"] != "blocked" {
        return Ok(None);
    }
    let evidence = &reply["evidence"];
    let cited = match (
        evidence["action_key"].as_str(),
        evidence["tool_error"].as_str(),
        evidence["tool"].as_str(),
        evidence["class"].as_str(),
    ) {
        (Some(key), None, None, Some("test_infrastructure")) => {
            format!("PrismaBuild action {key} ended in a test infrastructure error")
        }
        (None, Some(error), Some("run_tests"), Some("test_submission")) => {
            format!("the maker's run_tests tool recorded {error}, a failed test submission")
        }
        (None, Some(error), Some(tool), Some(class)) => {
            format!("the maker's {tool} tool recorded {error}, an {class} error")
        }
        _ => {
            return Err(crate::Error::Invalid(
                "blocked evidence breaks the reply schema",
            ));
        }
    };
    Ok(Some(format!("Evidence: {cited}.")))
}
