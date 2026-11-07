use crate::put;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use tl3_kernel::report::{Report, ReportEntry, StopCause};
use tl3_store::{Commit, RowChange};

// Translation stages protected-path details in the transaction. Remove them
// before any refusal path; only an executed stop may publish a close reason.
pub(crate) fn take_details(commit: &mut Commit) -> BTreeMap<String, Value> {
    let mut details = BTreeMap::new();
    commit.rows.retain(|change| {
        if let RowChange::Put { table, id, value } = change
            && table == "close_reasons"
        {
            details.insert(id.clone(), value.clone());
            false
        } else {
            true
        }
    });
    details
}

pub(crate) fn record(report: &Report, mut details: BTreeMap<String, Value>, commit: &mut Commit) {
    for entry in &report.entries {
        let ReportEntry::Stopped { node, cause } = entry else {
            continue;
        };
        let id = node.0.to_string();
        let value = match cause {
            StopCause::UserStop => json!({"reason":"user_stop"}),
            StopCause::Replaced => json!({"reason":"replaced"}),
            StopCause::PrClosed => json!({"reason":"pr_closed"}),
            StopCause::Merged => json!({"reason":"merged"}),
            StopCause::Duplicate => json!({"reason":"duplicate"}),
            StopCause::AlreadyDone => json!({"reason":"already_done"}),
            StopCause::NotActionable => details
                .remove(&id)
                .filter(|value| value["reason"] == "protected_paths")
                .unwrap_or_else(|| json!({"reason":"not_actionable"})),
            StopCause::ReviewStop(kind) => json!({"reason":"review_stop", "review_stop":kind}),
        };
        commit.rows.push(put("close_reasons", id, value));
    }
}
