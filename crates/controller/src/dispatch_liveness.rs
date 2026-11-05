pub mod decision;
pub mod model;
mod recovery;
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
pub(crate) const PARKED: &str = "worker_wait_exhaustion";
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

enum Deferral {
    Inputs,
    Incident,
    Counter(tl3_kernel::CounterExhaustion),
}

fn blocked(snapshot: &Snapshot, world: &World, scan: &mut impl FnMut()) -> Option<Deferral> {
    // Preserve the original short-circuit order, including the sampling point
    // of the global counter scan. Retained inputs may not have started a run.
    if crate::input_errors::kernel_backlog(snapshot) {
        return Some(Deferral::Inputs);
    }
    if snapshot.rows.iter().any(|r| {
        r.table == "incidents"
            && r.value["open"] == true
            && r.value["signature"]["class"] == crate::input_errors::COUNTER_HEADROOM
    }) {
        return Some(Deferral::Incident);
    }
    scan();
    world.counter_exhaustion().map(Deferral::Counter)
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
            && !quota_held(snapshot, &commit.rows, id, node)?
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
            if acknowledged_wait(node)
                && !quota_held(&snapshot, &[], id, node)?
                && !coverage.covered(id, node, snapshot.epoch.get())?
            {
                candidates.push(id);
            }
        }
        if let Some(reason) = blocked(&snapshot, &world, &mut || {}) {
            let mut commit = Commit::new(
                format!("worker-wait-deferred:{}", snapshot.revision.get()),
                json!({"worker_wait_deferred": true}),
                &snapshot,
            );
            if !matches!(reason, Deferral::Inputs) {
                // The existing #771 halt covers a parked wait even when no
                // kernel input has arrived yet. Clearance releases the queue.
                if let Deferral::Counter(exhausted) = reason {
                    self.counter_exhausted(
                        &snapshot,
                        &mut commit,
                        exhausted.node,
                        format!("kernel counter headroom exhausted: {}", exhausted.counter),
                    )?;
                }
                commit
                    .rows
                    .push(put(PARKED, "headroom".into(), json!(true)));
            }
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
            if snapshot.rows.iter().any(|r| r.table == PARKED) {
                let mut commit = Commit::new(
                    format!("worker-wait-unpark:{}", snapshot.revision.get()),
                    json!({"worker_wait_unpark": true}),
                    &snapshot,
                );
                commit.rows.push(RowChange::Delete {
                    table: PARKED.into(),
                    id: "headroom".into(),
                });
                self.store.commit(&commit)?;
                return self.reconcile_worker_waits();
            }
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
        // #816 P2: only startup or committed #771 clearance releases this row.
        // Unrelated drives, allowance wakes and row writes do not scan headroom.
        if !decision::scan_queued(snapshot.rows.iter().any(|r| r.table == PARKED)) {
            return Ok(false);
        }
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
        let mut settled = Commit::new(
            format!("worker-wait-quota:{}", snapshot.revision.get()),
            json!({"worker_wait_quota": true}),
            snapshot,
        );
        for id in &candidates {
            if let Some(node) = world.nodes.get(id.0 as usize)
                && quota_held(snapshot, &[], *id, node)?
            {
                settled.rows.push(RowChange::Delete {
                    table: QUEUED.into(),
                    id: id.0.to_string(),
                });
            }
        }
        if !settled.rows.is_empty() {
            self.store.commit(&settled)?;
            return Ok(true);
        }
        if candidates.is_empty() {
            return Ok(false);
        }
        if let Some(reason) = blocked(snapshot, world, &mut || {
            hook(crate::Boundary::WorkerWaitHeadroom, "headroom")
        }) {
            if !matches!(reason, Deferral::Inputs) {
                let mut commit = Commit::new(
                    format!("worker-wait-park:{}", snapshot.revision.get()),
                    json!({"worker_wait_park": true}),
                    snapshot,
                );
                if let Deferral::Counter(exhausted) = reason {
                    self.counter_exhausted(
                        snapshot,
                        &mut commit,
                        exhausted.node,
                        format!("kernel counter headroom exhausted: {}", exhausted.counter),
                    )?;
                }
                commit
                    .rows
                    .push(put(PARKED, "headroom".into(), json!(true)));
                self.store.commit(&commit)?;
                return Ok(true);
            }
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
        recovery::Recovery {
            store: recovery::ControllerIo(self),
        }
        .reconcile_worker_waits_in(snapshot, world, candidates, hook)
    }
}

