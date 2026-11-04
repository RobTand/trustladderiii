//! #783 detection only. No candidate rejection, incident or request hold.
use crate::{
    Controller, Repository, Result,
    executor::{CommentKind, Delivery, Output, Reconciliation, Upstream},
    put,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tl3_kernel::{Event, NodeId, Out, World};
use tl3_store::{Commit, RowChange, Snapshot};

pub(crate) const ALARMS: &str = "kernel_alarms";

/// #783 / state-table Rules: `check` is `consistent` with a named failure;
/// `check` plus `is_settled` is exactly `valid`, without checking twice.
pub fn check_world(world: &World) -> std::result::Result<(), String> {
    world.check().map_err(|broken| format!("{broken:?}"))?;
    if !world.is_settled() {
        return Err("Unsettled".into());
    }
    // #771 HEADROOM HOOK: add headroom_ok(world) here when its API lands.
    Ok(())
}

pub(crate) fn event_node(event: Event) -> Option<NodeId> {
    match event {
        Event::NewRequest { .. } | Event::NewAdoption { .. } => None,
        Event::Reply { node, .. }
        | Event::Ack { node, .. }
        | Event::Fact { node, .. }
        | Event::Command { node, .. } => Some(node),
    }
}

#[derive(Serialize, Deserialize)]
struct Alarm {
    node: Option<NodeId>,
    rule: String,
    event: Option<Event>,
    event_key: String,
    summary: String,
    posted: bool,
    retry_after_ms: u64,
}

/// #783 / Q27: observe the real result without changing world or outputs.
pub(crate) fn observe(world: &World, event: Option<Event>, commit: &mut Commit) {
    let Err(rule) = check_world(world) else {
        return;
    };
    let node = event.and_then(|event| {
        event_node(event).or_else(|| world.nodes.len().checked_sub(1).map(|id| NodeId(id as u64)))
    });
    if !commit.result.is_object() {
        commit.result = json!({});
    }
    let alarms = commit
        .result
        .as_object_mut()
        .expect("object")
        .entry("kernel_alarms")
        .or_insert_with(|| json!([]));
    alarms
        .as_array_mut()
        .expect("alarm array")
        .push(json!({"node":node,"event":event,"rule":rule}));
}

pub(crate) fn checked_step(
    world: World,
    event: Event,
    commit: &mut Commit,
) -> Result<(World, Vec<Out>)> {
    // Serialize the borrowed input once; do not clone two entire Worlds for the audit.
    let input = serde_json::to_value(&world)?;
    let (world, outputs) = tl3_kernel::step(world, event);
    observe(&world, Some(event), commit);
    if !commit.result.is_object() {
        commit.result = json!({});
    }
    let steps = commit
        .result
        .as_object_mut()
        .expect("object")
        .entry("kernel_steps")
        .or_insert_with(|| json!([]));
    let mut record = serde_json::Map::new();
    record.insert("input".into(), input);
    record.insert("event".into(), json!(event));
    record.insert("after".into(), serde_json::to_value(&world)?);
    steps
        .as_array_mut()
        .expect("step array")
        .push(serde_json::Value::Object(record));
    Ok((world, outputs))
}

/// #783 / Q27: append alarm metadata only. Keep the original transaction's
/// rows, effects, consumed completion keys, credit and their order intact.
pub(crate) fn record_alarms(snapshot: &Snapshot, commit: &mut Commit) {
    let Some(alarms) = commit
        .result
        .as_object_mut()
        .and_then(|result| result.remove("kernel_alarms"))
    else {
        return;
    };
    for value in alarms.as_array().expect("alarm array") {
        let node: Option<NodeId> =
            serde_json::from_value(value["node"].clone()).expect("typed node");
        let event: Option<Event> =
            serde_json::from_value(value["event"].clone()).expect("typed event");
        let rule = value["rule"].as_str().expect("named invariant");
        let id = format!(
            "{}:{rule}",
            node.map_or_else(|| "all".into(), |node| node.0.to_string())
        );
        if snapshot.rows.iter().any(|row|row.table==ALARMS && row.id==id)
            || commit.rows.iter().any(|row|matches!(row,RowChange::Put{table,id:existing,..} if table==ALARMS && existing==&id)) {continue;}
        let summary = format!(
            "KERNEL INVARIANT ALARM: request {}, invariant {rule}, event {}: {event:?}. The world and effects were committed unchanged. Detection only; operator investigation required.",
            node.map_or_else(|| "unassigned".into(), |node| node.0.to_string()),
            commit.event_key
        );
        eprintln!("{summary}");
        commit.rows.push(put(
            ALARMS,
            id,
            json!(Alarm {
                node,
                rule: rule.into(),
                event,
                event_key: commit.event_key.clone(),
                summary,
                posted: false,
                retry_after_ms: 0
            }),
        ));
    }
}

impl Controller {
    /// Reuse the existing OperationsNote/ConsoleNote channel, including its
    /// marker reconciliation. A durable alarm is a separate tiny outbox; it
    /// never enters the kernel effect list or the incident/retry rules.
    pub(crate) fn post_kernel_alarm(&mut self, snapshot: &Snapshot) -> Result<bool> {
        let Some(operations) = self.bootstrap.operations.as_ref() else {
            return Ok(false);
        };
        for row in snapshot.rows.iter().filter(|row| row.table == ALARMS) {
            let mut alarm: Alarm = crate::decode_stored(row.value.clone())?;
            if alarm.posted || alarm.retry_after_ms > self.now_ms() {
                continue;
            }
            let key = format!("kernel-alarm:{}", row.id);
            let delivery = Delivery {
                key: key.clone(),
                epoch: snapshot.epoch.get(),
                output: Output::Comment {
                    node: alarm.node.unwrap_or(NodeId(0)),
                    kind: CommentKind::OperationsNote,
                },
                generation: 0,
                head_sha: None,
                repository: Repository {
                    owner: operations.owner.clone(),
                    name: operations.name.clone(),
                    installation: operations.installation_id,
                },
                issue: operations.number,
                pull_request: None,
                proposed: None,
                paths: vec![],
                attempt: 0,
                round: 0,
                replanned: false,
                checks: None,
                job_generation: None,
                question: None,
                note: Some(alarm.summary.clone()),
                scope_removals: None,
                marker_key: None,
                question_context: None,
                steering: Some(crate::steering::Task::ConsoleNote {
                    text: alarm.summary.clone(),
                }),
                request_text: None,
                evidence: None,
            };
            let adapter = self.adapters.get(Upstream::GitHub)?;
            // The comment can land before the posted bit commits. Reconcile first
            // after every restart, just as the ordinary notice publisher does.
            let result = match adapter.reconcile(&delivery) {
                Ok(Reconciliation::Found(_)) => Ok(()),
                Ok(Reconciliation::Absent) => adapter.execute(&delivery).map(|_| ()),
                Ok(Reconciliation::Pending) => {
                    Err(crate::Error::Invalid("alarm reconciliation pending"))
                }
                Err(error) => Err(error),
            };
            match result {
                Ok(()) => alarm.posted = true,
                Err(_) => {
                    eprintln!(
                        "kernel alarm {} remains on status; operations comment pending",
                        row.id
                    );
                    alarm.retry_after_ms = self.now_ms().saturating_add(30_000);
                }
            }
            let mut commit = Commit::new(
                format!("{key}:post:{}", snapshot.revision.get()),
                json!({"kernel_alarm_post":row.id}),
                snapshot,
            );
            commit.rows.push(put(ALARMS, row.id.clone(), json!(alarm)));
            self.store.commit(&commit)?;
            return Ok(true);
        }
        Ok(false)
    }
}

#[cfg(test)]
#[path = "verification_tests.rs"]
mod tests;
