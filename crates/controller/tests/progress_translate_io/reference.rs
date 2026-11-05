use super::*;
controller!();
impl Controller {
    fn translate_arm(
        &mut self,
        snapshot: &Snapshot,
        observation: Observation,
        commit: &mut Commit,
    ) -> Result<Option<Event>> {
        match observation {
            Observation::Progress { key, checkpoint } => {
                self.progress_allowance(snapshot, &key, checkpoint, commit)?;
                return Ok(None);
            }
            Observation::Quota {
                key,
                reset_ms,
                model,
            } => {
                self.quota_allowance(snapshot, &key, reset_ms, model.as_deref(), commit)?;
                return Ok(None);
            }
            _ => unreachable!("outside G2d scope"),
        }
    }
    pub(crate) fn progress_allowance(
        &mut self,
        snapshot: &Snapshot,
        key: &str,
        checkpoint: String,
        commit: &mut Commit,
    ) -> Result<()> {
        if let Some(row) = snapshot
            .rows
            .iter()
            .find(|r| r.table == "allowances" && r.id == key)
        {
            let mut state: AllowanceState = crate::decode_stored(row.value.clone())?;
            if state.progress.as_ref() != Some(&checkpoint) {
                if state.external_wait.take().is_some() {
                    self.retire_wait(snapshot, &state.delivery, commit)?;
                }
                state.wait_ended = false;
                state.progress = Some(checkpoint);
                state.progress_ms = self.now_ms();
                // A model turn or tool result: a model served this step (#759).
                state.unserved_since_ms = None;
                state.ladder_out_noticed = false;
                state.due_ms = Some(state_decisions::progress_due(
                    state.progress_ms,
                    u64::from(self.allowance(&state.delivery).silence_secs) * 1000,
                    u64::from(self.allowance(&state.delivery).no_progress_secs) * 1000,
                ));
                commit
                    .rows
                    .push(put("allowances", key.into(), json!(state)));
            }
        }
        Ok(())
    }
    pub(crate) fn quota_allowance(
        &self,
        snapshot: &Snapshot,
        key: &str,
        reset_ms: u64,
        model: Option<&str>,
        commit: &mut Commit,
    ) -> Result<()> {
        // Q53: record the model's quota so every step's ladder can skip it.
        if let Some(model) = model
            && state_decisions::record_quota(
                snapshot
                    .rows
                    .iter()
                    .find(|r| r.table == "model_quotas" && r.id == model)
                    .and_then(|r| r.value["reset_ms"].as_u64()),
                reset_ms,
            )
        {
            commit.rows.push(put(
                "model_quotas",
                model.into(),
                json!({"reset_ms": reset_ms}),
            ));
        }
        if let Some(row) = snapshot
            .rows
            .iter()
            .find(|r| r.table == "allowances" && r.id == key)
        {
            let mut state: AllowanceState = crate::decode_stored(row.value.clone())?;
            // A replayed ending of the same model never restarts its wait.
            if state_decisions::quota_replayed(
                state.last_quota_ms,
                state.last_quota_model.as_deref() == model,
                reset_ms,
            ) {
                return Ok(());
            }
            state.last_quota_ms = Some(reset_ms);
            state.last_quota_model = model.map(str::to_owned);
            state.quota_ms = Some(reset_ms);
            state.unserved_since_ms.get_or_insert(self.now_ms());
            // The look asks the adapter whether another model can take the step now.
            state.due_ms = Some(self.now_ms());
            commit
                .rows
                .push(put("allowances", key.into(), json!(state)));
        }
        Ok(())
    }
    fn retire_wait(
        &mut self,
        snapshot: &Snapshot,
        delivery: &Delivery,
        commit: &mut Commit,
    ) -> Result<bool> {
        // State-table "Counter headroom": filesystem subscriptions cannot
        // participate in the input transaction. Their durable intent does.
        commit.rows.push(put(
            "retiring_waits",
            delivery.key.clone(),
            json!({"delivery": delivery}),
        ));
        let _ = snapshot;
        Ok(true)
    }
    pub(crate) fn allowance(&self, delivery: &Delivery) -> &tl3_config::Allowance {
        match upstream(delivery.output) {
            Upstream::Workers => &self.allowances.agent_worker,
            Upstream::PrismaBuild => &self.allowances.prismabuild,
            Upstream::GitHub | Upstream::Mergify => &self.allowances.github_mergify,
            Upstream::Worktrees => &self.allowances.worktrees,
        }
    }
    pub(crate) fn now_ms(&self) -> u64 {
        self.now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
            .min(u64::MAX as u128) as u64
    }
}
pub(super) fn run(
    snapshot: &Snapshot,
    observation: Observation,
    commit: &mut Commit,
) -> Result<Option<Event>> {
    Controller::new().translate_arm(snapshot, observation, commit)
}