impl Controller {
    /// #817: diagnostic classification uses the same snapshot and the current
    /// operation's immutable key. It never launches, acknowledges or replies.
    pub fn dispatch_status(&mut self, id: NodeId) -> Result<DispatchStatus> {
        let snapshot = self.store.load()?;
        let world = snapshot::decode(&snapshot)?;
        let mut evidence = self.wait_evidence(&snapshot, &world, id)?;
        let node = world
            .nodes
            .get(id.0 as usize)
            .ok_or(Error::Invalid("missing dispatch node"))?;
        if node.waiting_on() == tl3_kernel::Wait::AgentPool {
            let occupants: Vec<_> = world
                .nodes
                .iter()
                .enumerate()
                .filter(|(_, n)| n.pool)
                .map(|(i, _)| NodeId(i as u64))
                .collect();
            if occupants.len() >= usize::from(world.pool_capacity) {
                evidence.obstruction = true;
                for occupant in occupants {
                    let e = self.wait_evidence(&snapshot, &world, occupant)?;
                    let status = classify(world.nodes[occupant.0 as usize].waiting_on(), e);
                    if !matches!(
                        status,
                        DispatchStatus::Dispatched(_)
                            | DispatchStatus::Ending
                            | DispatchStatus::Retry(_)
                            | DispatchStatus::Incident
                    ) {
                        evidence.obstruction = false;
                    }
                }
            } else {
                evidence.obstruction = world.nodes.iter().any(|other| {
                    !matches!(other.phase, Phase::Ready(_))
                        && other.ticket.zip(node.ticket).is_some_and(|(theirs, mine)| {
                            other.priority.rank() < node.priority.rank()
                                || (other.priority == node.priority && theirs < mine)
                        })
                });
            }
        }
        Ok(classify(node.waiting_on(), evidence))
    }

