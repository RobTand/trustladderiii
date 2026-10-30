use crate::{
    Controller, Result,
    executor::{Completion, Delivery, Look, Observation, Upstream, upstream},
    hydrate, put, snapshot,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::time::{Duration, UNIX_EPOCH};
use tl3_kernel::{Ack, Event, Fact, NodeId, Out};
use tl3_store::{Commit, EffectState, RowChange, Snapshot};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct AllowanceState {
    pub delivery: Delivery,
    due_ms: Option<u64>,
    progress: Option<String>,
    progress_ms: u64,
    quota_ms: Option<u64>,
    #[serde(default)]
    last_quota_ms: Option<u64>,
    stopping: bool,
    pub incident: Option<String>,
    #[serde(default)]
    healthy: bool,
    #[serde(default)]
    found: Option<String>,
    #[serde(default)]
    failure: Option<String>,
}

impl AllowanceState {
    pub(crate) fn new(delivery: Delivery, now: u64, delay: u64) -> Self {
        Self {
            delivery,
            due_ms: Some(now.saturating_add(delay)),
            progress: None,
            progress_ms: now,
            quota_ms: None,
            last_quota_ms: None,
            stopping: false,
            incident: None,
            healthy: false,
            found: None,
            failure: None,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Incident {
    pub id: String,
    pub node: NodeId,
    pub open: bool,
    pub needs_rob: bool,
    pub summary: String,
    pub signature: String,
}

impl Controller {
    pub fn set_allowances(&mut self, allowances: tl3_config::Allowances) {
        self.allowances = allowances;
    }

    pub(crate) fn now_ms(&self) -> u64 {
        self.now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
            .min(u64::MAX as u128) as u64
    }

    pub(crate) fn allowance(&self, delivery: &Delivery) -> &tl3_config::Allowance {
        match upstream(delivery.output) {
            Upstream::Workers => &self.allowances.agent_worker,
            Upstream::PrismaBuild => &self.allowances.prismabuild,
            Upstream::GitHub | Upstream::Mergify => &self.allowances.github_mergify,
            Upstream::Worktrees => &self.allowances.worktrees,
        }
    }

    pub(crate) fn allowance_row(&self, delivery: Delivery) -> RowChange {
        let delay = u64::from(
            self.allowance(&delivery)
                .silence_secs
                .min(self.allowance(&delivery).no_progress_secs),
        ) * 1000;
        put(
            "allowances",
            delivery.key.clone(),
            json!(AllowanceState::new(delivery, self.now_ms(), delay)),
        )
    }

    /// One timer for the next outstanding allowance. A consumed allowance stays
    /// disarmed until an upstream record supplies a new checkpoint or a baseline.
    pub fn next_look_in(&mut self) -> Result<Option<Duration>> {
        let now = self.now_ms();
        Ok(self
            .store
            .load()?
            .rows
            .iter()
            .filter(|r| r.table == "allowances")
            .map(|r| serde_json::from_value::<AllowanceState>(r.value.clone()))
            .collect::<std::result::Result<Vec<_>, _>>()?
            .iter()
            .filter_map(|s| s.due_ms)
            .min()
            .map(|due| Duration::from_millis(due.saturating_sub(now))))
    }

    pub fn look_due(&mut self) -> Result<()> {
        self.looks(false)
    }

    /// Restart and webhook-gap recovery may request a look without consuming time.
    pub fn look_now(&mut self) -> Result<()> {
        self.looks(true)
    }

    fn looks(&mut self, force: bool) -> Result<()> {
        let snapshot = self.store.load()?;
        let states = snapshot
            .rows
            .iter()
            .filter(|r| r.table == "allowances")
            .map(|r| serde_json::from_value::<AllowanceState>(r.value.clone()))
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let now = self.now_ms();
        let mut looked = false;
        for mut state in states {
            if !force && !state.due_ms.is_some_and(|due| due <= now) {
                continue;
            }
            looked = true;
            state.due_ms = None;
            if state.quota_ms.is_some_and(|reset| reset > now) {
                state.due_ms = state.quota_ms;
                self.save_allowance(&state)?;
                continue;
            }
            if state.quota_ms.is_some() {
                let effect = self
                    .store
                    .effect(&state.delivery.key)?
                    .expect("quota effect");
                if effect.state == EffectState::Confirmed {
                    let mut delivery = state.delivery.clone();
                    delivery.epoch = self.store.load()?.epoch.get();
                    if let Some(completion) = self
                        .adapters
                        .adapter(&delivery)?
                        .resume_after_quota(&delivery)?
                    {
                        self.look_observations(&delivery, completion)?;
                        state.progress = None;
                        state.progress_ms = now;
                        state.due_ms = Some(now.saturating_add(
                            u64::from(self.allowance(&delivery).silence_secs) * 1000,
                        ));
                    } else {
                        return Err(crate::Error::Invalid(
                            "adapter cannot resume a quota ending",
                        ));
                    }
                }
                state.quota_ms = None;
                self.held_effects.remove(&state.delivery.key);
                self.save_allowance(&state)?;
                continue;
            }
            if state.stopping {
                self.finish_stall(&mut state)?;
                continue;
            }
            let result = self
                .adapters
                .adapter(&state.delivery)?
                .look(&state.delivery);
            match result {
                Ok(Look::Found(completion)) => {
                    let effect = self
                        .store
                        .effect(&state.delivery.key)?
                        .expect("allowance effect");
                    let found = serde_json::to_string(&completion)?;
                    let new = effect.state.is_pending()
                        || (state.found.as_ref() != Some(&found)
                            && !completion.observations.is_empty());
                    state.found = Some(found);
                    if new {
                        self.retry_held_effects();
                        if effect.state.is_pending() {
                            self.complete(&effect.key, completion)?;
                        } else {
                            self.look_observations(&state.delivery, completion)?;
                        }
                    }
                }
                Ok(Look::Absent) => {
                    let effect = self
                        .store
                        .effect(&state.delivery.key)?
                        .expect("allowance effect");
                    if self.held_effects.contains_key(&effect.key)
                        && effect.state != EffectState::Cancelling
                        && matches!(state.delivery.output, Out::Effect { .. })
                    {
                        // The owner proved no write survived. Count this failed
                        // delivery once, using the kernel's existing retry rule.
                        self.complete(
                            &effect.key,
                            Completion {
                                ack: Ack::Retryable,
                                observations: vec![],
                            },
                        )?;
                        continue;
                    }
                }
                Ok(Look::Alive { progress }) => {
                    let allowance = self.allowance(&state.delivery).clone();
                    let changed = state.progress.as_ref() != Some(&progress);
                    if changed {
                        state.progress = Some(progress);
                        state.progress_ms = now;
                        self.retry_held_effects();
                        if state.incident.is_some() {
                            self.probe_passed(&state)?;
                        }
                    } else if now.saturating_sub(state.progress_ms)
                        >= u64::from(allowance.no_progress_secs) * 1000
                    {
                        match upstream(state.delivery.output) {
                            Upstream::Workers => {
                                state.stopping = true;
                                state.failure = Some("no_progress".into());
                                self.save_allowance(&state)?;
                                self.finish_stall(&mut state)?;
                                continue;
                            }
                            Upstream::GitHub | Upstream::Mergify => {
                                self.observe(
                                    &format!("stall:{}:{}", state.delivery.key, state.progress_ms),
                                    Observation::Kernel(Event::Fact {
                                        node: state.delivery.node(),
                                        fact: Fact::UpstreamStalled,
                                    }),
                                )?;
                            }
                            // PB alone judges progress-contract failure and loss.
                            Upstream::PrismaBuild | Upstream::Worktrees => {}
                        }
                    }
                    if changed
                        || now.saturating_sub(state.progress_ms)
                            < u64::from(allowance.no_progress_secs) * 1000
                    {
                        state.due_ms = Some(
                            state
                                .progress_ms
                                .saturating_add(u64::from(allowance.no_progress_secs) * 1000),
                        );
                    }
                }
                Ok(Look::Healthy) => {
                    if !state.healthy {
                        self.retry_held_effects();
                    }
                    state.healthy = true;
                    self.probe_passed(&state)?;
                }
                Ok(Look::Quota { reset_ms }) => {
                    state.quota_ms = Some(reset_ms);
                    state.due_ms = (reset_ms > now).then_some(reset_ms);
                }
                Ok(Look::Pending) => {}
                Err(crate::Error::Quota { retry_after }) => {
                    let reset =
                        now.saturating_add(retry_after.as_millis().min(u64::MAX as u128) as u64);
                    state.quota_ms = Some(reset);
                    state.due_ms = (reset > now).then_some(reset);
                }
                Err(error) => {
                    eprintln!("allowance look failed for {}: {error}", state.delivery.key)
                }
            }
            self.save_allowance(&state)?;
        }
        if looked || force {
            let after = self
                .store
                .load()?
                .rows
                .iter()
                .find(|r| r.table == "delivery_looks" && r.id == "github")
                .and_then(|r| r.value.as_u64())
                .unwrap_or(0);
            match self
                .adapters
                .get(Upstream::GitHub)?
                .recover_deliveries(after)
            {
                Ok(Some(next)) if next > after => {
                    let snapshot = self.store.load()?;
                    let mut commit = Commit::new(
                        format!("github-delivery-look:{next}"),
                        json!({"delivery_watermark":next}),
                        &snapshot,
                    );
                    commit
                        .rows
                        .push(put("delivery_looks", "github".into(), json!(next)));
                    self.store.commit(&commit)?;
                    // Kept in the same controller snapshot as other look evidence.
                }
                Ok(_) => {}
                Err(error) => eprintln!("GitHub delivery look failed: {error}"),
            }
        }
        Ok(())
    }

    fn finish_stall(&mut self, state: &mut AllowanceState) -> Result<()> {
        let mut delivery = state.delivery.clone();
        delivery.epoch = self.store.load()?.epoch.get();
        let completion = self.adapters.adapter(&delivery)?.stop_stalled(&delivery)?;
        self.look_observations(&delivery, completion)?;
        state.stopping = false;
        state.due_ms = None;
        self.save_allowance(state)
    }

    fn look_observations(&mut self, delivery: &Delivery, completion: Completion) -> Result<()> {
        for (index, observation) in completion.observations.into_iter().enumerate() {
            self.observe(&format!("look:{}:{index}", delivery.key), observation)?;
        }
        Ok(())
    }

    fn probe_passed(&mut self, state: &AllowanceState) -> Result<()> {
        let world = self.world()?;
        let node = &world.nodes[state.delivery.node().0 as usize];
        if node.waiting_incident && !node.incident_needs_rob {
            self.observe(
                &format!(
                    "probe:{}",
                    state.incident.as_deref().unwrap_or(&state.delivery.key)
                ),
                Observation::Kernel(Event::Fact {
                    node: state.delivery.node(),
                    fact: Fact::IncidentCleared { by_rob: false },
                }),
            )?;
        }
        Ok(())
    }

    fn save_allowance(&mut self, state: &AllowanceState) -> Result<()> {
        let snapshot = self.store.load()?;
        let value = json!(state);
        if snapshot
            .rows
            .iter()
            .any(|r| r.table == "allowances" && r.id == state.delivery.key && r.value == value)
        {
            return Ok(());
        }
        let mut commit = Commit::new(
            format!(
                "allowance:{}:{}",
                state.delivery.key,
                snapshot.revision.get()
            ),
            json!({"look":state.delivery.key}),
            &snapshot,
        );
        commit
            .rows
            .push(put("allowances", state.delivery.key.clone(), value));
        self.store.commit(&commit)?;
        Ok(())
    }

    pub(crate) fn failure_allowance(
        &self,
        snapshot: &Snapshot,
        key: &str,
        class: &str,
        commit: &mut Commit,
    ) -> Result<()> {
        if let Some(row) = snapshot
            .rows
            .iter()
            .find(|r| r.table == "allowances" && r.id == key)
        {
            let mut state: AllowanceState = serde_json::from_value(row.value.clone())?;
            state.failure = Some(class.into());
            commit
                .rows
                .push(put("allowances", key.into(), json!(state)));
        }
        Ok(())
    }

    pub(crate) fn progress_allowance(
        &self,
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
            let mut state: AllowanceState = serde_json::from_value(row.value.clone())?;
            if state.progress.as_ref() != Some(&checkpoint) {
                state.progress = Some(checkpoint);
                state.progress_ms = self.now_ms();
                state.due_ms = Some(
                    state.progress_ms.saturating_add(
                        u64::from(
                            self.allowance(&state.delivery)
                                .silence_secs
                                .min(self.allowance(&state.delivery).no_progress_secs),
                        ) * 1000,
                    ),
                );
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
        commit: &mut Commit,
    ) -> Result<()> {
        if let Some(row) = snapshot
            .rows
            .iter()
            .find(|r| r.table == "allowances" && r.id == key)
        {
            let mut state: AllowanceState = serde_json::from_value(row.value.clone())?;
            if state.last_quota_ms.is_some_and(|old| old >= reset_ms) {
                return Ok(());
            }
            state.last_quota_ms = Some(reset_ms);
            state.quota_ms = Some(reset_ms);
            state.due_ms = Some(reset_ms.max(self.now_ms()));
            commit
                .rows
                .push(put("allowances", key.into(), json!(state)));
        }
        Ok(())
    }

    pub(crate) fn defer_quota(&mut self, key: &str, retry_after: Duration) -> Result<()> {
        let snapshot = self.store.load()?;
        if let Some(row) = snapshot
            .rows
            .iter()
            .find(|r| r.table == "allowances" && r.id == key)
        {
            let mut state: AllowanceState = serde_json::from_value(row.value.clone())?;
            let reset = self
                .now_ms()
                .saturating_add(retry_after.as_millis().min(u64::MAX as u128) as u64);
            state.quota_ms = Some(reset);
            state.due_ms = (reset > self.now_ms()).then_some(reset);
            self.save_allowance(&state)?;
        }
        Ok(())
    }

    pub(crate) fn attach_incident(
        &self,
        snapshot: &Snapshot,
        node: NodeId,
        id: &str,
        commit: &mut Commit,
    ) -> Result<(Upstream, String)> {
        let cause = snapshot
            .rows
            .iter()
            .filter(|r| r.table == "allowances")
            .filter_map(|r| serde_json::from_value::<AllowanceState>(r.value.clone()).ok())
            .filter(|s| {
                s.delivery.node() == node && matches!(s.delivery.output, Out::Effect { .. })
            })
            .max_by_key(|s| s.delivery.generation);
        if let Some(mut state) = cause {
            state.incident = Some(id.into());
            state.due_ms = Some(
                self.now_ms()
                    .saturating_add(u64::from(self.allowance(&state.delivery).silence_secs) * 1000),
            );
            let upstream = upstream(state.delivery.output);
            commit
                .rows
                .push(put("allowances", state.delivery.key.clone(), json!(state)));
            Ok((
                upstream,
                state.failure.unwrap_or_else(|| "operation_failed".into()),
            ))
        } else {
            Ok((Upstream::GitHub, "operation_failed".into()))
        }
    }

    pub(crate) fn prune_allowances(&mut self, snapshot: &Snapshot) -> Result<bool> {
        let world = snapshot::decode(snapshot)?;
        let mut commit = Commit::new(
            format!("allowance-retire:{}", snapshot.revision.get()),
            json!({"retire_allowances":true}),
            snapshot,
        );
        for row in snapshot.rows.iter().filter(|r| r.table == "allowances") {
            let state: AllowanceState = serde_json::from_value(row.value.clone())?;
            let incident_open = state.incident.as_ref().is_some_and(|id| {
                snapshot
                    .rows
                    .iter()
                    .any(|r| r.table == "incidents" && r.id == *id && r.value["open"] == true)
            });
            let pending = snapshot.pending_effects.iter().any(|e| e.key == row.id);
            let watched = snapshot
                .rows
                .iter()
                .any(|r| r.table == "watches" && r.id == row.id);
            if !pending && !watched && !incident_open {
                commit.rows.push(RowChange::Delete {
                    table: "allowances".into(),
                    id: row.id.clone(),
                });
            }
        }
        // Backfill allowances once for a snapshot written before this feature.
        for effect in &snapshot.pending_effects {
            if !snapshot
                .rows
                .iter()
                .any(|r| r.table == "allowances" && r.id == effect.key)
            {
                commit
                    .rows
                    .push(self.allowance_row(hydrate(effect, snapshot)?));
            }
        }
        for row in snapshot.rows.iter().filter(|r| r.table == "watches") {
            let delivery: Delivery = serde_json::from_value(row.value.clone())?;
            let node = &world.nodes[delivery.node().0 as usize];
            if let Out::Effect { kind, .. } = delivery.output
                && node.operation_pending(kind, delivery.generation)
                && !snapshot
                    .rows
                    .iter()
                    .any(|r| r.table == "allowances" && r.id == row.id)
                && !snapshot.pending_effects.iter().any(|e| e.key == row.id)
            {
                commit.rows.push(self.allowance_row(delivery));
            }
        }
        if commit.rows.is_empty() {
            return Ok(false);
        }
        self.store.commit(&commit)?;
        Ok(true)
    }
}

pub(crate) fn quota_waiting(snapshot: &Snapshot, key: &str, _now: u64) -> Result<bool> {
    let state = snapshot
        .rows
        .iter()
        .find(|r| r.table == "allowances" && r.id == key)
        .map(|r| serde_json::from_value::<AllowanceState>(r.value.clone()))
        .transpose()?;
    Ok(state.is_some_and(|s| s.quota_ms.is_some()))
}

pub(crate) fn failure_class(error: &crate::Error) -> &'static str {
    match error {
        crate::Error::Quota { .. } => "quota",
        crate::Error::Io(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
            "permission"
        }
        crate::Error::Io(_) => "transport",
        crate::Error::Adapter(error) => match error.downcast_ref::<tl3_github::Error>() {
            Some(tl3_github::Error::Http { status: 401 | 403 }) => "permission",
            Some(tl3_github::Error::Http { status: 500..=599 }) => "upstream_server",
            Some(tl3_github::Error::Response) => "invalid_response",
            _ => "transport",
        },
        crate::Error::MessageRejected => "message_rejected",
        _ => "setup",
    }
}
