//! #816 extraction: the shell retains the original reads, clock and commit order.
use super::*;

pub(super) trait RecoveryIo {
    fn effect(&mut self, key: &str) -> Result<Option<tl3_store::Effect>>;
    fn commit(&mut self, commit: &Commit) -> Result<()>;
    fn allowance_row(&self, delivery: Delivery) -> RowChange;
    #[allow(clippy::too_many_arguments)]
    fn input_failed(
        &self,
        snapshot: &Snapshot,
        scope: &str,
        target: Option<IssueTarget>,
        title: &str,
        node: Option<NodeId>,
        upstream: crate::executor::Upstream,
        error: Error,
        commit: &mut Commit,
    ) -> Result<()>;
}

pub(super) struct ControllerIo<'a>(pub &'a mut Controller);
impl RecoveryIo for ControllerIo<'_> {
    fn effect(&mut self, key: &str) -> Result<Option<tl3_store::Effect>> {
        Ok(self.0.store.effect(key)?)
    }
    fn commit(&mut self, commit: &Commit) -> Result<()> {
        self.0.store.commit(commit)?;
        Ok(())
    }
    fn allowance_row(&self, delivery: Delivery) -> RowChange {
        self.0.allowance_row(delivery)
    }
    fn input_failed(
        &self,
        snapshot: &Snapshot,
        scope: &str,
        target: Option<IssueTarget>,
        title: &str,
        node: Option<NodeId>,
        upstream: crate::executor::Upstream,
        error: Error,
        commit: &mut Commit,
    ) -> Result<()> {
        self.0.input_failed(
            snapshot, scope, target, title, node, upstream, error, commit,
        )
    }
}

pub(super) struct Recovery<I> {
    pub store: I,
}
impl<I: RecoveryIo> Recovery<I> {
    fn allowance_row(&self, delivery: Delivery) -> RowChange {
        self.store.allowance_row(delivery)
    }
    #[allow(clippy::too_many_arguments)]
    fn input_failed(
        &self,
        snapshot: &Snapshot,
        scope: &str,
        target: Option<IssueTarget>,
        title: &str,
        node: Option<NodeId>,
        upstream: crate::executor::Upstream,
        error: Error,
        commit: &mut Commit,
    ) -> Result<()> {
        self.store.input_failed(
            snapshot, scope, target, title, node, upstream, error, commit,
        )
    }
    pub(super) fn reconcile_worker_waits_in(
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
            let awaited = acknowledged_wait(node);
            let covered = awaited && coverage.covered(id, node, snapshot.epoch.get())?;
            if decision::preflight(awaited, covered) == decision::Repair::Keep {
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
                let repair = decision::repair_dispatch(
                    true,
                    coverage.tracked(id, node, snapshot.epoch.get())?,
                    coverage.allowances.contains(effect.key.as_str()),
                    false,
                );
                if matches!(repair, decision::Repair::Restore { watch: true, .. }) {
                    commit
                        .rows
                        .push(put("watches", effect.key.clone(), json!(delivery)));
                }
                if matches!(
                    repair,
                    decision::Repair::Restore {
                        allowance: true,
                        ..
                    }
                ) {
                    commit.rows.push(self.allowance_row(delivery.clone()));
                }
                continue;
            }
            if decision::repair_dispatch(false, false, false, missing.contains(&id.0))
                == decision::Repair::Keep
            {
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

#[cfg(test)]
#[path = "../dispatch_recovery_tests.rs"]
mod tests;
