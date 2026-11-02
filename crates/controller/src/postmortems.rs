//! Owed postmortems and code grades (#403, Q32). Both are recorded in the
//! commit that makes them owed, outside the kernel; the judge runs later.

use crate::Result;
use serde_json::{Value, json};
use tl3_kernel::{Node, NodeId, Outcome, Phase, World};
use tl3_store::{CodeGrade, Commit, OwedPostmortem, RowChange, Snapshot};

/// TL3's rubric version, in `rubric.json`. Raise it with any change that could
/// change a grade; regrading adds grades and leaves old ones as they are.
pub(crate) const RUBRIC_VERSION: i32 = 1;

/// Facts about a node's life that its world row no longer holds.
#[derive(Clone, Debug, Default)]
pub(crate) struct History {
    /// The kinds of the questions it asked Rob.
    pub questions: Vec<String>,
    /// Its incidents' cause classes, and whether each needed Rob.
    pub incidents: Vec<(String, bool)>,
    /// A worker run failed and was retried.
    pub run_failure: bool,
    /// Failed runs, effects and plans that were retried, from the retry ledger.
    pub retries: u32,
    /// Times the queue sent it back: CI failures, conflicts and dequeues.
    pub dequeues: u32,
    /// It merged with failing checks.
    pub escaped_ci: bool,
    /// Milliseconds spent in each phase that has ended, in order.
    pub phases: Vec<(String, u64)>,
}

/// Why a postmortem is owed, as the console lists it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Reason {
    Escalation,
    NeededFix,
    Retry,
    ClosedWithoutMerge,
    Sampled,
    RobAsked,
}

impl Reason {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Reason::Escalation => "escalation",
            Reason::NeededFix => "needed_fix",
            Reason::Retry => "retry",
            Reason::ClosedWithoutMerge => "closed_without_merge",
            Reason::Sampled => "sampled",
            Reason::RobAsked => "rob_asked",
        }
    }
}

/// Q32's selection at close: everything that needed a fix, a retry or Rob, and
/// everything that closed without merging, plus a sampled share of clean merges.
/// Returns the reason and, for a sampled node, the share in force.
pub(crate) fn close_reason(
    node: &Node,
    outcome: Outcome,
    history: &History,
    share: f64,
    sample_key: &str,
) -> Option<(Reason, Option<f64>)> {
    let reason = if !history.questions.is_empty()
        || history.incidents.iter().any(|(_, needs_rob)| *needs_rob)
    {
        Reason::Escalation
    } else if node.attempt > 1 || node.round > 0 || node.replanned || node.review_stop.is_some() {
        // Q54: a stop for TL3 review is TL3's own failure to fix.
        Reason::NeededFix
    } else if history.run_failure
        || history.retries > 0
        || !history.incidents.is_empty()
        || node.run_incidents > 0
        || node.fx_incidents > 0
        || node.plan_retries > 0
    {
        Reason::Retry
    } else if outcome != Outcome::Merged {
        Reason::ClosedWithoutMerge
    } else if sampled(sample_key, share) {
        return Some((Reason::Sampled, Some(share)));
    } else {
        return None;
    };
    Some((reason, None))
}

/// The same node is always sampled the same way at the same share.
fn sampled(key: &str, share: f64) -> bool {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(key.as_bytes());
    let bucket = u16::from_be_bytes([digest[0], digest[1]]) % 10_000;
    f64::from(bucket) < share * 10_000.0
}

/// 5 for none, then one grade lower for each more, down to 1.
fn fewer_is_better(count: u32) -> i16 {
    5 - i16::try_from(count.min(4)).unwrap_or(4)
}

