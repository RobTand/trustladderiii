//! Request records shared by closing summaries and published proof.
use crate::{Result, executor::Delivery};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tl3_store::{Commit, Snapshot};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Evidence {
    pub checks: Vec<Value>,
    pub reviews: Vec<Value>,
    pub audit: Option<Value>,
    pub queue: Vec<Value>,
    pub scratch: Vec<String>,
    pub goals: Vec<Value>,
    pub remaining_failures: Vec<Value>,
    pub artifacts: Vec<String>,
}

pub fn capture(_snapshot: &Snapshot, _commit: &Commit, _delivery: &Delivery) -> Result<Evidence> {
    todo!("capture recorded request facts")
}

pub fn publication(_delivery: &Delivery, _request: &str) -> Result<String> {
    todo!("render and validate recorded PR evidence")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::{
        github::writer_input,
        workers::writer::{
            FakeWriter, TextDraft, WriteRequest, Writer, WriterReply, checked_draft,
        },
    };
    use serde_json::json;
    use tl3_kernel::{Notice, Outcome};
    use tl3_store::{Epoch, Revision, WorldRow};

    fn delivery() -> Delivery {
        serde_json::from_value(json!({
            "key":"summary:118", "epoch":1, "generation":7,
            "output":{"Notice":{"node":118,"notice":{"Summary":"Stopped"}}},
            "head_sha":"reviewed-head", "repository":{"owner":"RobTand","name":"prismabuild","installation":1},
            "issue":2625,"pull_request":2660,"paths":[],"attempt":1,"round":0
        })).unwrap()
    }

    fn snapshot(rows: Vec<(&str, &str, Value)>) -> Snapshot {
        Snapshot {
            revision: Revision::ZERO,
            epoch: Epoch::ZERO,
            rows: rows
                .into_iter()
                .map(|(table, id, value)| WorldRow {
                    arrival: 0,
                    version: Revision::ZERO,
                    table: table.into(),
                    id: id.into(),
                    value,
                })
                .collect(),
            pending_effects: vec![],
            credit: vec![],
            interned: vec![],
        }
    }

    // Representative recorded shapes for PB #2625 req118. The source trace is
    // tr-3cbbebc9cd399bb9a8742a2018df4b8b; no live database or worker is read.
    fn fixture() -> (Snapshot, Delivery) {
        let d = delivery();
        let s = snapshot(vec![
            (
                "check_reports",
                "118:reviewed-head",
                json!({"base_sha":"base", "results":[{
                    "name":"check-1","kind":"demonstrating","verdict":"pass",
                    "forcing":{"symptom":"PID 4242 is live", "environment":{},"patches":[]},
                    "tests":[{"test":"test_departure","base":"assertion","head":"passed","head_forced":"passed","passed":true}]
                }]}),
            ),
            (
                "review_evidence",
                "118:7",
                json!({"head_sha":"reviewed-head","verdict":"approve","blocking_findings":[],"findings":[]}),
            ),
            ("audits", "118", json!({"verdict":"agree"})),
            (
                "queue_facts",
                "queue-failure",
                json!({"repository":d.repository,"number":2660,
                "evidence":{"CiFailed":{"check":99,"head":"reviewed-head","infra":false,"head_check":false,
                    "details":{"checks":[{"name":"Gate","summary":"Queue check failed","url":"https://github.com/RobTand/prismabuild/actions/runs/99"}]}}}}),
            ),
            (
                "saved_scratch",
                "scratch118",
                json!({"node":118,"reference":"refs/tl3/scratch/issue-2625-node-118"}),
            ),
            (
                "goals",
                "118",
                json!([{"id":"g1","description":"Give red/green evidence in the PR body","verification":"review"}]),
            ),
            (
                "check_reports",
                "119:other-head",
                json!({"results":[{"name":"foreign","verdict":"fail"}]}),
            ),
        ]);
        (s, d)
    }

    #[test]
    fn req118_keeps_completed_forcing_review_and_later_queue_failure() {
        let (s, mut d) = fixture();
        d.evidence = Some(capture(&s, &Commit::new("stop", json!({}), &s), &d).unwrap());
        let input = writer_input(
            &d,
            Some(Notice::Summary(Outcome::Stopped)),
            "Fix process departure",
            "Fix the test",
        )
        .unwrap();
        assert_eq!(
            input.facts["evidence"]["checks"][0]["results"][0]["verdict"],
            "pass"
        );
        assert_eq!(input.facts["evidence"]["reviews"][0]["verdict"], "approve");
        assert_eq!(
            input.facts["evidence"]["queue"].as_array().unwrap().len(),
            1
        );
        assert!(input.facts["unmet_goals"].as_array().unwrap().is_empty());
        let text = checked_draft(&mut FakeWriter, "summary", input, &Default::default()).unwrap();
        assert!(text.contains("checks passed"), "{text}");
        assert!(text.contains("review approved"), "{text}");
        assert!(text.contains("queue"), "{text}");
    }

    #[test]
    fn missing_records_are_not_claimed_as_unperformed_work() {
        let d = delivery();
        let input = writer_input(
            &d,
            Some(Notice::Summary(Outcome::Stopped)),
            "Fix process departure",
            "Fix the test",
        )
        .unwrap();
        assert!(
            input.facts["unmet_goals"].as_array().unwrap().is_empty(),
            "missing facts do not prove unmet goals"
        );
        assert_eq!(input.facts["evidence_status"], "not_captured");
    }

    #[test]
    fn capture_includes_pending_scratch_and_preserves_history_after_report_deletion() {
        let (s, d) = fixture();
        let mut c = Commit::new("close", json!({}), &s);
        c.rows.push(crate::put(
            "saved_scratch",
            "new".into(),
            json!({"node":118,"reference":"new-scratch"}),
        ));
        c.rows.push(crate::put(
            "check_evidence",
            "118:6".into(),
            json!({"head_sha":"earlier-head","pass":false,"results":[]}),
        ));
        c.rows.push(tl3_store::RowChange::Delete {
            table: "check_reports".into(),
            id: "118:reviewed-head".into(),
        });
        let facts = capture(&s, &c, &d).unwrap();
        assert!(facts.scratch.contains(&"new-scratch".into()));
        assert_eq!(facts.checks.len(), 2);
        assert_eq!(
            facts.artifacts,
            vec!["https://github.com/RobTand/prismabuild/actions/runs/99"]
        );
    }

    struct FalseNegative;
    impl Writer for FalseNegative {
        fn draft(&mut self, _: &str, _: &WriteRequest) -> Result<WriterReply> {
            Ok(WriterReply::Text(TextDraft {
                text: "No tests were run. No review was completed. No work was done.".into(),
            }))
        }
    }

    #[test]
    fn writer_rejects_unsupported_negative_claims() {
        let input = writer_input(
            &delivery(),
            Some(Notice::Summary(Outcome::Stopped)),
            "Fix process departure",
            "Fix the test",
        )
        .unwrap();
        assert!(matches!(
            checked_draft(&mut FalseNegative, "summary", input, &Default::default()),
            Err(crate::Error::MessageRejected)
        ));
    }

    #[test]
    fn required_pr_proof_is_rendered_from_checks_and_forcing_results() {
        let (s, mut d) = fixture();
        d.evidence = Some(capture(&s, &Commit::new("publish", json!({}), &s), &d).unwrap());
        let body = publication(&d, "The PR body must give red/green evidence.").unwrap();
        for fact in [
            "check-1",
            "pass",
            "head_forced",
            "PID 4242 is live",
            "approve",
        ] {
            assert!(body.contains(fact), "missing {fact}: {body}");
        }
    }

    #[test]
    fn required_pr_proof_fails_closed_when_missing_or_for_another_head() {
        let mut d = delivery();
        for evidence in [
            None,
            Some(Evidence {
                checks: vec![json!({"head_sha":"other","results":[{"verdict":"pass"}]})],
                ..Default::default()
            }),
        ] {
            d.evidence = evidence;
            assert!(publication(&d, "Publish red/green evidence in the PR body.").is_err());
        }
    }

    #[test]
    fn pr_without_proof_requirement_can_have_no_recorded_checks() {
        assert_eq!(
            publication(&delivery(), "Update the documentation.").unwrap(),
            ""
        );
    }
}
