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

/// Old arrays and conflict objects remain readable until this node gets its first
/// canonical document. Legacy projections keep question and evidence readers compatible.
fn load(snapshot: &Snapshot, changes: &[RowChange], node: NodeId) -> Result<Document> {
    let id = node.0.to_string();
    if let Some(value) = row(snapshot, changes, "feedback", &id) {
        return crate::decode_stored(value.clone());
    }
    Ok(Document {
        generation: 0,
        maker: row(snapshot, changes, "maker_findings", &id)
            .map(|v| crate::decode_stored(v.clone()))
            .transpose()?
            .unwrap_or_default(),
        planner: row(snapshot, changes, "findings", &id)
            .map(|v| crate::decode_stored(v.clone()))
            .transpose()?
            .unwrap_or_default(),
        conflict: row(snapshot, changes, "rebase_conflicts", &id)
            .map(|v| crate::decode_stored(v.clone()))
            .transpose()?,
    })
}

pub(crate) fn read(snapshot: &Snapshot, node: NodeId) -> Result<Document> {
    load(snapshot, &[], node)
}

fn save(commit: &mut Commit, node: NodeId, document: &Document) -> Result<()> {
    let id = node.0.to_string();
    commit
        .rows
        .push(put("feedback", id.clone(), serde_json::to_value(document)?));
    commit
        .rows
        .push(put("maker_findings", id.clone(), json!(document.maker)));
    commit
        .rows
        .push(put("findings", id.clone(), json!(document.planner)));
    match &document.conflict {
        Some(conflict) => commit
            .rows
            .push(put("rebase_conflicts", id, json!(conflict))),
        None => commit.rows.push(RowChange::Delete {
            table: "rebase_conflicts".into(),
            id,
        }),
    }
    Ok(())
}

pub(crate) fn maker_history(snapshot: &Snapshot, commit: &Commit, node: NodeId) -> Result<Value> {
    Ok(json!(load(snapshot, &commit.rows, node)?.maker))
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
        let mut items = findings::bounded_feedback(items, head.filter(|_| round == 1));
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
        let mut document = load(snapshot, &commit.rows, node)?;
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
                FailedRoundCause::MakerCantMeet => (findings::Kind::ReviewFinding, commit.rows.iter().rev().find_map(|change| match change { RowChange::Put { table, id, value } if table == "maker_replies" && id == &node.0.to_string() => Some(value.to_string()), _ => None }).unwrap_or_else(|| "The maker could not meet the goals.".into())),
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
        let mut document = load(snapshot, &commit.rows, node)?;
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
        if document != before {
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
    let mut document = load(snapshot, &commit.rows, node)?;
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
        if !replanning && !closed {
            continue;
        }
        let id = NodeId(index as u64);
        let mut document = load(snapshot, &commit.rows, id)?;
        let before = document.clone();
        if replanning {
            for item in &document.maker {
                let report = item.report.as_str().to_owned();
                if !document.planner.contains(&report) {
                    document.planner.push(report);
                }
            }
        }
        if closed {
            document.conflict = None;
        }
        if document != before {
            document.generation = node.generation;
            save(commit, id, &document)?;
        }
    }
    Ok(())
}
