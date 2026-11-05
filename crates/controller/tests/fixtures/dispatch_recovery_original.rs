// Verbatim method from cdacce8f99c28ca8dba8f5f76831b77b4dfcd836.
impl<I: RecoveryIo> Reference<I> {
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
