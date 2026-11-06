//! Feedback recorded after the kernel decides which replies and failures it accepted.
use crate::{Error, Result, adapters::workers::review, findings, put};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tl3_contracts::roles::maker_input::Feedbackitem;
use tl3_kernel::{Event, FailedRoundCause, NodeId, Phase, Reply, Report, ReportEntry, World};
use tl3_store::{Commit, RowChange, Snapshot, Store};

#[derive(Default)]
pub(crate) struct Evidence {
    pub failed_tests: Vec<findings::FailedTest>,
    pub failure_message: Option<String>,
    pub cant_meet: Option<String>,
    pub ci_checks: Vec<findings::CiFailure>,
    pub outside_paths: Vec<String>,
    pub outside_paths_total: usize,
    pub review: Option<Vec<Feedbackitem>>,
    pub planner: Option<Value>,
    pub rebase: Option<Rebase>,
}

pub(crate) enum Rebase {
    Conflict {
        head: String,
        target: String,
        files: Vec<String>,
    },
    Clean,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct Conflict {
    head: String,
    target: String,
    files: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Document {
    pub generation: u64,
    pub maker: Vec<Feedbackitem>,
    pub planner: Vec<String>,
    conflict: Option<Conflict>,
}

fn row<'a>(
    snapshot: &'a Snapshot,
    changes: &'a [RowChange],
    table: &str,
    id: &str,
) -> Option<&'a Value> {
    for change in changes.iter().rev() {
        match change {
            RowChange::Put {
                table: t,
                id: i,
                value,
            } if t == table && i == id => return Some(value),
            RowChange::Delete { table: t, id: i } if t == table && i == id => return None,
            _ => {}
        }
    }
    snapshot
        .rows
        .iter()
        .find(|r| r.table == table && r.id == id)
        .map(|r| &r.value)
}

fn load_value(source: &dyn Fn(&str) -> Option<Value>, clearing_conflict: bool) -> Value {
    if let Some(mut value) = source("feedback") {
        if clearing_conflict && let Some(fields) = value.as_object_mut() {
            fields.insert("conflict".into(), Value::Null);
        }
        return value;
    }
    // remove the legacy fallback once the live store holds no legacy feedback rows (ops check after the R2 deploy)
    json!({
        "generation": 0,
        "maker": source("maker_findings").unwrap_or_else(|| json!([])),
        "planner": source("findings").unwrap_or_else(|| json!([])),
        "conflict": if clearing_conflict { None } else { source("rebase_conflicts") },
    })
}

pub(crate) fn maker_from(source: &dyn Fn(&str) -> Option<Value>) -> Value {
    load_value(source, false)["maker"].take()
}

pub(crate) fn planner_from(source: &dyn Fn(&str) -> Option<Value>) -> Value {
    load_value(source, false)["planner"].take()
}

fn load(
    snapshot: &Snapshot,
    changes: &[RowChange],
    node: NodeId,
    clearing_conflict: bool,
) -> Result<Document> {
    let id = node.0.to_string();
    crate::decode_stored(load_value(
        &|table| row(snapshot, changes, table, &id).cloned(),
        clearing_conflict,
    ))
}

pub(crate) fn read(snapshot: &Snapshot, node: NodeId) -> Result<Document> {
    load(snapshot, &[], node, false)
}

fn save(commit: &mut Commit, node: NodeId, document: &Document) -> Result<()> {
    commit.rows.push(put(
        "feedback",
        node.0.to_string(),
        serde_json::to_value(document)?,
    ));
    Ok(())
}

pub(crate) fn maker_history(snapshot: &Snapshot, commit: &Commit, node: NodeId) -> Result<Value> {
    Ok(json!(load(snapshot, &commit.rows, node, false)?.maker))
}

