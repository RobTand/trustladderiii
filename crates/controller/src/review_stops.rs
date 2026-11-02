//! Q54 (#440, Rob, 2026-11-02): under `limits.fail_fast`, the kernel stops a
//! request for TL3 review instead of retrying the work. At the stop the
//! controller records the failing step with its evidence, for the closing
//! summary, and posts one code-written line to the operations issue for the
//! coordinator. A postmortem for the stop is owed as `needed_fix` (#415 reads
//! `Node::review_stop`).

use crate::executor::{CommentKind, Delivery, Output};
use crate::question_findings::{self, Sources};
use crate::{Controller, Repository, Result, put};
use serde_json::{Value, json};
use tl3_kernel::{NodeId, ReviewStop, World};
use tl3_store::{Commit, RowChange, Snapshot};

const TABLE: &str = "review_stops";

/// The step's name in the operations note and the stored record.
pub(crate) fn step_name(step: ReviewStop) -> &'static str {
    match step {
        ReviewStop::ChecksFailed => "checks_failed",
        ReviewStop::ChangesRequested => "changes_requested",
        ReviewStop::CiFailed => "ci_failed",
        ReviewStop::MakerCantMeet => "maker_cant_meet",
        ReviewStop::MakerNeedsFiles => "maker_needs_files",
        ReviewStop::MakerOutOfRoom => "maker_out_of_room",
        ReviewStop::ReviewerOutOfRoom => "reviewer_out_of_room",
        ReviewStop::PlanCantMeet => "plan_cant_meet",
        ReviewStop::PlanBrokeRule => "plan_broke_rule",
    }
}

/// The failing step in plain words, for the summary's writer.
fn step_text(step: ReviewStop) -> &'static str {
    match step {
        ReviewStop::ChecksFailed => "the checks failed again after one fix",
        ReviewStop::ChangesRequested => "the review asked for changes again after one fix",
        ReviewStop::CiFailed => "the required tests failed again after one fix",
        ReviewStop::MakerCantMeet => "the change could not meet one of the goals",
        ReviewStop::MakerNeedsFiles => "the change needed more files than its plan allows",
        ReviewStop::MakerOutOfRoom => "the change was too large to make in one go",
        ReviewStop::ReviewerOutOfRoom => "the change was too large to review in one go",
        ReviewStop::PlanCantMeet => "planning found no way to meet the request",
        ReviewStop::PlanBrokeRule => "planning broke one of its rules twice",
    }
}

/// The summary's outcome for a request stopped for TL3 review.
pub(crate) fn outcome(context: &Value) -> Option<String> {
    let step = context["review_stop"]["text"].as_str()?;
    Some(format!(
        "Work on this request stopped so that TL3's maintainers can review a failure before anything is retried: {step}."
    ))
}

/// The evidence behind the stop, for the summary's writer. The writer
/// summarises it and the worker redacts it first, as for a question (#322).
pub(crate) fn findings(context: &Value) -> Option<Value> {
    context["review_stop"]
        .get("findings")
        .filter(|f| f.is_object())
        .cloned()
}

/// The stored record of a node's stop, read from this commit or the store.
pub(crate) fn context(snapshot: &Snapshot, commit: &Commit, node: NodeId) -> Option<Value> {
    let id = node.0.to_string();
    commit
        .rows
        .iter()
        .rev()
        .find_map(|row| match row {
            RowChange::Put {
                table,
                id: row_id,
                value,
            } if table == TABLE && *row_id == id => Some(value.clone()),
            _ => None,
        })
        .or_else(|| {
            snapshot
                .rows
                .iter()
                .find(|r| r.table == TABLE && r.id == id)
                .map(|r| r.value.clone())
        })
        .map(|record| json!({"review_stop": record}))
}

impl Controller {
    /// Records each stop this step made and queues its operations note.
    pub(crate) fn record_review_stops(
        &mut self,
        snapshot: &Snapshot,
        previous: &World,
        world: &World,
        commit: &mut Commit,
    ) -> Result<()> {
        for (index, n) in world.nodes.iter().enumerate() {
            let Some(step) = n.review_stop else {
                continue;
            };
            if previous
                .nodes
                .get(index)
                .is_some_and(|old| old.review_stop.is_some())
            {
                continue;
            }
            let node = NodeId(index as u64);
            let id = index.to_string();
            let written = |table: &str| {
                commit.rows.iter().rev().find_map(|row| match row {
                    RowChange::Put {
                        table: t,
                        id: row_id,
                        value,
                    } if t == table && *row_id == id => Some(value.clone()),
                    _ => None,
                })
            };
            let before = |table: &str| {
                snapshot
                    .rows
                    .iter()
                    .find(|r| r.table == table && r.id == id)
                    .map(|r| r.value.clone())
            };
            let prior = previous
                .nodes
                .get(index)
                .map_or(tl3_kernel::Phase::Ingesting, |old| old.phase);
            let found = question_findings::review_stop(
                step,
                &Sources {
                    fresh: &written,
                    previous: &before,
                    incidents: &[],
                    prior,
                },
            );
            let mut record = json!({"step": step_name(step), "text": step_text(step),
                "attempt": n.attempt, "round": n.round});
            if let Some(found) = found {
                record["findings"] = found;
            }
            commit.rows.push(put(TABLE, id.clone(), record));
            let request = crate::request(snapshot, node)?;
            let line = format!(
                "Stopped for TL3 review (tl3_review): {}/{}#{} at step {}, attempt {}, round {}. Restart it once the fix ships.",
                request.repo.owner,
                request.repo.name,
                request.issue,
                step_name(step),
                n.attempt,
                n.round
            );
            let (repository, issue) = match &self.bootstrap.operations {
                Some(operations) => (
                    Repository {
                        owner: operations.owner.clone(),
                        name: operations.name.clone(),
                        installation: operations.installation_id,
                    },
                    operations.number,
                ),
                None => (request.repo.clone(), request.issue),
            };
            let delivery = Delivery {
                key: format!("review-stop:{index}:{}", n.generation),
                epoch: snapshot.epoch.get(),
                output: Output::Comment {
                    node,
                    kind: CommentKind::OperationsNote,
                },
                generation: n.generation,
                head_sha: None,
                repository,
                issue,
                pull_request: None,
                proposed: None,
                paths: vec![],
                attempt: n.attempt,
                round: n.round,
                replanned: n.replanned,
                checks: None,
                job_generation: None,
                question: None,
                note: Some(line.clone()),
                scope_removals: None,
                marker_key: None,
                question_context: None,
                steering: Some(crate::steering::Task::ConsoleNote { text: line }),
                request_text: None,
            };
            commit.rows.push(self.allowance_row(delivery.clone()));
            commit.effects.push(delivery.into_effect());
        }
        Ok(())
    }
}
