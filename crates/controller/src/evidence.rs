//! Request records shared by closing summaries and published proof.
use crate::executor::Output;
use crate::{Result, executor::Delivery};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use tl3_kernel::{Notice, Outcome};
use tl3_store::RowChange;
use tl3_store::{Commit, Snapshot};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Evidence {
    pub checks: Vec<Value>,
    pub reviews: Vec<Value>,
    pub audit: Option<Value>,
    pub queue: Vec<Value>,
    pub scratch: Vec<String>,
    #[serde(default)]
    pub scratch_history: Vec<Value>,
    pub goals: Vec<Value>,
    pub remaining_failures: Vec<Value>,
    pub artifacts: Vec<String>,
}

/// Include writes made by the event that emits the delivery. Check reports and
/// queue facts retired at close remain reporting evidence for that close.
pub fn capture(snapshot: &Snapshot, commit: &Commit, delivery: &Delivery) -> Result<Evidence> {
    let mut rows: BTreeMap<_, _> = snapshot
        .rows
        .iter()
        .map(|row| {
            (
                (row.table.clone(), row.id.clone()),
                (row.arrival, row.value.clone()),
            )
        })
        .collect();
    let last = snapshot.rows.iter().map(|r| r.arrival).max().unwrap_or(0);
    for (index, row) in commit.rows.iter().enumerate() {
        match row {
            RowChange::Put { table, id, value } => {
                let arrival = rows
                    .get(&(table.clone(), id.clone()))
                    .map_or(last + index as i64 + 1, |r| r.0);
                rows.insert((table.clone(), id.clone()), (arrival, value.clone()));
            }
            RowChange::Delete { table, id }
                if !matches!(table.as_str(), "check_reports" | "queue_facts") =>
            {
                rows.remove(&(table.clone(), id.clone()));
            }
            _ => {}
        }
    }
    let id = delivery.node().0.to_string();
    let prefix = format!("{id}:");
    let mut facts = Evidence::default();
    let mut ordered: Vec<_> = rows.into_iter().collect();
    ordered.sort_by_key(|(_, (arrival, _))| *arrival);
    for ((table, row), (_, value)) in &ordered {
        match table.as_str() {
            "check_evidence" if row.starts_with(&prefix) => facts.checks.push(value.clone()),
            "review_evidence" if row.starts_with(&prefix) => facts.reviews.push(value.clone()),
            "audits" if row == &id => facts.audit = Some(value.clone()),
            "goals" if row == &id => facts.goals = value.as_array().cloned().unwrap_or_default(),
            "saved_scratch" if value["node"] == json!(delivery.node()) => {
                if let Some(reference) = value["reference"].as_str() {
                    facts.scratch.push(reference.into());
                }
            }
            "scratch_evidence" if value["node"] == json!(delivery.node()) => {
                facts.scratch_history.push(value.clone())
            }
            "queue_facts" => {
                let fact: crate::adapters::mergify::QueueFact =
                    crate::decode_stored(value.clone())?;
                if fact.repository == delivery.repository
                    && Some(fact.number) == delivery.pull_request
                {
                    facts.queue.push(serde_json::to_value(fact.evidence)?);
                }
            }
            _ => {}
        }
    }
    // Older releases only kept the current check report. Preserve its exact
    // results without inventing a run key or a top-level verdict.
    for ((table, row), (_, value)) in &ordered {
        if table == "check_reports" && row.starts_with(&prefix) {
            let head = &row[prefix.len()..];
            if !facts.checks.iter().any(|record| record["head_sha"] == head) {
                let mut record = value.clone();
                record["head_sha"] = json!(head);
                facts.checks.push(record);
            }
        }
    }
    if let Some(check) = facts
        .checks
        .iter()
        .rev()
        .find(|c| c["head_sha"].as_str() == delivery.head_sha.as_deref())
    {
        if check["pass"] == false || check["report"] == false {
            facts.remaining_failures.push(check.clone());
        } else {
            facts.remaining_failures.extend(
                check["results"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter(|result| result["verdict"] == "fail")
                    .cloned(),
            );
        }
    }
    if let Some(review) = facts
        .reviews
        .iter()
        .rev()
        .find(|r| r["head_sha"].as_str() == delivery.head_sha.as_deref())
    {
        facts.remaining_failures.extend(
            review["blocking_findings"]
                .as_array()
                .into_iter()
                .flatten()
                .cloned(),
        );
    }
    if let Some(queue) = facts.queue.last().filter(|q| {
        q.get("CiFailed").is_some()
            || q.get("Conflict").is_some()
            || q.get("QueueBlocked").is_some()
    }) {
        facts.remaining_failures.push(queue.clone());
    }
    let mut links = BTreeSet::new();
    collect_links(&serde_json::to_value(&facts)?, &mut links);
    facts.artifacts = links.into_iter().collect();
    facts.scratch.sort();
    facts.scratch.dedup();
    Ok(facts)
}

fn collect_links(value: &Value, links: &mut BTreeSet<String>) {
    match value {
        Value::String(text) => {
            for word in text.split_whitespace() {
                let word = word.trim_matches(['(', ')', '[', ']', ',', ';', '"']);
                if word.starts_with("https://") || word.starts_with("http://") {
                    links.insert(word.to_owned());
                }
            }
        }
        Value::Array(values) => values.iter().for_each(|v| collect_links(v, links)),
        Value::Object(values) => values.values().for_each(|v| collect_links(v, links)),
        _ => {}
    }
}

/// Plain statements whose scope is the record, not everything that may have run.
pub fn summary(delivery: &Delivery) -> String {
    let Some(facts) = &delivery.evidence else {
        return "Details of checks and review were not captured in this summary.".into();
    };
    let mut lines = Vec::new();
    for check in &facts.checks {
        let results = check["results"].as_array();
        let passed = check["report"] != false
            && (check["pass"] == true
                || (check.get("pass").is_none()
                    && results.is_some_and(|r| {
                        !r.is_empty() && r.iter().all(|c| c["verdict"] == "pass")
                    })));
        lines.push(if passed {
            "The recorded checks passed."
        } else {
            "Check results were recorded."
        });
        if results.is_some_and(|r| r.iter().any(|c| c.get("forcing").is_some())) {
            lines.push("These checks include forced reproduction results.");
        }
    }
    for review in &facts.reviews {
        lines.push(match review["verdict"].as_str() {
            Some("approve") => "The independent review approved the change.",
            Some("changes") => "The independent review requested changes.",
            _ => "An independent review result was recorded.",
        });
    }
    if facts
        .audit
        .as_ref()
        .is_some_and(|a| a["verdict"] == "agree")
    {
        lines.push("The request's independent audit agreed with its goals.");
    }
    if facts.queue.iter().any(|q| q.get("CiFailed").is_some()) {
        lines.push("The merge queue later reported a check failure.");
    }
    if !facts.remaining_failures.is_empty() {
        lines.push("Recorded failures remain unresolved.");
    }
    if !matches!(
        delivery.output,
        Output::Notice {
            notice: Notice::Summary(Outcome::Merged),
            ..
        }
    ) {
        lines.push("A merge has not been recorded for this request.");
    }
    if facts.checks.is_empty() {
        lines.push("Check results were not captured.");
    }
    if facts.reviews.is_empty() {
        lines.push("A review verdict was not captured.");
    }
    lines.dedup();
    lines.join(" ")
}

fn proof_required(text: &str) -> bool {
    static PUBLICATION: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"(?i)\b(?:pr|pull request|publish(?:ed|ing)?)\b")
            .expect("publication pattern")
    });
    let text = text.to_lowercase();
    PUBLICATION.is_match(&text)
        && ["evidence", "proof", "red/green", "red and green"]
            .iter()
            .any(|word| text.contains(word))
}

