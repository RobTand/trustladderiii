use crate::{
    Controller, Error, Result, decode_stored,
    executor::{Delivery, Observation, Output},
    hydrate,
    input_errors::IssueTarget,
    put, request, snapshot,
};
use serde_json::json;
use tl3_kernel::{EffectKind, Event, NodeId, Phase, Role};
use tl3_store::{Commit, EffectState, Snapshot};

pub(crate) const MISSING: &str = "worker_dispatch_missing";

impl Controller {
    pub(crate) fn reconcile_worker_waits(&mut self) -> Result<()> {
        let snapshot = self.store.load()?;
        self.reconcile_worker_waits_in(&snapshot, &mut |_, _| {})?;
        Ok(())
    }

    // Runs at startup and in the existing event drain, including pool changes.
    // No timer, historical effect scan, worker start, or kernel event is added.
    pub(crate) fn reconcile_worker_waits_in(
        &mut self,
        snapshot: &Snapshot,
        hook: &mut impl FnMut(crate::Boundary, &str),
    ) -> Result<bool> {
        let world = snapshot::decode(snapshot)?;
        let mut commit = Commit::new(
            format!("worker-wait-recovery:{}", snapshot.revision.get()),
            json!({"worker_wait_recovery": true}),
            snapshot,
        );
        for (index, node) in world.nodes.iter().enumerate() {
            hook(crate::Boundary::WorkerWait, &index.to_string());
            let kind = match node.phase {
                Phase::Ingesting => EffectKind::StartRun(Role::Ingest),
                Phase::Auditing => EffectKind::StartRun(Role::Audit),
                Phase::Planning { .. } => EffectKind::StartRun(Role::Plan),
                Phase::Making => EffectKind::StartRun(Role::Make),
                Phase::Reviewing => EffectKind::StartRun(Role::Review),
                Phase::Rebasing => EffectKind::StartRebase,
                Phase::Checking => EffectKind::SubmitChecks,
                _ => continue,
            };
            if !node.operation_pending(kind, node.generation) || node.waiting_incident {
                continue;
            }
            let id = NodeId(index as u64);
            // Watch retirement and reply consumption are separate commits.
            // A retained ending already owns progress across that boundary.
            if snapshot
                .rows
                .iter()
                .filter(|row| row.table == "inbox")
                .any(|row| {
                    serde_json::from_value::<Observation>(row.value.clone())
                        .ok()
                        .and_then(reply_identity)
                        == Some((id, node.generation))
                })
            {
                continue;
            }
            // Starting an operation is the last effect emitted before Await.
            // Recover that immutable identity, never allocate another start key.
            let key = node
                .next_seq
                .checked_sub(1)
                .map(|seq| format!("effect:{index}:{seq}"));
            let watch = snapshot
                .rows
                .iter()
                .find(|r| r.table == "watches" && Some(&r.id) == key.as_ref());
            let tracked = watch.map(|row| decode_stored::<Delivery>(row.value.clone())).transpose()?
                .is_some_and(|watch| watch.generation == node.generation
                    && watch.epoch <= snapshot.epoch.get()
                    && Some(&watch.key) == key.as_ref()
                    && matches!(watch.output, Output::Effect { node: owner, seq, kind: actual, .. }
                        if owner == id && actual == kind && Some(seq) == node.next_seq.checked_sub(1)));
            if tracked
                && snapshot
                    .rows
                    .iter()
                    .any(|r| r.table == "allowances" && Some(&r.id) == key.as_ref())
            {
                continue;
            }
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
            // The normal pending-effect path still cancels stale dispatches.
            let matching = effect.as_ref().zip(delivery.as_ref()).filter(|(effect, delivery)| {
                effect.epoch <= snapshot.epoch
                    && delivery.generation == node.generation
                    && delivery.key == effect.key
                    && matches!(delivery.output, Output::Effect { node: owner, seq, kind: actual, .. }
                        if owner == id && actual == kind && Some(seq) == node.next_seq.checked_sub(1))
                    && (effect.state.is_pending() || effect.state == EffectState::Confirmed)
            });
            if let Some((effect, delivery)) = matching {
                if !tracked {
                    commit
                        .rows
                        .push(put("watches", effect.key.clone(), json!(delivery)));
                }
                if !snapshot
                    .rows
                    .iter()
                    .any(|r| r.table == "allowances" && r.id == effect.key)
                {
                    commit.rows.push(self.allowance_row(delivery.clone()));
                }
                continue;
            }
            if snapshot.rows.iter().any(|r| {
                r.table == "incidents"
                    && r.value["open"] == true
                    && r.value["node"] == json!(id)
                    && r.value["signature"]["class"] == MISSING
            }) {
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
                    kind,
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

fn reply_identity(observation: Observation) -> Option<(NodeId, u64)> {
    match observation {
        Observation::Kernel(Event::Reply {
            node, generation, ..
        })
        | Observation::WorkerReply {
            node, generation, ..
        }
        | Observation::WorkerFailed {
            node, generation, ..
        }
        | Observation::Plan {
            node, generation, ..
        }
        | Observation::Made {
            node, generation, ..
        }
        | Observation::Reviewed {
            node, generation, ..
        }
        | Observation::Checks {
            node, generation, ..
        }
        | Observation::Rebased {
            node, generation, ..
        }
        | Observation::RebaseConflict {
            node, generation, ..
        } => Some((node, generation)),
        _ => None,
    }
}
