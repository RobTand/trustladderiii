//! The deciding worker's findings for a question to Rob (#322). The writer
//! summarises them; the worker process redacts them before any model sees them.

use serde_json::Value;
use tl3_kernel::Question;

/// Longest report passed to the writer; it summarises, and never pastes, the report.
pub(crate) const REPORT_CHARS: usize = 6_000;
const PIECE_CHARS: usize = 600;

/// Findings for `question`, read from the node's stored worker documents and incidents.
pub(crate) fn findings(
    question: &Question,
    document: &dyn Fn(&str) -> Option<Value>,
    incidents: &[Value],
) -> Option<Value> {
    let _ = (question, document, incidents);
    todo!()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tl3_kernel::{Cont, NodeId, QKind, Resume};

    fn fixture(name: &str) -> Value {
        // Bazel declares these through //workers/tests:fixtures as compile data.
        macro_rules! fixture {
            ($($file:literal),*) => {
                match name {
                    $($file => include_str!(concat!(
                        "../../../workers/tests/fixtures/question-findings/", $file
                    )),)*
                    _ => panic!("unknown fixture {name}"),
                }
            };
        }
        serde_json::from_str(fixture!(
            "prismaquant-2662-planner-cant-meet-a.json",
            "prismaquant-2662-planner-cant-meet-b.json",
            "tessera-1464-planner-split.json",
            "audit-disagreement.json",
            "maker-cant-meet.json",
            "out-of-credit.json",
            "effect-failed.json"
        ))
        .unwrap()
    }

    fn question(kind: QKind, cont: Cont) -> Question {
        Question {
            id: 10,
            kind,
            cont,
            cost: 1,
            clarified: false,
        }
    }

    fn from(documents: &Value) -> impl Fn(&str) -> Option<Value> + '_ {
        move |table| documents.get(table).cloned()
    }

    fn text(value: &Value, field: &str) -> String {
        value[field]
            .as_str()
            .unwrap_or_else(|| panic!("findings need {field}: {value}"))
            .to_owned()
    }

    fn assert_one_question(decision: &str) {
        assert!(decision.trim_end().ends_with('?'), "{decision}");
        assert_eq!(decision.matches('?').count(), 1, "{decision}");
    }

    #[test]
    fn planner_cant_meet_reports_carry_their_cause_and_question() {
        // prismaquant#2662, question 10: both planner reports from MLflow, redacted.
        for name in [
            "prismaquant-2662-planner-cant-meet-a.json",
            "prismaquant-2662-planner-cant-meet-b.json",
        ] {
            let documents = json!({"plans": fixture(name)});
            let found = findings(
                &question(QKind::CantMeet, Cont::Planning { must_split: false }),
                &from(&documents),
                &[],
            )
            .unwrap();
            assert_eq!(found["source"], "planner");
            let report = text(&found, "report");
            assert!(report.contains("CURRENT_SOURCE_BLOBS"), "{name}: {report}");
            assert!(report.chars().count() <= REPORT_CHARS);
            let decision = text(&found, "decision");
            assert!(
                decision.contains("may this leaf also edit")
                    && decision.contains("CURRENT_SOURCE_BLOBS"),
                "{name}: the planner's own question must reach Rob: {decision}"
            );
            assert_one_question(&decision);
        }
    }

    #[test]
    fn maker_cant_meet_report_carries_its_question() {
        let documents = fixture("maker-cant-meet.json");
        let found = findings(
            &question(QKind::CantMeet, Cont::Ready(Resume::Making)),
            &from(&documents),
            &[],
        )
        .unwrap();
        assert_eq!(found["source"], "maker");
        assert!(text(&found, "report").contains("read at import time"));
        assert_eq!(
            text(&found, "decision"),
            "Should the cache directory become a constructor argument instead of a module-level setting?"
        );
    }

    #[test]
    fn split_carries_the_size_cause_and_each_proposed_piece() {
        // tessera#1464, question 9: the planner's split from MLflow, redacted.
        let documents = json!({"plans": fixture("tessera-1464-planner-split.json")});
        let found = findings(
            &question(QKind::NeedsSplitting, Cont::Nowhere),
            &from(&documents),
            &[],
        )
        .unwrap();
        assert_eq!(found["source"], "planner");
        let report = text(&found, "report");
        for part in ["docs/ARCHITECTURE.md", "170542", "100000"] {
            assert!(
                report.contains(part),
                "the size cause names {part}: {report}"
            );
        }
        let pieces = found["pieces"].as_array().unwrap();
        assert_eq!(pieces.len(), 2);
        assert!(pieces.iter().all(|piece| {
            piece["brief"].as_str().unwrap().chars().count() <= PIECE_CHARS
                && piece["files"].is_array()
        }));
        assert_one_question(&text(&found, "decision"));
    }

    #[test]
    fn audit_disagreement_carries_the_reading_and_open_objections() {
        let documents = fixture("audit-disagreement.json");
        let found = findings(
            &question(QKind::AuditDisagreement, Cont::Unplanned),
            &from(&documents),
            &[],
        )
        .unwrap();
        assert_eq!(found["source"], "audit");
        assert!(text(&found, "reading").contains("compact JSON export"));
        assert_eq!(found["goals"].as_array().unwrap().len(), 2);
        let objections = found["objections"].as_array().unwrap();
        assert_eq!(objections.len(), 2);
        assert!(objections[0].as_str().unwrap().contains("never says where"));
        assert_one_question(&text(&found, "decision"));
    }

    #[test]
    fn out_of_credit_carries_the_latest_failures() {
        let documents = fixture("out-of-credit.json");
        let found = findings(
            &question(QKind::OutOfCredit, Cont::Ready(Resume::Making)),
            &from(&documents),
            &[],
        )
        .unwrap();
        assert_eq!(found["source"], "checks_and_review");
        let report = text(&found, "report");
        assert!(report.contains("fixed 5 seconds") && report.contains("base passed"));
        assert_one_question(&text(&found, "decision"));
    }

    #[test]
    fn effect_failed_carries_this_node_open_incident() {
        let incidents = fixture("effect-failed.json")["incidents"]
            .as_array()
            .unwrap()
            .clone();
        let found = findings(
            &question(QKind::EffectFailed, Cont::Ready(Resume::Making)),
            &|_| None,
            &incidents
                .into_iter()
                .filter(|incident| incident["node"] == json!(NodeId(0)))
                .collect::<Vec<_>>(),
        )
        .unwrap();
        assert_eq!(found["source"], "incident");
        let report = text(&found, "report");
        assert!(report.contains("signed commits"), "{report}");
        assert!(
            !report.contains("later succeeded"),
            "closed incidents are history"
        );
        assert_one_question(&text(&found, "decision"));
    }

    #[test]
    fn missing_documents_give_no_findings() {
        for kind in [
            QKind::AuditDisagreement,
            QKind::NeedsSplitting,
            QKind::CantMeet,
            QKind::OutOfCredit,
            QKind::EffectFailed,
        ] {
            assert!(
                findings(&question(kind, Cont::Nowhere), &|_| None, &[]).is_none(),
                "{kind:?}"
            );
        }
    }
}
