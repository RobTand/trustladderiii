pub mod decision;
use crate::{
    Controller, Error, Result, decode_stored,
    executor::{Delivery, Output},
    hydrate,
    input_errors::IssueTarget,
    put, request, snapshot,
};
pub use decision::{DispatchStatus, Evidence, ProcessEvidence, classify};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use tl3_kernel::{EffectKind, Node, NodeId, Phase, Role, World};
use tl3_store::{Commit, EffectState, RowChange, Snapshot};

pub(crate) const MISSING: &str = "worker_dispatch_missing";
pub(crate) const QUEUED: &str = "worker_wait_recovery";

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Pool {
    Agents,
    Rebases,
    Checks,
}

fn operation(node: &Node) -> Option<(Pool, EffectKind)> {
    Some(match node.phase {
        Phase::Ingesting => (Pool::Agents, EffectKind::StartRun(Role::Ingest)),
        Phase::Auditing => (Pool::Agents, EffectKind::StartRun(Role::Audit)),
        Phase::Planning { .. } => (Pool::Agents, EffectKind::StartRun(Role::Plan)),
        Phase::Making => (Pool::Agents, EffectKind::StartRun(Role::Make)),
        Phase::Reviewing => (Pool::Agents, EffectKind::StartRun(Role::Review)),
        Phase::Rebasing => (Pool::Rebases, EffectKind::StartRebase),
        Phase::Checking => (Pool::Checks, EffectKind::SubmitChecks),
        _ => return None,
    })
}

fn acknowledged_wait(node: &Node) -> bool {
    // run/job is assigned before dispatch acknowledgement. An in-flight start
    // must settle through its original effect, not through result-watch repair.
    node.effect.is_none()
        && !node.waiting_incident
        && operation(node).is_some_and(|(_, kind)| node.operation_pending(kind, node.generation))
}

fn key(id: NodeId, node: &Node) -> Option<String> {
    node.next_seq
        .checked_sub(1)
        .map(|seq| format!("effect:{}:{seq}", id.0))
}

struct Coverage<'a> {
    watches: BTreeMap<&'a str, &'a Value>,
    allowances: BTreeSet<&'a str>,
}

impl<'a> Coverage<'a> {
    fn new(snapshot: &'a Snapshot, changes: &'a [RowChange]) -> Self {
        let mut coverage = Self {
            watches: snapshot
                .rows
                .iter()
                .filter(|r| r.table == "watches")
                .map(|r| (r.id.as_str(), &r.value))
                .collect(),
            allowances: snapshot
                .rows
                .iter()
                .filter(|r| r.table == "allowances")
                .map(|r| r.id.as_str())
                .collect(),
        };
        for change in changes {
            match change {
                RowChange::Put { table, id, value } if table == "watches" => {
                    coverage.watches.insert(id, value);
                }
                RowChange::Put { table, id, .. } if table == "allowances" => {
                    coverage.allowances.insert(id);
                }
                RowChange::Delete { table, id } if table == "watches" => {
                    coverage.watches.remove(id.as_str());
                }
                RowChange::Delete { table, id } if table == "allowances" => {
                    coverage.allowances.remove(id.as_str());
                }
                _ => {}
            }
        }
        coverage
    }

    fn tracked(&self, id: NodeId, node: &Node, epoch: i64) -> Result<bool> {
        let Some(key) = key(id, node) else {
            return Ok(false);
        };
        self.watches
            .get(key.as_str())
            .map(|value| decode_stored::<Delivery>((*value).clone()))
            .transpose()
            .map(|watch| watch.is_some_and(|d| matches_dispatch(&d, id, node, &key, epoch)))
    }

    fn covered(&self, id: NodeId, node: &Node, epoch: i64) -> Result<bool> {
        Ok(self.tracked(id, node, epoch)?
            && key(id, node).is_some_and(|key| self.allowances.contains(key.as_str())))
    }
}

fn matches_dispatch(d: &Delivery, id: NodeId, node: &Node, key: &str, epoch: i64) -> bool {
    d.generation == node.generation
        && d.epoch <= epoch
        && d.key == key
        && matches!(d.output, Output::Effect { node: owner, seq, kind, .. }
            if owner == id && Some(seq) == node.next_seq.checked_sub(1)
                && operation(node).is_some_and(|(_, expected)| kind == expected))
}

