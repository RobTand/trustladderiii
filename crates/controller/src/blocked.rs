//! Q50 (Rob, 2026-11-01): a maker that TL3's environment stopped replies
//! `blocked`. The kernel counts it as a failed run; its incident cites the
//! evidence the worker checked.

use crate::{Result, findings, put};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tl3_kernel::NodeId;
use tl3_store::{Commit, RowChange, Snapshot};

pub(crate) const CLASS: &str = "environment_blocked";
const TABLE: &str = "blocked_evidence";
const MAX_REPORT_CHARS: usize = 300;

#[derive(Serialize, Deserialize)]
struct Blocked {
    generation: u64,
    evidence: Value,
    report: String,
}

fn entries(snapshot: &Snapshot, commit: &Commit, node: NodeId) -> Result<Vec<Blocked>> {
    let id = node.0.to_string();
    let value = commit
        .rows
        .iter()
        .rev()
        .find_map(|row| match row {
            RowChange::Put {
                table,
                id: row,
                value,
            } if table == TABLE && *row == id => Some(value),
            _ => None,
        })
        .or_else(|| {
            snapshot
                .rows
                .iter()
                .find(|row| row.table == TABLE && row.id == id)
                .map(|row| &row.value)
        });
    value.map_or(Ok(Vec::new()), |value| crate::decode_stored(value.clone()))
}

/// Keep the evidence of the last two blocked replies, the most one incident needs.
pub(crate) fn record(
    snapshot: &Snapshot,
    commit: &mut Commit,
    node: NodeId,
    generation: u64,
    reply: &Value,
) -> Result<()> {
    let mut entries = entries(snapshot, commit, node)?;
    entries.push(Blocked {
        generation,
        evidence: reply["evidence"].clone(),
        report: findings::clipped(reply["report"].as_str().unwrap_or(""), MAX_REPORT_CHARS),
    });
    let stale = entries.len().saturating_sub(2);
    entries.drain(..stale);
    commit
        .rows
        .push(put(TABLE, node.0.to_string(), json!(entries)));
    Ok(())
}

/// The evidence of blocked replies among the two failed runs that just raised
/// an incident. Each failed run moves the node to the next generation.
pub(crate) fn evidence(
    snapshot: &Snapshot,
    commit: &Commit,
    node: NodeId,
    generation: u64,
) -> Result<Option<String>> {
    let cited = entries(snapshot, commit, node)?
        .into_iter()
        .filter(|entry| entry.generation.saturating_add(2) >= generation)
        .map(|entry| {
            let source = match (
                entry.evidence["action_key"].as_str(),
                entry.evidence["tool_error"].as_str(),
            ) {
                (Some(key), _) => {
                    format!("PrismaBuild action {key} returned an infrastructure error")
                }
                (None, Some(error)) => format!("the maker's tools recorded {error}"),
                (None, None) => "no recorded evidence".into(),
            };
            format!("{source} (maker's report: {})", entry.report)
        })
        .collect::<Vec<_>>();
    Ok((!cited.is_empty()).then(|| format!("Evidence: {}.", cited.join("; "))))
}