/// The rubric's code grades at close: (dimension, grade, fact).
pub(crate) fn close_grades(
    node: &Node,
    outcome: Outcome,
    history: &History,
) -> Vec<(&'static str, Option<i16>, Value)> {
    let merged = outcome == Outcome::Merged;
    let tries = u32::from(node.attempt.saturating_sub(1))
        + u32::from(node.round)
        + u32::from(node.replanned)
        + history.retries;
    let needed_rob = history.questions.len() as u32
        + history
            .incidents
            .iter()
            .filter(|(_, needs_rob)| *needs_rob)
            .count() as u32;
    let queue = if !merged {
        None
    } else if history.escaped_ci {
        Some(1)
    } else {
        Some(5 - i16::try_from(history.dequeues.min(3)).unwrap_or(3))
    };
    let credit = node.credit;
    vec![
        (
            "reliability",
            Some(if merged { fewer_is_better(tries) } else { 1 }),
            json!({"attempt": node.attempt, "round": node.round, "replanned": node.replanned,
                "retries": history.retries, "outcome": format!("{outcome:?}")}),
        ),
        (
            "escalation",
            Some(fewer_is_better(needed_rob)),
            json!({"needed_rob": needed_rob}),
        ),
        (
            "queue",
            queue,
            json!({"merged": merged, "dequeues": history.dequeues, "escaped_ci": history.escaped_ci}),
        ),
        (
            "credit",
            None,
            json!({"start": credit.start, "earned": credit.earned, "added": credit.added,
                "spent": credit.spent, "balance": credit.balance()}),
        ),
        ("questions", None, json!({"kinds": history.questions})),
        (
            "incidents",
            None,
            json!({"classes": history.incidents.iter().map(|(class, _)| class).collect::<Vec<_>>(),
                "needed_rob": history.incidents.iter().filter(|(_, needs_rob)| *needs_rob).count()}),
        ),
        (
            "phase_time",
            None,
            Value::Object(history.phases.iter().fold(
                serde_json::Map::new(),
                |mut seconds, (phase, spent)| {
                    let total = seconds.get(phase).and_then(Value::as_u64).unwrap_or(0) + spent;
                    seconds.insert(phase.clone(), json!(total));
                    seconds
                },
            )),
        ),
    ]
}

/// A postmortem owed for `node`, made owed by `trigger`.
pub(crate) fn owed(
    snapshot: &Snapshot,
    node: NodeId,
    trigger: String,
    reason: Reason,
    sample_share: Option<f64>,
) -> Result<OwedPostmortem> {
    let request = crate::request(snapshot, node)?;
    Ok(OwedPostmortem {
        node: node.0.to_string(),
        trigger,
        reason: reason.as_str().into(),
        sample_share,
        repo: format!("{}/{}", request.repo.owner, request.repo.name),
        issue: i64::try_from(request.issue)
            .map_err(|_| crate::Error::Invalid("issue number overflow"))?,
        title: request.title,
    })
}

/// The kernel zeroes its failure counters once a step succeeds, so each retry
/// is counted in the commit where its counter rises.
fn record_retries(
    snapshot: &Snapshot,
    previous: &World,
    world: &World,
    commit: &mut Commit,
) -> Result<()> {
    for (index, node) in world.nodes.iter().enumerate() {
        let Some(before) = previous.nodes.get(index) else {
            continue;
        };
        let rose = |now: u8, then: u8| u64::from(now > then);
        let added = [
            (
                "run",
                rose(node.run_failures, before.run_failures)
                    + rose(node.run_incidents, before.run_incidents),
            ),
            (
                "effect",
                rose(node.fx_failures, before.fx_failures)
                    + rose(node.fx_incidents, before.fx_incidents),
            ),
            ("plan", rose(node.plan_retries, before.plan_retries)),
        ];
        if added.iter().all(|(_, count)| *count == 0) {
            continue;
        }
        let id = index.to_string();
        let mut counts = commit
            .rows
            .iter()
            .rev()
            .find_map(|row| match row {
                RowChange::Put {
                    table,
                    id: row,
                    value,
                } if table == "retries" && *row == id => Some(value.clone()),
                _ => None,
            })
            .or_else(|| {
                snapshot
                    .rows
                    .iter()
                    .find(|row| row.table == "retries" && row.id == id)
                    .map(|row| row.value.clone())
            })
            .unwrap_or_else(|| json!({}));
        for (kind, count) in added {
            counts[kind] = json!(counts[kind].as_u64().unwrap_or(0) + count);
        }
        commit.rows.push(crate::put("retries", id, counts));
    }
    Ok(())
}