fn blocked(snapshot: &Snapshot, world: &World) -> bool {
    // No conclusion about a missing dispatch is valid ahead of retained inputs
    // or Q27/#771 headroom recovery. Their transaction may not have started it.
    crate::input_errors::kernel_backlog(snapshot)
        || snapshot.rows.iter().any(|r| {
            r.table == "incidents"
                && r.value["open"] == true
                && r.value["signature"]["class"] == crate::input_errors::COUNTER_HEADROOM
        })
        || world.counter_exhaustion().is_some()
}

pub(crate) fn queue_pool_changes(
    snapshot: &Snapshot,
    previous: &World,
    world: &World,
    commit: &mut Commit,
) -> Result<()> {
    let mut pools = BTreeSet::new();
    if previous.pool_capacity != world.pool_capacity {
        pools.insert(Pool::Agents);
    }
    let changed: BTreeSet<_> = commit
        .rows
        .iter()
        .filter_map(|row| match row {
            RowChange::Put { table, id, .. } if table == "nodes" => Some(
                id.parse::<usize>()
                    .map_err(|_| Error::Invalid("invalid changed node")),
            ),
            _ => None,
        })
        .collect::<Result<_>>()?;
    for index in changed {
        let node = world
            .nodes
            .get(index)
            .ok_or(Error::Invalid("missing changed node"))?;
        let old = previous.nodes.get(index);
        if old.is_some_and(|old| {
            (
                old.pool,
                old.ticket,
                old.run,
                old.job,
                old.generation,
                old.effect,
                old.paused,
            ) == (
                node.pool,
                node.ticket,
                node.run,
                node.job,
                node.generation,
                node.effect,
                node.paused,
            )
        }) {
            continue;
        }
        for node in old.into_iter().chain(std::iter::once(node)) {
            if let Some((pool, _)) = operation(node) {
                pools.insert(pool);
            }
        }
    }
    if pools.is_empty() {
        return Ok(());
    }
    let coverage = Coverage::new(snapshot, &commit.rows);
    let mut pending = Vec::new();
    for (index, node) in world.nodes.iter().enumerate() {
        let id = NodeId(index as u64);
        if acknowledged_wait(node)
            && operation(node).is_some_and(|(pool, _)| pools.contains(&pool))
            && !coverage.covered(id, node, snapshot.epoch.get())?
        {
            pending.push(put(QUEUED, id.0.to_string(), json!(true)));
        }
    }
    // This queue commits with the pool change. A refused or rolled-back kernel
    // input cannot schedule repair of its decided-but-unwritten effects.
    commit.rows.extend(pending);
    Ok(())
}

impl Controller {
    pub(crate) fn reconcile_worker_waits(&mut self) -> Result<()> {
        let snapshot = self.store.load()?;
        let world = snapshot::decode(&snapshot)?;
        let coverage = Coverage::new(&snapshot, &[]);
        let mut candidates = Vec::new();
        for (index, node) in world.nodes.iter().enumerate() {
            let id = NodeId(index as u64);
            if acknowledged_wait(node) && !coverage.covered(id, node, snapshot.epoch.get())? {
                candidates.push(id);
            }
        }
        if blocked(&snapshot, &world) {
            let mut commit = Commit::new(
                format!("worker-wait-deferred:{}", snapshot.revision.get()),
                json!({"worker_wait_deferred": true}),
                &snapshot,
            );
            for id in candidates {
                if !snapshot
                    .rows
                    .iter()
                    .any(|r| r.table == QUEUED && r.id == id.0.to_string())
                {
                    commit.rows.push(put(QUEUED, id.0.to_string(), json!(true)));
                }
            }
            if !commit.rows.is_empty() {
                self.store.commit(&commit)?;
            }
        } else {
            self.reconcile_worker_waits_in(&snapshot, &world, &candidates, &mut |_, _| {})?;
        }
        Ok(())
    }