impl Document {
    pub(crate) fn maker_input(
        &self,
        initial: Option<&Value>,
        round: u8,
        head: Option<&str>,
    ) -> Result<Vec<Value>> {
        let mut items = match initial {
            Some(value) => value
                .as_array()
                .cloned()
                .ok_or(Error::Invalid("invalid maker feedback"))?,
            None => vec![],
        };
        items.extend(self.maker.iter().map(|item| json!(item)));
        let mut items = bounded_feedback(items, head.filter(|_| round == 1));
        if let Some(conflict) = &self.conflict
            && Some(conflict.head.as_str()) == head
        {
            items.push(json!({"kind":"failed_check", "goal_ids":[],
                "report":findings::clipped(&format!("Resolve the merge onto main {} in these files: {}. The checkout contains the pending merge, both sides in the index, and conflict markers. Preserve the request and upstream changes. Complete the resolution; worker code commits the merge. Do not abort the merge or return to the old base.", json!(conflict.target), json!(conflict.files)), findings::MAX_REPORT_CHARS)
            }));
        }
        Ok(items)
    }
}

pub(crate) fn review_findings(
    blockers: &[review::BlockingFinding],
    sha: &Value,
    generation: u64,
) -> Result<Vec<Feedbackitem>> {
    let formatted: Vec<_> = blockers
        .iter()
        .take(findings::MAX_REVIEW_FINDINGS)
        .map(|finding| {
            format!(
                "{}: {}",
                findings::clipped(&finding.problem, findings::MAX_MESSAGE_CHARS),
                findings::clipped(&finding.evidence, findings::MAX_MESSAGE_CHARS)
            )
        })
        .collect();
    let lengths: Vec<_> = formatted
        .iter()
        .map(|report| report.chars().count())
        .collect();
    let count = tl3_controller_core::translate_worker::findings_prefix(
        &lengths,
        findings::MAX_REVIEW_FINDINGS,
        findings::MAX_REPORT_CHARS,
    );
    blockers.iter().zip(formatted).take(count).enumerate().map(|(index, (finding, report))| {
        let goal_ids = if finding.citation.kind == review::CitationKind::Goal { vec![finding.citation.id.clone()] } else { vec![] };
        Ok(serde_json::from_value(json!({"kind":"review_finding", "id":format!("review-{generation}-{index}"), "reviewed_sha":sha, "report":report, "goal_ids":goal_ids, "outside_paths_total":0}))?)
    }).collect()
}

fn goals(store: &mut Store, node: NodeId) -> Result<Vec<String>> {
    let materials = store.worker_materials(&node.0.to_string())?;
    let Some(goals) = materials
        .as_ref()
        .and_then(|m| m.get("maker").unwrap_or(m).get("goals"))
    else {
        eprintln!(
            "maker finding for node {} has no goal materials; retaining unbound evidence",
            node.0
        );
        return Ok(vec![]);
    };
    goals
        .as_array()
        .ok_or(Error::Invalid("maker goals must be an array"))?
        .iter()
        .map(|goal| {
            goal["id"]
                .as_str()
                .filter(|id| !id.trim().is_empty())
                .map(str::to_owned)
                .ok_or(Error::Invalid("maker goal needs a string id"))
        })
        .collect()
}

