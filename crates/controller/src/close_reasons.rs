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

#[cfg(test)]
mod tests {
    use super::*;
    use tl3_kernel::{
        NodeId, ReviewStop,
        report::{FailedRoundCause, ReplyKind},
    };
    use tl3_store::{Epoch, Revision, Snapshot};

    fn transaction() -> Commit {
        Commit::new(
            "close-reasons-test",
            json!(null),
            &Snapshot {
                revision: Revision::ZERO,
                epoch: Epoch::ZERO,
                rows: Vec::new(),
                pending_effects: Vec::new(),
                credit: Vec::new(),
                interned: Vec::new(),
            },
        )
    }

    fn values(commit: &Commit) -> Vec<(&str, &Value)> {
        commit
            .rows
            .iter()
            .map(|row| match row {
                RowChange::Put { table, id, value } if table == "close_reasons" => {
                    (id.as_str(), value)
                }
                _ => panic!("unexpected close-reason row: {row:?}"),
            })
            .collect()
    }

    #[test]
    fn every_stop_cause_has_its_durable_reason() {
        let cases = [
            (StopCause::UserStop, json!({"reason":"user_stop"})),
            (StopCause::Replaced, json!({"reason":"replaced"})),
            (StopCause::PrClosed, json!({"reason":"pr_closed"})),
            (StopCause::Merged, json!({"reason":"merged"})),
            (StopCause::Duplicate, json!({"reason":"duplicate"})),
            (StopCause::AlreadyDone, json!({"reason":"already_done"})),
            (StopCause::NotActionable, json!({"reason":"not_actionable"})),
            (
                StopCause::ReviewStop(ReviewStop::ChecksFailed),
                json!({"reason":"review_stop","review_stop":"ChecksFailed"}),
            ),
        ];
        for (cause, expected) in cases {
            let mut commit = transaction();
            let report = Report {
                entries: vec![ReportEntry::Stopped {
                    node: NodeId(7),
                    cause,
                }],
            };
            record(&report, BTreeMap::new(), &mut commit);
            assert_eq!(values(&commit), vec![("7", &expected)], "{cause:?}");
        }
    }

    #[test]
    fn staged_details_are_removed_before_a_refused_step() {
        let mut commit = transaction();
        let detail = json!({"reason":"protected_paths","paths":["private.txt"]});
        commit.rows = vec![
            put("other", "before".into(), json!(1)),
            put("close_reasons", "7".into(), detail.clone()),
            RowChange::Delete {
                table: "close_reasons".into(),
                id: "old".into(),
            },
            put("other", "after".into(), json!(2)),
        ];
        assert_eq!(
            take_details(&mut commit),
            BTreeMap::from([("7".into(), detail)])
        );
        assert!(matches!(commit.rows.as_slice(), [
            RowChange::Put { table: a, id: b, value: c },
            RowChange::Delete { table: d, id: e },
            RowChange::Put { table: f, id: g, value: h },
        ] if a == "other" && b == "before" && c == &json!(1)
            && d == "close_reasons" && e == "old"
            && f == "other" && g == "after" && h == &json!(2)));
    }

    #[test]
    fn only_matching_not_actionable_stop_uses_staged_details() {
        let detail = json!({"reason":"protected_paths","paths":["private.txt"]});
        let cases = [
            (
                "7",
                detail.clone(),
                StopCause::NotActionable,
                detail.clone(),
            ),
            (
                "8",
                detail.clone(),
                StopCause::NotActionable,
                json!({"reason":"not_actionable"}),
            ),
            (
                "7",
                json!({"reason":"other"}),
                StopCause::NotActionable,
                json!({"reason":"not_actionable"}),
            ),
            (
                "7",
                detail,
                StopCause::UserStop,
                json!({"reason":"user_stop"}),
            ),
        ];
        for (id, detail, cause, expected) in cases {
            let mut commit = transaction();
            commit.rows.push(put("close_reasons", id.into(), detail));
            let details = take_details(&mut commit);
            let report = Report {
                entries: vec![ReportEntry::Stopped {
                    node: NodeId(7),
                    cause,
                }],
            };
            record(&report, details, &mut commit);
            assert_eq!(values(&commit), vec![("7", &expected)]);
        }
    }

    #[test]
    fn non_stop_report_entries_cannot_publish_staged_details() {
        for entries in [
            vec![],
            vec![
                ReportEntry::Stale {
                    node: NodeId(7),
                    reply_kind: ReplyKind::ProtectedPaths,
                },
                ReportEntry::FailedRound {
                    node: NodeId(7),
                    cause: FailedRoundCause::RunFailed,
                },
            ],
        ] {
            let mut commit = transaction();
            commit.rows.push(put(
                "close_reasons",
                "7".into(),
                json!({"reason":"protected_paths"}),
            ));
            let details = take_details(&mut commit);
            record(&Report { entries }, details, &mut commit);
            assert!(commit.rows.is_empty());
        }
    }
}
