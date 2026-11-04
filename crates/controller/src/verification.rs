//! Runtime checks for #783, covering the invariants in docs/state-table.md.
use crate::{Error, Result, input_errors, looks, put};
use serde_json::json;
use tl3_kernel::{Event, NodeId, Out, World};
use tl3_store::{Commit, RowChange, Snapshot};

pub(crate) const CLASS: &str = "kernel_invariant";

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

pub(crate) fn held(snapshot: &Snapshot, node: NodeId) -> bool {
    snapshot
        .rows
        .iter()
        .any(|row| row.table == "kernel_holds" && (row.id == "all" || row.id == node.0.to_string()))
}

pub(crate) fn prepare(snapshot: &Snapshot, commit: &mut Commit) {
    if !commit.result.is_object() {
        commit.result = json!({});
    }
    commit.result["kernel_holds"] = json!(
        snapshot
            .rows
            .iter()
            .filter(|row| row.table == "kernel_holds")
            .map(|row| row.id.as_str())
            .collect::<Vec<_>>()
    );
}

pub(crate) fn checked_step(
    world: World,
    event: Event,
    commit: &mut Commit,
) -> Result<(World, Vec<Out>)> {
    let holds = commit.result["kernel_holds"].as_array();
    if holds.is_some_and(|holds| {
        holds.iter().any(|id| {
            id == "all" || event_node(event).is_some_and(|node| id == &node.0.to_string())
        })
    }) {
        return Ok((world, vec![]));
    }
    let input = world.clone();
    let (world, outputs) = tl3_kernel::step(world, event);
    check_world(&world).map_err(|rule| Error::KernelInvariant {
        event: Some(event),
        rule,
    })?;
    if !commit.result.is_object() {
        commit.result = json!({});
    }
    let steps = commit
        .result
        .as_object_mut()
        .expect("object")
        .entry("kernel_steps")
        .or_insert_with(|| json!([]));
    steps
        .as_array_mut()
        .expect("step array")
        .push(json!(crate::replay::StepRecord {
            input,
            event,
            after: world.clone(),
        }));
    Ok((world, outputs))
}

/// #783 / Q50: reject the whole candidate before persistence or dispatch.
/// The separate hold leaves the last valid kernel snapshot usable after restart.
pub(crate) fn reject(
    snapshot: &Snapshot,
    event: Option<Event>,
    rule: &str,
    commit: &mut Commit,
) -> Result<()> {
    let node = event.and_then(event_node);
    let key = commit.event_key.clone();
    *commit = Commit::new(key.clone(), commit.event.clone(), snapshot);
    let id = format!("{}.kernel", snapshot.revision.get() + 1);
    let summary = format!(
        "Kernel invariant {rule} failed after event {key}: {event:?}. The candidate was rejected. Further events for this request are held until the incident clears."
    );
    let request = node
        .filter(|node| {
            snapshot
                .rows
                .iter()
                .any(|row| row.table == "requests" && row.id == node.0.to_string())
        })
        .map(|node| crate::request(snapshot, node))
        .transpose()?;
    let target = request.as_ref().map(|r| input_errors::IssueTarget {
        repository: r.repo.clone(),
        issue: r.issue,
    });
    let incident = looks::Incident {
        id: id.clone(),
        node,
        open: true,
        needs_rob: true,
        blocking: false,
        summary,
        signature: looks::IncidentSignature {
            upstream: crate::executor::Upstream::Workers,
            class: CLASS.into(),
        },
        effect: None,
        post_key: Some(format!("incident:{id}")),
        cause: Some(looks::Cause::Tl3),
        input_failure: Some(input_errors::InputFailure::runtime(target, &key)),
    };
    eprintln!("{}", incident.summary);
    commit
        .rows
        .push(put("incidents", id.clone(), json!(incident)));
    commit.rows.push(put(
        "kernel_holds",
        node.map_or_else(|| "all".into(), |node| node.0.to_string()),
        json!({"incident":id,"event":key}),
    ));
    commit.result = json!({"kernel_violation":{"event":event,"rule":rule}});
    Ok(())
}

pub(crate) fn release(snapshot: &Snapshot, id: &str, commit: &mut Commit) {
    for row in snapshot
        .rows
        .iter()
        .filter(|row| row.table == "kernel_holds" && row.value["incident"] == id)
    {
        commit.rows.push(RowChange::Delete {
            table: row.table.clone(),
            id: row.id.clone(),
        });
    }
}

#[cfg(test)]
#[path = "verification_tests.rs"]
mod tests;