/// Only explicit goal-cited blockers establish unmet goals.
pub fn unmet_goals(evidence: Option<&Evidence>) -> Vec<String> {
    let Some(evidence) = evidence else {
        return Vec::new();
    };
    evidence
        .goals
        .iter()
        .filter(|goal| {
            evidence.remaining_failures.iter().any(|failure| {
                failure["citation"]["kind"] == "goal" && failure["citation"]["id"] == goal["id"]
            })
        })
        .filter_map(|goal| goal["description"].as_str().map(str::to_owned))
        .collect()
}

fn current<'a>(records: &'a [Value], delivery: &Delivery) -> Vec<&'a Value> {
    records
        .iter()
        .filter(|record| {
            delivery
                .head_sha
                .as_deref()
                .is_some_and(|head| record["head_sha"] == head)
        })
        .collect()
}

/// PR evidence is rendered by code from records for the published head.
/// Missing proof fails before any GitHub write.
pub fn publication(delivery: &Delivery, request: &str) -> Result<String> {
    let red_green = |text: &str| {
        let text = text.to_lowercase();
        text.contains("red/green") || text.contains("red and green")
    };
    let needs_red_green = red_green(request)
        || delivery
            .evidence
            .as_ref()
            .is_some_and(|e| red_green(&serde_json::to_string(&e.goals).expect("JSON goals")));
    let required = proof_required(request)
        || delivery
            .evidence
            .as_ref()
            .is_some_and(|e| e.goals.iter().any(|g| proof_required(&g.to_string())));
    let checks = delivery
        .evidence
        .as_ref()
        .map(|e| current(&e.checks, delivery))
        .unwrap_or_default();
    let complete = !checks.is_empty()
        && checks.iter().all(|record| {
            record["results"].as_array().is_some_and(|results| {
                !results.is_empty()
                    && results.iter().all(|result| {
                        result["verdict"].as_str().is_some()
                            && (!needs_red_green
                                || result["kind"] != "demonstrating"
                                || result["tests"].as_array().is_some_and(|tests| {
                                    !tests.is_empty()
                                        && tests.iter().all(|test| {
                                            test["base"].as_str().is_some()
                                                && test["head"].as_str().is_some()
                                        })
                                }))
                            && (result.get("forcing").is_none()
                                || result["tests"].as_array().is_some_and(|tests| {
                                    !tests.is_empty()
                                        && tests
                                            .iter()
                                            .all(|test| test["head_forced"].as_str().is_some())
                                }))
                    })
            })
        });
    if required && !complete {
        return Err(crate::Error::Permanent {class:"pr_evidence", source:"The request requires published proof, but completed evidence for this head was not captured".into()});
    }
    if checks.is_empty() {
        return Ok(String::new());
    }
    let reviews = delivery
        .evidence
        .as_ref()
        .map(|e| current(&e.reviews, delivery))
        .unwrap_or_default();
    // JSON escaping keeps external strings from introducing Markdown or markers.
    let records = serde_json::to_string_pretty(
        &json!({"head_sha":delivery.head_sha,"checks":checks,"reviews":reviews}),
    )?
    .replace('`', "\\u0060")
    .replace('<', "\\u003c")
    .replace('>', "\\u003e")
    .replace('@', "\\u0040");
    let records = crate::memory::redact_text(&records);
    Ok(format!(
        "<!-- tl3:recorded-evidence:start -->\n## Recorded evidence\n\n```json\n{records}\n```\n<!-- tl3:recorded-evidence:end -->"
    ))
}