/// A phase's name as the board shows it: Planning, not Planning { must_split }.
fn phase_name(phase: &Value) -> String {
    match phase {
        Value::String(name) => name.clone(),
        Value::Object(fields) => fields.keys().next().cloned().unwrap_or_default(),
        _ => String::new(),
    }
}

impl crate::Controller {
    /// Q32 at close: code grades for every node, and a postmortem for the selected ones.
    pub(crate) fn record_postmortems(
        &mut self,
        snapshot: &Snapshot,
        previous: &World,
        world: &World,
        commit: &mut Commit,
    ) -> Result<()> {
        record_retries(snapshot, previous, world, commit)?;
        for (index, node) in world.nodes.iter().enumerate() {
            let Phase::Closed(outcome) = node.phase else {
                continue;
            };
            if previous
                .nodes
                .get(index)
                .is_some_and(|before| before.phase.is_closed())
            {
                continue;
            }
            let id = NodeId(index as u64);
            let history = self.history(snapshot, id, commit)?;
            for (dimension, grade, fact) in close_grades(node, outcome, &history) {
                commit.grades.push(CodeGrade {
                    node: id.0.to_string(),
                    trigger: "close".into(),
                    rubric_version: RUBRIC_VERSION,
                    dimension: dimension.into(),
                    grade,
                    fact,
                });
            }
            let share = self.store.postmortem_sample_share()?;
            let request = crate::request(snapshot, id)?;
            let sample_key = format!(
                "{}/{}#{}:{}",
                request.repo.owner, request.repo.name, request.issue, id.0
            );
            if let Some((reason, share)) = close_reason(node, outcome, &history, share, &sample_key)
            {
                commit
                    .postmortems
                    .push(owed(snapshot, id, "close".into(), reason, share)?);
            }
        }
        Ok(())
    }