    pub(crate) fn reconcile_queued_worker_waits(
        &mut self,
        snapshot: &Snapshot,
        world: &World,
        hook: &mut impl FnMut(crate::Boundary, &str),
    ) -> Result<bool> {
        let candidates = snapshot
            .rows
            .iter()
            .filter(|r| r.table == QUEUED)
            .map(|r| {
                r.id.parse::<u64>()
                    .map(NodeId)
                    .map_err(|_| Error::Invalid("invalid worker wait node"))
            })
            .collect::<Result<Vec<_>>>()?;
        if !candidates.is_empty() {
            hook(crate::Boundary::WorkerWaitHeadroom, "headroom");
        }
        if candidates.is_empty() || blocked(snapshot, world) {
            return Ok(false);
        }
        self.reconcile_worker_waits_in(snapshot, world, &candidates, hook)
    }

    fn reconcile_worker_waits_in(
        &mut self,
        snapshot: &Snapshot,
        world: &World,
        candidates: &[NodeId],
        hook: &mut impl FnMut(crate::Boundary, &str),
    ) -> Result<bool> {
        let coverage = Coverage::new(snapshot, &[]);
        let queued: BTreeSet<_> = snapshot
            .rows
            .iter()
            .filter(|r| r.table == QUEUED)
            .map(|r| r.id.as_str())
            .collect();
        let missing: BTreeSet<_> = snapshot
            .rows
            .iter()
            .filter(|r| {
                r.table == "incidents"
                    && r.value["open"] == true
                    && r.value["signature"]["class"] == MISSING
            })
            .filter_map(|r| r.value["node"].as_u64())
            .collect();
        let mut commit = Commit::new(
            format!("worker-wait-recovery:{}", snapshot.revision.get()),
            json!({"worker_wait_recovery": true}),
            snapshot,
        );
        for &id in candidates {
            hook(crate::Boundary::WorkerWait, &id.0.to_string());
            if queued.contains(id.0.to_string().as_str()) {
                commit.rows.push(RowChange::Delete {
                    table: QUEUED.into(),
                    id: id.0.to_string(),
                });
            }
            let node = world
                .nodes
                .get(id.0 as usize)
                .ok_or(Error::Invalid("missing worker wait node"))?;
            if !acknowledged_wait(node) || coverage.covered(id, node, snapshot.epoch.get())? {
                continue;
            }
            let key = key(id, node);
            let effect = key
                .as_deref()
                .map(|key| self.store.effect(key))
                .transpose()?
                .flatten();
            let delivery = effect
                .as_ref()
                .map(|effect| hydrate(effect, snapshot))
                .transpose()?;
            // Reconciliation keeps the original epoch after ownership advances.
            let matching = effect
                .as_ref()
                .zip(delivery.as_ref())
                .filter(|(effect, delivery)| {
                    matches_dispatch(delivery, id, node, &effect.key, snapshot.epoch.get())
                        && (effect.state.is_pending() || effect.state == EffectState::Confirmed)
                });
            if let Some((effect, delivery)) = matching {
                if !coverage.tracked(id, node, snapshot.epoch.get())? {
                    commit
                        .rows
                        .push(put("watches", effect.key.clone(), json!(delivery)));
                }
                if !coverage.allowances.contains(effect.key.as_str()) {
                    commit.rows.push(self.allowance_row(delivery.clone()));
                }
                continue;
            }
            if missing.contains(&id.0) {
                continue;
            }
            let request = request(snapshot, id)?;
            self.input_failed(
                snapshot,
                &format!("worker-wait:{}:{}", id.0, node.generation),
                Some(IssueTarget {
                    repository: request.repo,
                    issue: request.issue,
                }),
                &request.title,
                Some(id),
                crate::executor::upstream(Output::Effect {
                    node: id,
                    seq: 0,
                    kind: operation(node).expect("acknowledged operation").1,
                    head: None,
                }),
                Error::WorkerDispatchMissing,
                &mut commit,
            )?;
        }
        if commit.rows.is_empty() {
            return Ok(false);
        }
        self.store.commit(&commit)?;
        Ok(true)
    }
}

impl Controller {
    pub fn dispatch_status(&mut self, _id: NodeId) -> Result<DispatchStatus> {
        Ok(DispatchStatus::Silent)
    }
}
