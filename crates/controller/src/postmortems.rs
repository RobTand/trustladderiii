//! Owed postmortems and code grades (#403, Q32). Both are recorded in the
//! commit that makes them owed, outside the kernel. The runner then judges one
//! postmortem at a time on spare agent-pool capacity (Q39).

use crate::Result;
use crate::executor::Delivery;
use serde_json::{Value, json};
use tl3_kernel::{Node, NodeId, Outcome, Phase, World};
use tl3_store::{CodeGrade, Commit, OwedPostmortem, PostmortemResult, RowChange, Snapshot, Store};

/// The world table that keeps a claimed postmortem's delivery live.
pub(crate) const JOBS: &str = "postmortem_jobs";

/// Q32: a postmortem recommends at most three changes.
pub(crate) const MAX_RECOMMENDATIONS: usize = 3;

/// Q39: a postmortem starts only on spare agent-pool capacity, so it is the
/// pool's lowest priority. Nothing may wait for a pool grant: a node with a
/// ticket and no grant waits, whether for the pool or for a slot. And the
/// nodes holding the pool, plus the postmortems already `running`, must be
/// fewer than the pool's capacity. The kernel never sees a postmortem, so one
/// that runs while new work starts uses one model session beyond the pool.
/// #417's priority-first pool can order this test with its own.
pub(crate) fn spare_pool_capacity(world: &World, running: usize) -> bool {
    let _ = (world, running);
    todo!("#403: spare pool capacity")
}

/// `rec-` and 16 hex digits of sha256 over the normalised target, operation,
/// before and after: the same change counts the same in every postmortem.
pub(crate) fn fingerprint(target: &str, operation: &str, before: &str, after: &str) -> String {
    let _ = (target, operation, before, after);
    todo!("#403: recommendation fingerprints")
}

/// A postmortem reply as the worker protocol accepts it: the role's schema,
/// and at most three recommendations, which code checks as well.
pub(crate) fn check_reply(reply: &Value) -> Result<()> {
    let _ = reply;
    todo!("#403: check a postmortem reply")
}

/// The postmortem a run claimed, as its result is stored.
#[derive(Clone, Debug)]
pub(crate) struct Claimed {
    pub id: i64,
    pub node: String,
    pub repo: String,
    pub issue: u64,
    pub title: String,
}

/// What a valid reply stores: redacted text, ranked and fingerprinted
/// recommendations, tokens and cache rate as later code grades, and the
/// lessons for Hindsight. A fourth recommendation is refused here too.
pub(crate) fn result(claimed: &Claimed, reply: &Value) -> Result<PostmortemResult> {
    let _ = (claimed, reply);
    todo!("#403: a postmortem's stored result")
}

/// The simulated worker's reply: a fixed, valid postmortem.
pub(crate) fn simulated_reply() -> Value {
    json!({
        "postmortem": {
            "summary": "The maker needed one fix round before the change merged.",
            "cause": "tl3",
            "went_wrong": "The maker's first head failed a check the plan named.",
            "timeline": [{"at": "Making", "event": "The maker ran."},
                         {"at": "Reviewing", "event": "The reviewer asked for a fix."}],
            "process_failures": [{"phase": "maker", "kind": "prompt or skill defect",
                "what": "The maker skipped a planned check.", "evidence": "The review's finding."}],
            "recommendations": [{"phase": "maker", "kind": "prompt", "target": "maker",
                "operation": "add", "before": "", "after": "Run every check the plan names.",
                "evidence": "The review's finding.", "effect": "Fewer fix rounds."}],
            "notes": "",
            "lessons": ["A plan's named checks must all run before a maker replies."]
        },
        "grades": [{"dimension": "correctness", "grade": 4, "rationale": "The review's finding.",
            "judge_model": "simulated", "assessment_id": null}],
        "assessment_ids": [],
        "usage": {"traces": 0, "input_tokens": 0, "output_tokens": 0,
            "cache_read_input_tokens": 0, "cache_creation_input_tokens": 0},
        "models": {"postmortem": "simulated", "grades": "simulated"}
    })
}

/// The case file for one postmortem, written into the worker's inputs: the
/// request, its outcome and grades, phase times, questions with their #322
/// findings, incidents, plan, brief, audit and the release that ran.
pub(crate) fn case_file(
    store: &mut Store,
    snapshot: &Snapshot,
    delivery: &Delivery,
    id: i64,
    models: &tl3_config::Models,
) -> Result<Value> {
    let _ = (store, snapshot, delivery, id, models);
    todo!("#403: a postmortem's case file")
}