    fn history(&mut self, snapshot: &Snapshot, node: NodeId, commit: &Commit) -> Result<History> {
        let id = node.0.to_string();
        let mut rows: std::collections::BTreeMap<(String, String), Value> = snapshot
            .rows
            .iter()
            .map(|row| ((row.table.clone(), row.id.clone()), row.value.clone()))
            .collect();
        for row in &commit.rows {
            match row {
                RowChange::Put { table, id, value } => {
                    rows.insert((table.clone(), id.clone()), value.clone());
                }
                RowChange::Delete { table, id } => {
                    rows.remove(&(table.clone(), id.clone()));
                }
            }
        }
        let mut history = History::default();
        for ((table, row), value) in &rows {
            match table.as_str() {
                "question_context" if value["node"] == json!(node) => history
                    .questions
                    .push(value["kind"].as_str().unwrap_or("unknown").into()),
                "incidents" if value["node"] == json!(node) => history.incidents.push((
                    value["signature"]["class"]
                        .as_str()
                        .unwrap_or("unknown")
                        .into(),
                    value["needs_rob"] == true,
                )),
                "worker_failures" if *row == id => history.run_failure = true,
                "retries" if *row == id => {
                    history.retries = ["run", "effect", "plan"]
                        .iter()
                        .filter_map(|kind| value[kind].as_u64())
                        .sum::<u64>() as u32;
                }
                "queue_facts" => {
                    let Ok(fact) = serde_json::from_value::<crate::adapters::mergify::QueueFact>(
                        value.clone(),
                    ) else {
                        continue;
                    };
                    if fact.node(snapshot)? != Some(node) {
                        continue;
                    }
                    use crate::adapters::mergify::Evidence;
                    match fact.evidence {
                        Evidence::CiFailed {
                            head_check: false, ..
                        }
                        | Evidence::Conflict { .. }
                        | Evidence::DequeuedByCommand { .. }
                        | Evidence::DequeuedByPerson { .. } => history.dequeues += 1,
                        Evidence::Merged {
                            checks_passed: false,
                            ..
                        } => history.escaped_ci = true,
                        _ => {}
                    }
                }
                _ => {}
            }
        }
        // The phase that ends at close is open-ended here, so it is left out and
        // grades depend only on stored times.
        for pair in self.store.phase_history(&id)?.windows(2) {
            let ((phase, since), (_, until)) = (&pair[0], &pair[1]);
            history
                .phases
                .push((phase_name(phase), u64::try_from(until - since).unwrap_or(0)));
        }
        Ok(history)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tl3_kernel::RepoId;

    fn clean() -> Node {
        let mut node = Node::new(RepoId(0));
        node.attempt = 1;
        node
    }

    fn reason(node: &Node, outcome: Outcome, history: &History) -> Option<Reason> {
        close_reason(node, outcome, history, 0.0, "node:0").map(|(reason, _)| reason)
    }

    #[test]
    fn a_stop_for_tl3_review_owes_needed_fix() {
        // Q54 (#440): a planner cant_meet stop has a plan retry and no fix
        // round, and a maker cant_meet stop may have neither.
        let retried = History {
            retries: 1,
            ..History::default()
        };
        for step in [
            tl3_kernel::ReviewStop::PlanCantMeet,
            tl3_kernel::ReviewStop::MakerCantMeet,
        ] {
            let mut stopped = clean();
            stopped.plan_retries = 1;
            stopped.review_stop = Some(step);
            assert_eq!(
                reason(&stopped, Outcome::Stopped, &retried),
                Some(Reason::NeededFix),
                "{step:?}"
            );
        }
    }

    #[test]
    fn selection_follows_q32() {
        let quiet = History::default();
        assert_eq!(reason(&clean(), Outcome::Merged, &quiet), None);
        let mut fixed = clean();
        fixed.round = 1;
        assert_eq!(
            reason(&fixed, Outcome::Merged, &quiet),
            Some(Reason::NeededFix)
        );
        let mut later = clean();
        later.attempt = 2;
        assert_eq!(
            reason(&later, Outcome::Merged, &quiet),
            Some(Reason::NeededFix)
        );
        let mut replanned = clean();
        replanned.replanned = true;
        assert_eq!(
            reason(&replanned, Outcome::Merged, &quiet),
            Some(Reason::NeededFix)
        );
        let retried = History {
            run_failure: true,
            ..History::default()
        };
        assert_eq!(
            reason(&clean(), Outcome::Merged, &retried),
            Some(Reason::Retry)
        );
        // The kernel zeroes its failure counters once a retry succeeds.
        let recovered = History {
            retries: 1,
            ..History::default()
        };
        assert_eq!(
            reason(&clean(), Outcome::Merged, &recovered),
            Some(Reason::Retry),
            "a retry that succeeded still needed a retry"
        );
        assert!(
            graded(&clean(), Outcome::Merged, &recovered).contains(&("reliability", Some(4))),
            "a retried merge is not a first-time merge"
        );
        let incident = History {
            incidents: vec![("transport".into(), false)],
            ..History::default()
        };
        assert_eq!(
            reason(&clean(), Outcome::Merged, &incident),
            Some(Reason::Retry)
        );
        let asked = History {
            questions: vec!["cant_meet".into()],
            ..History::default()
        };
        assert_eq!(
            reason(&fixed, Outcome::Merged, &asked),
            Some(Reason::Escalation),
            "escalation outranks the fix it led to"
        );
        let needed_rob = History {
            incidents: vec![("permission".into(), true)],
            ..History::default()
        };
        assert_eq!(
            reason(&clean(), Outcome::Merged, &needed_rob),
            Some(Reason::Escalation)
        );
        for outcome in [
            Outcome::Stopped,
            Outcome::Replaced,
            Outcome::Duplicate,
            Outcome::AlreadyDone,
            Outcome::NotActionable,
        ] {
            assert_eq!(
                reason(&clean(), outcome, &quiet),
                Some(Reason::ClosedWithoutMerge),
                "{outcome:?}: every node that closes without merging gets one"
            );
        }
    }

    #[test]
    fn clean_merges_are_sampled_at_the_stored_share() {
        let quiet = History::default();
        assert_eq!(
            close_reason(&clean(), Outcome::Merged, &quiet, 1.0, "node:3"),
            Some((Reason::Sampled, Some(1.0)))
        );
        assert_eq!(
            close_reason(&clean(), Outcome::Merged, &quiet, 0.0, "node:3"),
            None
        );
        let sampled = (0..1000)
            .filter(|n| {
                close_reason(
                    &clean(),
                    Outcome::Merged,
                    &quiet,
                    0.25,
                    &format!("node:{n}"),
                )
                .is_some()
            })
            .count();
        assert!((180..=320).contains(&sampled), "{sampled} of 1000 at 0.25");
        for n in 0..50 {
            let key = format!("node:{n}");
            assert_eq!(
                close_reason(&clean(), Outcome::Merged, &quiet, 0.25, &key),
                close_reason(&clean(), Outcome::Merged, &quiet, 0.25, &key),
                "the same node is always sampled the same way"
            );
        }
    }

    fn graded(
        node: &Node,
        outcome: Outcome,
        history: &History,
    ) -> Vec<(&'static str, Option<i16>)> {
        close_grades(node, outcome, history)
            .into_iter()
            .map(|(dimension, grade, _)| (dimension, grade))
            .collect()
    }

    #[test]
    fn code_grades_record_the_facts_at_close() {
        let quiet = History {
            phases: vec![("Making".into(), 90), ("Checking".into(), 30)],
            ..History::default()
        };
        let grades = graded(&clean(), Outcome::Merged, &quiet);
        for expected in [
            ("reliability", Some(5)),
            ("escalation", Some(5)),
            ("queue", Some(5)),
            ("credit", None),
            ("questions", None),
            ("incidents", None),
            ("phase_time", None),
        ] {
            assert!(grades.contains(&expected), "{expected:?} in {grades:?}");
        }
        let facts = close_grades(&clean(), Outcome::Merged, &quiet);
        let phases = &facts.iter().find(|g| g.0 == "phase_time").unwrap().2;
        assert_eq!(phases["Making"], 90);
        let mut troubled = clean();
        troubled.attempt = 2;
        troubled.round = 1;
        let history = History {
            questions: vec!["cant_meet".into(), "out_of_credit".into()],
            incidents: vec![("permission".into(), true), ("transport".into(), false)],
            dequeues: 1,
            ..History::default()
        };
        let grades = graded(&troubled, Outcome::Merged, &history);
        assert!(grades.contains(&("reliability", Some(3))), "{grades:?}");
        assert!(grades.contains(&("escalation", Some(2))), "{grades:?}");
        assert!(grades.contains(&("queue", Some(4))), "{grades:?}");
        let escaped = History {
            escaped_ci: true,
            ..History::default()
        };
        assert!(graded(&clean(), Outcome::Merged, &escaped).contains(&("queue", Some(1))));
        let stopped = graded(&clean(), Outcome::Stopped, &History::default());
        assert!(stopped.contains(&("reliability", Some(1))), "{stopped:?}");
        assert!(
            stopped.contains(&("queue", None)),
            "an unmerged node has no queue grade"
        );
    }

    #[test]
    fn the_rubric_names_every_code_dimension() {
        let rubric: Value = serde_json::from_str(include_str!("rubric.json")).unwrap();
        assert_eq!(rubric["version"], RUBRIC_VERSION);
        for (dimension, _, _) in close_grades(&clean(), Outcome::Merged, &History::default()) {
            assert_eq!(
                rubric["dimensions"][dimension]["grader"], "code",
                "{dimension}"
            );
        }
    }
}