/// Replace only our generated section. Repeated publication preserves other text.
pub fn with_publication(body: &str, evidence: &str) -> String {
    if evidence.is_empty() {
        return body.into();
    }
    let start = "<!-- tl3:recorded-evidence:start -->";
    let end = "<!-- tl3:recorded-evidence:end -->";
    if let Some((before, rest)) = body.split_once(start)
        && let Some((_, after)) = rest.split_once(end)
    {
        return format!("{before}{evidence}{after}");
    }
    format!("{}\n\n{evidence}", body.trim_end())
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
        for request in ["Update the documentation.", "Improve evidence parsing."] {
            assert_eq!(publication(&delivery(), request).unwrap(), "");
        }
    }

    #[test]
    fn repeated_publication_preserves_other_body_text_and_redacts_proof() {
        let (s, mut d) = fixture();
        d.evidence = Some(capture(&s, &Commit::new("publish", json!({}), &s), &d).unwrap());
        d.evidence.as_mut().unwrap().checks[0]["diagnostic"] =
            json!("password=secret-fixture-value");
        let evidence = publication(&d, "Publish proof in the PR body").unwrap();
        assert!(!evidence.contains("secret-fixture-value"));
        let body = with_publication(
            "Refs #2625.\nChange type: test\nUser explanation.",
            &evidence,
        );
        assert_eq!(with_publication(&body, &evidence), body);
        assert!(body.contains("User explanation."));
    }

    #[test]
    fn required_proof_comes_from_goals_and_needs_actual_forcing_results() {
        let (s, mut d) = fixture();
        d.evidence = Some(capture(&s, &Commit::new("publish", json!({}), &s), &d).unwrap());
        d.evidence.as_mut().unwrap().checks[0]["results"][0]["tests"][0]["head_forced"] =
            Value::Null;
        assert!(publication(&d, "Fix the departure test").is_err());
    }

    #[test]
    fn unmet_goals_are_only_the_reviewers_explicit_goal_blockers() {
        let facts = Evidence {
            goals: vec![
                json!({"id":"g1","description":"Publish proof"}),
                json!({"id":"g2","description":"Fix departure"}),
            ],
            remaining_failures: vec![
                json!({"citation":{"kind":"goal","id":"g1"},"problem":"Proof is missing"}),
            ],
            ..Default::default()
        };
        assert_eq!(unmet_goals(Some(&facts)), vec!["Publish proof"]);
    }
    #[test]
    fn published_proof_is_valid_json_and_preserves_verifiable_hashes() {
        let (snapshot, mut delivery) = fixture();
        let head = "a".repeat(40);
        let action = "b".repeat(64);
        delivery.evidence = Some(
            capture(
                &snapshot,
                &Commit::new("publish", json!({}), &snapshot),
                &delivery,
            )
            .unwrap(),
        );
        delivery.head_sha = Some(head.clone());
        let check = &mut delivery.evidence.as_mut().unwrap().checks[0];
        check["head_sha"] = json!(head);
        check["action_key"] = json!(action);
        check["diagnostic"] = json!("password=secret-fixture-value");
        let body = publication(&delivery, "Publish red/green evidence in the PR body").unwrap();
        let data = body
            .split_once("```json\n")
            .unwrap()
            .1
            .split_once("\n```")
            .unwrap()
            .0;
        let proof: Value =
            serde_json::from_str(data).expect("proof must remain valid JSON after redaction");
        assert_eq!(proof["head_sha"], head);
        assert_eq!(proof["checks"][0]["action_key"], action);
        assert!(!body.contains("secret-fixture-value"));
    }
}