pub(crate) fn record(
    store: &mut Store,
    snapshot: &Snapshot,
    world: &World,
    event: Event,
    report: &Report,
    evidence: Option<&Evidence>,
    commit: &mut Commit,
) -> Result<()> {
    let empty = Evidence::default();
    let evidence = evidence.unwrap_or(&empty);
    let stale = |node| {
        report
            .entries
            .iter()
            .any(|entry| matches!(entry, ReportEntry::Stale { node: n, .. } if *n == node))
    };
    for entry in &report.entries {
        let ReportEntry::FailedRound { node, cause } = *entry else {
            continue;
        };
        if stale(node) {
            continue;
        }
        let mut document = load(snapshot, &commit.rows, node, false)?;
        let mut detailed = false;
        if cause == FailedRoundCause::ChangesRequested
            && let Some(review) = &evidence.review
        {
            document.maker.extend(review.iter().cloned());
            detailed = true;
        }
        if !detailed {
            let (kind, text) = match cause {
                FailedRoundCause::ChecksFailed => {
                    let (pass, in_footprint) = match event {
                        Event::Reply { reply: Reply::ChecksDone { pass, in_footprint, .. }, .. } => (pass, in_footprint),
                        _ => (false, true),
                    };
                    (findings::Kind::FailedCheck, findings::check_report(pass, in_footprint, &evidence.failed_tests, &evidence.outside_paths, evidence.outside_paths_total))
                }
                FailedRoundCause::CiFailed => (findings::Kind::FailedCheck, findings::ci_report(&evidence.ci_checks, &evidence.failed_tests)),
                FailedRoundCause::ChangesRequested => (findings::Kind::ReviewFinding, "The independent reviewer requested changes. No detailed review report was supplied.".into()),
                FailedRoundCause::MakerCantMeet => (findings::Kind::ReviewFinding, evidence.cant_meet.clone().unwrap_or_else(|| "The maker could not meet the goals.".into())),
                FailedRoundCause::RunFailed => (findings::Kind::ReviewFinding, evidence.failure_message.as_deref().unwrap_or("The worker run failed. No detailed failure report was supplied.").to_owned()),
            };
            let finding = findings::Finding {
                id: None,
                reviewed_sha: None,
                kind,
                report: findings::clipped(&text, findings::MAX_REPORT_CHARS),
                goal_ids: goals(store, node)?,
                failed_tests: evidence.failed_tests.clone(),
                outside_paths: evidence.outside_paths.clone(),
                outside_paths_total: evidence.outside_paths_total,
                ci_checks: evidence.ci_checks.clone(),
            };
            document
                .maker
                .push(serde_json::from_value(serde_json::to_value(finding)?)?);
        }
        document.generation = world.nodes[node.0 as usize].generation;
        save(commit, node, &document)?;
    }
    if let Event::Reply { node, reply, .. } = event
        && !stale(node)
    {
        let clearing_conflict = matches!(
            (&evidence.rebase, reply),
            (Some(Rebase::Clean), Reply::RebaseClean { .. })
        );
        let mut document = load(snapshot, &commit.rows, node, clearing_conflict)?;
        let before = document.clone();
        if let Some(planner) = &evidence.planner
            && matches!(
                reply,
                Reply::PlanLeaf { .. }
                    | Reply::PlanSplit
                    | Reply::PlanCantMeet
                    | Reply::PlanCheckRejected
                    | Reply::PlanCheckNeedsSplit
                    | Reply::ProtectedPaths
            )
        {
            for violation in crate::adapters::workers::protocol::reply_field::<Vec<String>>(
                planner,
                "violations",
            )? {
                if !document.planner.contains(&violation) {
                    document.planner.push(violation);
                }
            }
            if let Some(text) = planner["report"].as_str() {
                document.planner.push(text.into());
            }
        }
        match (&evidence.rebase, reply) {
            (
                Some(Rebase::Conflict {
                    head,
                    target,
                    files,
                }),
                Reply::RebaseNeedsChanges,
            ) => {
                document.conflict = Some(Conflict {
                    head: head.clone(),
                    target: target.clone(),
                    files: files.clone(),
                });
            }
            (Some(Rebase::Clean), Reply::RebaseClean { .. }) => document.conflict = None,
            _ => {}
        }
        if document != before || clearing_conflict {
            document.generation = world.nodes[node.0 as usize].generation;
            save(commit, node, &document)?;
        }
    }
    Ok(())
}

pub(crate) fn guidance(
    snapshot: &Snapshot,
    commit: &mut Commit,
    node: NodeId,
    generation: u64,
    text: &Value,
) -> Result<()> {
    let mut document = load(snapshot, &commit.rows, node, false)?;
    document.planner.push(
        text.as_str()
            .ok_or(Error::Invalid("invalid findings"))?
            .into(),
    );
    document.generation = generation;
    save(commit, node, &document)
}