    fn wait_evidence(
        &mut self,
        snapshot: &Snapshot,
        world: &World,
        id: NodeId,
    ) -> Result<Evidence> {
        use decision::{DeliveryClass, delivery_class};
        use tl3_kernel::Wait;
        let node = world
            .nodes
            .get(id.0 as usize)
            .ok_or(Error::Invalid("missing dispatch node"))?;
        let mut e = Evidence {
            class: DeliveryClass::Worker,
            intent: false,
            contract: crate::dispatch_liveness::decision::ContractEvidence::Absent,
            watch: false,
            process: ProcessEvidence::Unknown,
            ending: false,
            deadline: false,
            quota_reset: None,
            incident: snapshot.rows.iter().any(|r| {
                r.table == "incidents"
                    && r.value["open"] == true
                    && (r.value["node"] == json!(id)
                        || r.value["signature"]["class"] == crate::input_errors::COUNTER_HEADROOM)
            }),
            paused: node.paused,
            ticket: node.ticket.is_some(),
            obstruction: false,
            question: node.question.is_some(),
            external: false,
            closed: node.phase.is_closed(),
            admission: false,
        };
        let current = node.effect.map(|fx| (fx.seq, fx.kind)).or_else(|| {
            operation(node).and_then(|(_, kind)| {
                node.operation_pending(kind, node.generation)
                    .then(|| node.next_seq.checked_sub(1).map(|seq| (seq, kind)))
                    .flatten()
            })
        });
        if let Some((seq, kind)) = current {
            e.class = delivery_class(kind);
            let key = format!("effect:{}:{seq}", id.0);
            let matches = |d: &Delivery| {
                d.key == key
                    && d.generation == node.generation
                    && d.epoch <= snapshot.epoch.get()
                    && matches!(d.output, Output::Effect { node: owner, seq: s, kind: k, .. }
                    if owner == id && s == seq && k == kind)
            };
            if let Some(effect) = self.store.effect(&key)? {
                let d = hydrate(&effect, snapshot)?;
                e.intent = matches(&d)
                    && (effect.state.is_pending() || effect.state == EffectState::Confirmed);
                if e.intent && e.class == DeliveryClass::Worker {
                    (e.process, e.ending) = self
                        .adapters
                        .get(crate::executor::upstream(d.output))?
                        .dispatch_evidence(&d)?;
                } else if e.intent {
                    e.contract = self.adapters.adapter(&d)?.contract_evidence(&d)?;
                    e.ending = e.contract == decision::ContractEvidence::Ending;
                }
            }
            for row in &snapshot.rows {
                if row.id != key {
                    continue;
                }
                if row.table == "watches" {
                    e.watch = matches(&decode_stored::<Delivery>(row.value.clone())?);
                }
                if row.table == "allowances" {
                    let d: Delivery = decode_stored(row.value["delivery"].clone())?;
                    e.deadline = matches(&d) && row.value["due_ms"].as_u64().is_some();
                    if matches(&d) {
                        e.quota_reset = row.value["quota_ms"].as_u64();
                    }
                }
            }
            for row in snapshot.rows.iter().filter(|r| r.table == "inbox") {
                if let Ok(observation) =
                    serde_json::from_value::<crate::executor::Observation>(row.value.clone())
                {
                    use crate::executor::Observation;
                    e.ending |= match observation {
                        Observation::WorkerFailed {
                            node: owner,
                            generation,
                            ..
                        }
                        | Observation::WorkerReply {
                            node: owner,
                            generation,
                            ..
                        }
                        | Observation::Kernel(tl3_kernel::Event::Reply {
                            node: owner,
                            generation,
                            ..
                        }) => owner == id && generation == node.generation,
                        _ => false,
                    };
                }
            }
        }
        match node.waiting_on() {
            Wait::Incident
            | Wait::Answer
            | Wait::Resume
            | Wait::Effect(_)
            | Wait::WorkerReply
            | Wait::CheckResults
            | Wait::AgentPool
            | Wait::None => {}
            Wait::MergeQueue => {
                e.external = snapshot.rows.iter().any(|r| {
                    r.table == "pull_requests"
                        && r.id == id.0.to_string()
                        && r.value["number"].as_u64().is_some()
                });
            }
            Wait::SlotOrFiles => {
                let holders = world
                    .nodes
                    .iter()
                    .filter(|n| n.holds && n.repo == node.repo);
                let count = holders.clone().count();
                e.obstruction = world
                    .gates
                    .get(node.repo.0 as usize)
                    .is_some_and(|gate| count >= usize::from(gate.w))
                    || holders
                        .clone()
                        .any(|n| !disjoint(world, n.footprint, node.footprint))
                    || world.nodes.iter().any(|n| {
                        n.repo == node.repo
                            && matches!(n.phase, Phase::Ready(_))
                            && n.ticket.zip(node.ticket).is_some_and(|(a, b)| a < b)
                            && !disjoint(world, n.footprint, node.footprint)
                    });
            }
            Wait::UpstreamEvent => {
                e.external = !world.dependencies_delivered(id);
                e.admission = node.phase == Phase::Unplanned
                    && world.gates.get(node.repo.0 as usize).is_some_and(|gate| {
                        world
                            .nodes
                            .iter()
                            .filter(|n| {
                                n.repo == node.repo
                                    && matches!(n.phase, Phase::Ready(_))
                                    && n.ticket.is_some()
                            })
                            .count()
                            >= usize::from(gate.w)
                    });
            }
        }
        Ok(e)
    }
}

fn disjoint(world: &World, a: tl3_kernel::Footprint, b: tl3_kernel::Footprint) -> bool {
    world.files(a).is_some_and(|mut a| {
        a.all(|file| {
            world
                .files(b)
                .is_some_and(|mut b| b.all(|other| file != other))
        })
    })
}

/// Q53 / #828: a current quota retry is already covered by its durable reset,
/// including when its completed worker no longer has a result watch.
pub(crate) fn quota_held(
    snapshot: &Snapshot,
    changes: &[RowChange],
    id: NodeId,
    node: &Node,
) -> Result<bool> {
    let Some(key) = key(id, node) else {
        return Ok(false);
    };
    let mut value = snapshot
        .rows
        .iter()
        .find(|r| r.table == "allowances" && r.id == key)
        .map(|r| &r.value);
    for change in changes {
        match change {
            RowChange::Put {
                table,
                id,
                value: next,
            } if table == "allowances" && *id == key => value = Some(next),
            RowChange::Delete { table, id } if table == "allowances" && *id == key => value = None,
            _ => {}
        }
    }
    let Some(value) = value else {
        return Ok(false);
    };
    if value["quota_ms"].as_u64().is_none() {
        return Ok(false);
    }
    let delivery: Delivery = decode_stored(value["delivery"].clone())?;
    Ok(matches_dispatch(
        &delivery,
        id,
        node,
        &key,
        snapshot.epoch.get(),
    ))
}