impl crate::Controller {
    /// Claim the oldest owed postmortem when the pool has spare capacity, in
    /// the commit that creates its worker delivery.
    pub(crate) fn start_postmortem(&mut self, snapshot: &Snapshot) -> Result<bool> {
        let _ = snapshot;
        Ok(false)
    }

    /// Store a postmortem's result, or its failure, for the claim's run key only.
    pub(crate) fn postmortem_finished(
        &mut self,
        snapshot: &Snapshot,
        key: &str,
        reply: Option<Value>,
        failure: Option<String>,
        commit: &mut Commit,
    ) -> Result<()> {
        let _ = (snapshot, key, reply, failure, commit);
        todo!("#403: store a postmortem's result")
    }

    /// A postmortem whose launch failed permanently fails; it raises no incident.
    pub(crate) fn postmortem_failed(
        &mut self,
        snapshot: &Snapshot,
        delivery: &Delivery,
        class: &str,
        commit: &mut Commit,
    ) -> Result<()> {
        let _ = (snapshot, delivery, class, commit);
        todo!("#403: fail a postmortem")
    }

    /// At startup, a `running` postmortem with no live delivery is owed again.
    pub(crate) fn recover_postmortems(&mut self) -> Result<()> {
        Ok(())
    }
}

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
    } else if node.attempt > 1 || node.round > 0 || node.replanned {
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

    fn world() -> World {
        let mut world = World::new(&[2], 2);
        for _ in 0..3 {
            world.nodes.push(Node::new(RepoId(0)));
        }
        world
    }

    #[test]
    fn a_postmortem_starts_only_on_spare_pool_capacity() {
        let free = world();
        assert!(spare_pool_capacity(&free, 0), "an idle pool has room");
        let mut one = world();
        one.nodes[0].pool = true;
        assert!(spare_pool_capacity(&one, 0), "one of two grants is free");
        assert!(
            !spare_pool_capacity(&one, 1),
            "a running postmortem counts against the pool"
        );
        let mut full = world();
        full.nodes[0].pool = true;
        full.nodes[1].pool = true;
        assert!(!spare_pool_capacity(&full, 0), "every grant is held");
        let mut waiting = world();
        waiting.nodes[2].ticket = Some(7);
        assert!(
            !spare_pool_capacity(&waiting, 0),
            "a node waiting for a grant goes first"
        );
        let mut holding = world();
        holding.nodes[0].pool = true;
        holding.nodes[0].ticket = Some(3);
        assert!(
            spare_pool_capacity(&holding, 0),
            "a holder with a ticket waits for nothing"
        );
    }

    #[test]
    fn fingerprints_are_stable_across_spacing_and_case_of_the_target() {
        let one = fingerprint(
            "maker",
            "replace",
            "Run the tests.",
            "Run  the tests\nfirst.",
        );
        assert!(one.starts_with("rec-"), "{one}");
        assert_eq!(one.len(), 4 + 16, "{one}");
        assert!(
            one[4..]
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        );
        assert_eq!(
            one,
            fingerprint(
                " Maker ",
                "replace",
                " Run the tests. ",
                "Run the tests first."
            )
        );
        assert_ne!(
            one,
            fingerprint("maker", "replace", "Run the tests.", "Run the tests later.")
        );
        assert_ne!(one, fingerprint("maker", "add", "", "Run the tests first."));
        assert_ne!(
            fingerprint("ab", "add", "", "c"),
            fingerprint("a", "add", "", "bc"),
            "fields are delimited"
        );
    }

    fn claimed() -> Claimed {
        Claimed {
            id: 7,
            node: "3".into(),
            repo: "RobTand/tl3-sandbox".into(),
            issue: 42,
            title: "Fix the sandbox build".into(),
        }
    }

    fn recommendation(n: usize) -> Value {
        json!({"phase": "maker", "kind": "prompt", "target": "maker", "operation": "add",
            "before": "", "after": format!("Run check {n}."), "evidence": "The review.",
            "effect": "Fewer fix rounds."})
    }

    fn with_recommendations(count: usize) -> Value {
        let mut reply = simulated_reply();
        reply["postmortem"]["recommendations"] = (0..count).map(recommendation).collect();
        reply
    }

    #[test]
    fn a_fourth_recommendation_is_refused_by_the_schema_and_by_code() {
        check_reply(&with_recommendations(3)).unwrap();
        assert!(check_reply(&with_recommendations(4)).is_err(), "schema");
        assert!(check_reply(&json!({"postmortem": {}})).is_err());
        // Code refuses a fourth even when a reply skipped the schema.
        assert!(
            result(&claimed(), &with_recommendations(4)).is_err(),
            "code"
        );
        result(&claimed(), &with_recommendations(3)).unwrap();
    }

    #[test]
    fn a_result_stores_the_postmortem_its_ranked_recommendations_and_later_grades() {
        let mut reply = with_recommendations(2);
        reply["grades"] = json!([{"dimension": "correctness", "grade": 4,
            "rationale": "The review.", "judge_model": "claude-sonnet-5-5", "assessment_id": "a-9"}]);
        reply["assessment_ids"] = json!(["a-9"]);
        reply["usage"] = json!({"traces": 3, "input_tokens": 1000, "output_tokens": 40,
            "cache_read_input_tokens": 600, "cache_creation_input_tokens": 100});
        let stored = result(&claimed(), &reply).unwrap();
        assert_eq!(stored.cause, "tl3");
        assert_eq!(stored.summary, reply["postmortem"]["summary"]);
        assert_eq!(stored.timeline, reply["postmortem"]["timeline"]);
        assert_eq!(
            stored.process_failures,
            reply["postmortem"]["process_failures"]
        );
        assert_eq!(stored.assessment_ids, json!(["a-9"]));
        assert_eq!(stored.grades[0]["dimension"], "correctness");
        assert_eq!(stored.grades[0]["grade"], 4);
        let ranks: Vec<_> = stored.recommendations.iter().map(|r| r.rank).collect();
        assert_eq!(ranks, [1, 2]);
        let first = &stored.recommendations[0];
        assert_eq!(
            first.fingerprint,
            fingerprint(&first.target, &first.operation, &first.before, &first.after)
        );
        assert_eq!(first.after, "Run check 0.");
        let grades: Vec<_> = stored
            .code_grades
            .iter()
            .map(|g| {
                (
                    g.dimension.as_str(),
                    g.trigger.as_str(),
                    g.node.as_str(),
                    g.grade,
                )
            })
            .collect();
        assert_eq!(
            grades,
            [
                ("tokens", "postmortem:7", "3", None),
                ("cache_rate", "postmortem:7", "3", None)
            ]
        );
        assert_eq!(stored.code_grades[0].fact["input_tokens"], 1000);
        assert_eq!(stored.code_grades[1].fact["rate"], 0.6);
        let (operation, item) = stored.memory.clone().expect("lessons go to Hindsight");
        assert_eq!(operation.len(), 36, "{operation}");
        assert_eq!(
            result(&claimed(), &reply).unwrap().memory.unwrap().0,
            operation,
            "a replay retains the same memory"
        );
        let tags: Vec<_> = item["tags"].as_array().unwrap().iter().collect();
        assert!(tags.contains(&&json!("postmortem")), "{tags:?}");
        assert!(
            tags.contains(&&json!("repo:RobTand/tl3-sandbox")),
            "{tags:?}"
        );
        assert!(
            item["content"]
                .as_str()
                .unwrap()
                .contains("A plan's named checks")
        );
    }

    #[test]
    fn stored_and_retained_text_is_redacted() {
        let planted = [
            "sk-planted403abcdefghijklmnop",
            "ghp_planted403abcdefghijklmnop",
            "0123456789abcdef0123456789abcdef01234567",
        ];
        let text = format!(
            "The key {} with token={} and {}.",
            planted[0], planted[1], planted[2]
        );
        let mut reply = with_recommendations(1);
        for field in ["summary", "went_wrong", "notes"] {
            reply["postmortem"][field] = json!(text);
        }
        reply["postmortem"]["lessons"] = json!([text]);
        reply["postmortem"]["timeline"][0]["event"] = json!(text);
        reply["postmortem"]["recommendations"][0]["after"] = json!(text);
        reply["postmortem"]["recommendations"][0]["evidence"] = json!(text);
        let stored = result(&claimed(), &reply).unwrap();
        let mut everything = format!(
            "{} {} {} {} {}",
            stored.summary, stored.notes, stored.timeline, stored.process_failures, stored.grades
        );
        for recommendation in &stored.recommendations {
            everything.push_str(&serde_json::to_string(recommendation).unwrap());
        }
        everything.push_str(&stored.memory.unwrap().1.to_string());
        for secret in planted {
            assert!(!everything.contains(secret), "{secret} was stored");
        }
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
        // #403: the runner's later grades are code dimensions too.
        for dimension in ["tokens", "cache_rate"] {
            assert_eq!(
                rubric["dimensions"][dimension]["grader"], "code",
                "{dimension}"
            );
        }
    }
}