pub(crate) fn record_world(
    snapshot: &Snapshot,
    previous: &World,
    world: &World,
    commit: &mut Commit,
) -> Result<()> {
    for (index, node) in world.nodes.iter().enumerate() {
        let replanning = matches!(node.phase, Phase::Planning { .. })
            && previous
                .nodes
                .get(index)
                .is_none_or(|n| !matches!(n.phase, Phase::Planning { .. }));
        let closed = node.merged.is_some() || node.phase.is_closed();
        let id = NodeId(index as u64);
        if closed {
            if previous
                .nodes
                .get(index)
                .is_some_and(|n| n.merged.is_some() || n.phase.is_closed())
            {
                continue;
            }
            let key = id.0.to_string();
            commit.rows.push(RowChange::Delete {
                table: "rebase_conflicts".into(),
                id: key.clone(),
            });
            if let Some(mut value) = row(snapshot, &commit.rows, "feedback", &key).cloned()
                && let Some(fields) = value.as_object_mut()
                && fields.get("conflict").is_some_and(|v| !v.is_null())
            {
                fields.insert("conflict".into(), Value::Null);
                fields.insert("generation".into(), json!(node.generation));
                commit.rows.push(put("feedback", key, value));
            }
            continue;
        }
        if !replanning {
            continue;
        }
        let mut document = load(snapshot, &commit.rows, id, false)?;
        let before = document.clone();
        for item in &document.maker {
            let report = item.report.as_str().to_owned();
            if !document.planner.contains(&report) {
                document.planner.push(report);
            }
        }
        if document != before {
            document.generation = node.generation;
            save(commit, id, &document)?;
        }
    }
    Ok(())
}

pub(crate) const TITLE_CHARS: usize = 120;

fn reviewed(item: &Value) -> Option<&str> {
    if item["kind"] == "review_finding" {
        item["reviewed_sha"].as_str()
    } else {
        None
    }
}

// The round of the first later event that superseded the item at `index`: a review with
// another commit supersedes a review, and any later review or check failure supersedes a
// check failure.
fn superseded_in(feedback: &[Value], rounds: &[u64], index: usize) -> u64 {
    let sha = reviewed(&feedback[index]);
    feedback[index + 1..]
        .iter()
        .zip(&rounds[index + 1..])
        .find(|(later, _)| match sha {
            Some(sha) => reviewed(later).is_some_and(|other| other != sha),
            None => reviewed(later).is_some() || later["kind"] == "failed_check",
        })
        .map_or(rounds[index], |(_, round)| *round)
}

/// What a maker fix round is sent of the accumulated feedback (#316). Findings that a
/// later review or check superseded shrink to a one-line record: kind, id, title and the
/// round of the event that superseded them. The latest review, the reviewed head's
/// findings and the check failure no later review has superseded keep their full text.
/// The store keeps it all.
///
/// A round is one feedback event: a review (all its findings) or a check failure. The
/// order is the append order of the canonical document's maker findings.
pub(crate) fn bounded_feedback(feedback: Vec<Value>, head: Option<&str>) -> Vec<Value> {
    let last_review = feedback.iter().rposition(|item| reviewed(item).is_some());
    let last_sha = last_review.and_then(|index| reviewed(&feedback[index]));
    let last_check = feedback
        .iter()
        .rposition(|item| item["kind"] == "failed_check");
    let mut review_rounds: Vec<(&str, u64)> = Vec::new();
    let mut round = 0u64;
    let rounds: Vec<u64> = feedback
        .iter()
        .map(|item| match reviewed(item) {
            Some(sha) => match review_rounds.iter().find(|(seen, _)| *seen == sha) {
                Some((_, round)) => *round,
                None => {
                    round += 1;
                    review_rounds.push((sha, round));
                    round
                }
            },
            None => {
                round += u64::from(item["kind"] == "failed_check");
                round
            }
        })
        .collect();
    feedback
        .iter()
        .enumerate()
        .map(|(index, item)| {
            let open = match reviewed(item) {
                Some(sha) => Some(sha) == last_sha || Some(sha) == head,
                None => {
                    item["kind"] != "failed_check"
                        || last_check == Some(index) && last_check > last_review
                }
            };
            if open {
                return item.clone();
            }
            let title = item["report"]
                .as_str()
                .unwrap_or_default()
                .lines()
                .map(str::trim)
                .find(|line| !line.is_empty());
            let mut record = serde_json::json!({
                "kind": item["kind"], "goal_ids": item["goal_ids"],
                "report": findings::clipped(title.unwrap_or("(no text)"), TITLE_CHARS),
                "resolved_round": superseded_in(&feedback, &rounds, index),
            });
            for field in ["id", "reviewed_sha"] {
                if !item[field].is_null() {
                    record[field] = item[field].clone();
                }
            }
            record
        })
        .collect()
}

#[cfg(test)]
mod bounded_feedback_defects {
    use super::*;
    use serde_json::{Value, json};

