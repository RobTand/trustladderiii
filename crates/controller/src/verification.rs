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
mod tests {
    use super::*;
    use serde_json::json;
    use tl3_kernel::{Priority, RepoId};
    use tl3_store::{Epoch, Revision};

    fn snapshot() -> Snapshot {
        Snapshot {
            revision: Revision::ZERO,
            epoch: Epoch::ZERO,
            rows: vec![],
            pending_effects: vec![],
            credit: vec![],
            interned: vec![],
        }
    }

    /// #783 / state-table containment: reject a seeded invalid event result without panic.
    #[test]
    fn invalid_step_raises_incident_and_discards_candidate_writes() {
        let base = snapshot();
        let mut commit = Commit::new("fixture:bad-event", json!({}), &base);
        commit
            .rows
            .push(crate::put("candidate", "0".into(), json!(true)));
        let mut world = World::new(&[1], 1);
        world.pool_capacity = 0;
        let event = Event::NewRequest {
            repo: RepoId(0),
            priority: Priority::Unranked,
        };
        let failure = checked_step(world, event, &mut commit)
            .expect_err("seeded invalid result must be rejected");
        reject(&base, Some(event), &failure.to_string(), &mut commit).unwrap();
        assert!(
            !commit.rows.iter().any(
                |r| matches!(r, tl3_store::RowChange::Put { table, .. } if table == "candidate")
            )
        );
        let incident = commit
            .rows
            .iter()
            .find_map(|r| match r {
                tl3_store::RowChange::Put { table, value, .. } if table == "incidents" => {
                    Some(value)
                }
                _ => None,
            })
            .expect("visible incident");
        assert_eq!(incident["open"], true);
        assert!(
            incident["summary"]
                .as_str()
                .unwrap()
                .contains("fixture:bad-event")
        );
        assert!(commit.rows.iter().any(
            |r| matches!(r, tl3_store::RowChange::Put {table, ..} if table == "kernel_holds")
        ));
        assert!(commit.effects.is_empty());
    }

    /// #783 / Q50-Q51: reject before effects, persist the hold across restart,
    /// allow an unrelated request to progress, and release on a new TL3 release.
    #[test]
    fn controller_rejects_bad_step_and_holds_only_its_request_after_restart() {
        use crate::{
            Bootstrap, Controller, LabelledIssue, Repository, adapters,
            git_tests::support::Database,
        };
        use tl3_kernel::{Fact, NodeId};
        use tl3_store::Store;
        let mut db = Database::start();
        let settings = db.create("runtime_invariant_hold");
        let root = tempfile::tempdir().unwrap();
        let bootstrap = Bootstrap {
            operations: None,
            repos: vec![Repository {
                owner: "owner".into(),
                name: "sandbox".into(),
                installation: 1,
            }],
            rob: 42,
            slots: 1,
            pool: 2,
            fail_fast: false,
        };
        let open = || {
            Controller::open(
                Store::open(&settings).unwrap(),
                adapters::simulation(root.path()).unwrap(),
                bootstrap.clone(),
            )
            .unwrap()
        };
        let mut c = open();
        for number in [42, 43] {
            c.intake(LabelledIssue {
                delivery: format!("fixture:{number}"),
                owner: "owner".into(),
                repo: "sandbox".into(),
                installation: 1,
                number,
                title: "Fixture".into(),
                actor: 42,
                label: "tl3".into(),
                labels: vec![],
            })
            .unwrap();
        }
        let baseline = c.world().unwrap();
        let snapshot = c.snapshot().unwrap();
        let mut bad = baseline.clone();
        bad.pool_capacity = 0;
        let event = Event::Fact {
            node: NodeId(0),
            fact: Fact::Priority(Priority::P0),
        };
        let mut commit = Commit::new("fixture:violating-event", json!({}), &snapshot);
        c.apply(&snapshot, bad, event, &mut commit).unwrap();
        assert!(commit.effects.is_empty());
        c.store.commit(&commit).unwrap();
        assert_eq!(c.world().unwrap(), baseline);
        let snapshot = c.snapshot().unwrap();
        assert!(held(&snapshot, NodeId(0)));
        assert!(!held(&snapshot, NodeId(1)));
        let incident = snapshot
            .rows
            .iter()
            .find(|row| row.table == "incidents")
            .unwrap();
        assert!(
            incident.value["summary"]
                .as_str()
                .unwrap()
                .contains("fixture:violating-event")
        );
        let error = crate::replay::run(&mut c.store.reader().unwrap(), None)
            .unwrap_err()
            .to_string();
        assert!(error.contains("fixture:violating-event"), "{error}");
        drop(c);
        let mut c = open();
        let snapshot = c.snapshot().unwrap();
        let mut commit = Commit::new("fixture:held-event", json!({}), &snapshot);
        c.apply(&snapshot, baseline.clone(), event, &mut commit)
            .unwrap();
        c.store.commit(&commit).unwrap();
        assert_eq!(c.world().unwrap(), baseline);
        // The real dispatch loop must leave the held request's effects Intended.
        for _ in 0..64 {
            if !c.drive_once().unwrap() {
                break;
            }
        }
        let snapshot = c.snapshot().unwrap();
        assert_eq!(c.world().unwrap().nodes[0], baseline.nodes[0]);
        assert!(
            snapshot
                .pending_effects
                .iter()
                .filter(|effect| effect.key.starts_with("effect:0:"))
                .all(|effect| effect.state == tl3_store::EffectState::Intended)
        );
        assert_ne!(c.world().unwrap().nodes[1], baseline.nodes[1]);
        c.start_release("fixture-fixed-release").unwrap();
        assert!(!held(&c.snapshot().unwrap(), NodeId(0)));
    }
}