    fn review(id: &str, sha: &str, report: &str) -> Value {
        json!({"kind":"review_finding", "id":id, "reviewed_sha":sha, "report":report, "goal_ids":["g"]})
    }
    fn failed(report: &str) -> Value {
        json!({"kind":"failed_check", "report":report, "goal_ids":[]})
    }
    fn long(tag: &str) -> String {
        format!("{tag} problem: {}", "evidence ".repeat(200))
    }
    fn one_line(item: &Value, kind: &str, round: u64) {
        assert_eq!(item["kind"], kind, "{item}");
        assert_eq!(item["resolved_round"], round, "{item}");
        assert!(
            item["report"].as_str().unwrap().chars().count() <= 120,
            "{item}"
        );
        assert!(item.get("failed_tests").is_none() && item.get("outside_paths").is_none());
    }

    #[test]
    fn earlier_reviews_shrink_to_one_line_records_and_the_latest_stays_whole() {
        let feedback = vec![
            review("review-1-0", "a", &long("first")),
            review("review-1-1", "a", &long("second")),
            review("review-3-0", "b", &long("third")),
            review("review-5-0", "c", &long("fourth")),
        ];
        let sent = bounded_feedback(feedback.clone(), Some("c"));
        assert_eq!(sent.len(), 4, "no finding is dropped");
        assert_eq!(sent[3], feedback[3], "the open finding keeps its full text");
        for (item, id, sha, round, title) in [
            (&sent[0], "review-1-0", "a", 2, "first problem"),
            (&sent[1], "review-1-1", "a", 2, "second problem"),
            (&sent[2], "review-3-0", "b", 3, "third problem"),
        ] {
            one_line(item, "review_finding", round);
            assert_eq!(item["id"], id);
            assert_eq!(item["reviewed_sha"], sha, "the id keeps its commit");
            assert_eq!(item["goal_ids"], json!(["g"]));
            assert!(
                item["report"].as_str().unwrap().starts_with(title),
                "{item}"
            );
        }
    }

    #[test]
    fn a_check_failure_before_a_later_review_is_resolved() {
        let feedback = vec![
            failed(&long("old check")),
            review("review-2-0", "a", &long("review")),
            failed(&long("new check")),
        ];
        let sent = bounded_feedback(feedback.clone(), None);
        one_line(&sent[0], "failed_check", 2);
        assert!(sent[0].get("id").is_none());
        assert_eq!(sent[1], feedback[1], "the latest review is still open");
        assert_eq!(sent[2], feedback[2], "a check failure after it is open");
    }

    #[test]
    fn only_the_latest_check_failure_keeps_full_text() {
        let feedback = vec![
            failed(&long("same")),
            failed(&long("same")),
            failed(&long("last")),
        ];
        let sent = bounded_feedback(feedback.clone(), None);
        one_line(&sent[0], "failed_check", 2);
        one_line(&sent[1], "failed_check", 3);
        assert_eq!(sent[2], feedback[2]);
    }

    #[test]
    fn the_head_under_review_is_open_whatever_the_order() {
        let feedback = vec![
            review("review-1-0", "a", &long("head")),
            review("review-3-0", "b", &long("later")),
        ];
        let sent = bounded_feedback(feedback.clone(), Some("a"));
        assert_eq!(
            sent, feedback,
            "both are open: the reviewed head and the latest review"
        );
    }

    #[test]
    fn short_feedback_with_nothing_to_resolve_is_unchanged() {
        let feedback = vec![
            review("review-1-0", "a", "short"),
            review("review-1-1", "a", "also short"),
        ];
        assert_eq!(bounded_feedback(feedback.clone(), Some("a")), feedback);
        assert!(bounded_feedback(vec![], None).is_empty());
    }

    #[test]
    fn a_title_is_the_first_line_clipped() {
        let feedback = vec![
            failed(&format!("{}\nsecond line", "x".repeat(300))),
            failed("Checks failed.\nmore detail"),
            failed("last"),
        ];
        let sent = bounded_feedback(feedback, None);
        assert_eq!(sent[0]["report"].as_str().unwrap().chars().count(), 120);
        assert_eq!(sent[1]["report"], "Checks failed.");
    }
}
