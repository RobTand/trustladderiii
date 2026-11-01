use crate::executor::Output as Out;
use crate::{
    Controller, Result,
    executor::{Completion, Delivery, Look, Observation, Upstream, upstream},
    hydrate, put, snapshot,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::time::{Duration, UNIX_EPOCH};
use tl3_kernel::{Ack, Event, Fact, NodeId, Phase, World};
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
    #[serde(default)]
    read_failures: u8,
    #[serde(default)]
    read_incidents: u8,
    #[serde(default)]
    external_wait: Option<tl3_store::ExternalWait>,
    #[serde(default)]
    wait_ended: bool,
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
            read_failures: 0,
            read_incidents: 0,
            external_wait: None,
            wait_ended: false,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Incident {
    pub id: String,
    pub node: Option<NodeId>,
    #[serde(default)]
    pub input_failure: Option<crate::input_errors::InputFailure>,
    pub open: bool,
    pub needs_rob: bool,
    pub blocking: bool,
    pub summary: String,
    pub signature: IncidentSignature,
    #[serde(default)]
    pub effect: Option<String>,
    #[serde(default)]
    pub post_key: Option<String>,
}

pub(crate) fn resolve_effect_incidents(
    snapshot: &Snapshot,
    commit: &mut Commit,
    mut resolved: impl FnMut(&Incident, &str) -> Result<bool>,
) -> Result<()> {
    for row in snapshot.rows.iter().filter(|row| row.table == "incidents") {
        let mut incident: Incident = crate::decode_stored(row.value.clone())?;
        if incident.open
            && let Some(key) = incident.effect.as_deref()
            && resolved(&incident, key)?
        {
            incident.open = false;
            commit
                .rows
                .push(put("incidents", row.id.clone(), json!(incident)));
        }
    }
    Ok(())
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(try_from = "StoredSignature")]
pub(crate) struct IncidentSignature {
    pub upstream: Upstream,
    pub class: String,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum StoredSignature {
    Structured { upstream: Upstream, class: String },
    Legacy(String),
}

impl TryFrom<StoredSignature> for IncidentSignature {
    type Error = &'static str;

    fn try_from(value: StoredSignature) -> std::result::Result<Self, Self::Error> {
        let (upstream, class) = match value {
            StoredSignature::Structured { upstream, class } => (upstream, class),
            StoredSignature::Legacy(signature) => {
                let (source, class) = signature
                    .split_once(':')
                    .ok_or("invalid incident signature")?;
                let upstream = [
                    Upstream::GitHub,
                    Upstream::Mergify,
                    Upstream::PrismaBuild,
                    Upstream::Workers,
                    Upstream::Worktrees,
                ]
                .into_iter()
                .find(|upstream| upstream.kind() == source)
                .ok_or("unknown incident upstream")?;
                (upstream, class.into())
            }
        };
        Ok(Self { upstream, class })
    }
}

pub(crate) enum IncidentCause {
    Kernel { needs_rob: bool },
    ReadFailure { incidents: u64 },
    NoticeFailure { incidents: u64 },
}

pub(crate) fn incident_needs_rob(cause: IncidentCause) -> bool {
    match cause {
        // Kernel notices also cover permanent failures and merge races.
        IncidentCause::Kernel { needs_rob } => needs_rob,
        IncidentCause::ReadFailure { incidents } | IncidentCause::NoticeFailure { incidents } => {
            incidents > 1
        }
    }
}

pub(crate) fn incident_id(snapshot: &Snapshot, cause: impl std::fmt::Display) -> String {
    format!("{}.{}", snapshot.revision.get() + 1, cause)
}

pub(crate) fn incident_note(repository: &crate::Repository, issue: u64, summary: &str) -> String {
    format!(
        "{summary} Request: `https://github.com/{}/{}/issues/{}`.",
        repository.owner, repository.name, issue
    )
}

impl Controller {
    pub(crate) fn resolve_closed_effect_incidents(
        &mut self,
        snapshot: &Snapshot,
        world: &tl3_kernel::World,
        commit: &mut Commit,
    ) -> Result<()> {
        resolve_effect_incidents(snapshot, commit, |incident, key| {
            let Some(node) = incident.node else {
                return Ok(false);
            };
            if !matches!(
                world.nodes[node.0 as usize].phase,
                tl3_kernel::Phase::Closed(_)
            ) {
                return Ok(false);
            }
            let Some(effect) = self.store.effect(key)? else {
                return Ok(false);
            };
            let delivery: Delivery = crate::decode_stored(effect.payload)?;
            // Failed summaries and other notices still need delivery after closure.
            Ok(matches!(delivery.output, Out::Effect { .. }))
        })
    }

    pub(crate) fn cleanup_ended_looks(&mut self) -> Result<()> {
        let snapshot = self.store.load()?;
        let world = snapshot::decode(&snapshot)?;
        let mut commit = Commit::new(
            format!("ended-look-retire:{}", snapshot.revision.get()),
            json!({"retire_ended_looks":true}),
            &snapshot,
        );
        self.retire_ended_effect_looks(&snapshot, &world, &mut commit)?;
        if !commit.rows.is_empty() {
            self.store.commit(&commit)?;
        }
        Ok(())
    }

    pub(crate) fn retire_ended_effect_looks(
        &mut self,
        snapshot: &Snapshot,
        world: &World,
        commit: &mut Commit,
    ) -> Result<()> {
        let mut states = snapshot
            .rows
            .iter()
            .filter(|row| row.table == "allowances")
            .map(|row| crate::decode_stored::<AllowanceState>(row.value.clone()))
            .collect::<std::result::Result<Vec<_>, _>>()?;
        for row in &commit.rows {
            if let RowChange::Put { table, value, .. } = row
                && table == "allowances"
            {
                states.push(crate::decode_stored(value.clone())?);
            }
        }
        let mut retired = std::collections::BTreeSet::new();
        for state in states {
            if !ended_effect_look(snapshot, world, &state.delivery)
                || !retired.insert(state.delivery.key.clone())
            {
                continue;
            }
            if state.external_wait.is_some()
                && !self.retire_wait(snapshot, &state.delivery, commit)?
            {
                continue;
            }
            commit.rows.push(RowChange::Delete {
                table: "allowances".into(),
                id: state.delivery.key.clone(),
            });
            self.remove_hold(&state.delivery);
        }
        Ok(())
    }

    fn retire_wait(
        &mut self,
        snapshot: &Snapshot,
        delivery: &Delivery,
        commit: &mut Commit,
    ) -> Result<bool> {
        match self
            .adapters
            .get(Upstream::PrismaBuild)?
            .watch_wait(delivery, None)
        {
            Ok(()) => Ok(true),
            Err(error) => {
                let request = crate::request(snapshot, delivery.node())?;
                self.input_failed(
                    snapshot,
                    &format!("retire-wait:{}", delivery.key),
                    Some(crate::input_errors::IssueTarget {
                        repository: request.repo,
                        issue: request.issue,
                    }),
                    &request.title,
                    Some(delivery.node()),
                    Upstream::PrismaBuild,
                    error,
                    commit,
                )?;
                Ok(false)
            }
        }
    }

    pub(crate) fn link_allowance_incident(
        &self,
        snapshot: &Snapshot,
        key: &str,
        id: &str,
        commit: &mut Commit,
    ) -> Result<()> {
        if let Some(row) = snapshot
            .rows
            .iter()
            .find(|r| r.table == "allowances" && r.id == key)
        {
            let mut state: AllowanceState = crate::decode_stored(row.value.clone())?;
            state.incident = Some(id.into());
            commit
                .rows
                .push(put("allowances", row.id.clone(), json!(state)));
        }
        Ok(())
    }
    pub fn set_allowances(&mut self, allowances: tl3_config::Allowances) {
        self.allowances = allowances;
    }

    pub(crate) fn retry_queue_observation_looks(&mut self) -> Result<()> {
        let snapshot = self.store.load()?;
        let mut commit = Commit::new(
            format!("deferred-queue-look:{}", snapshot.revision.get()),
            json!({"retry_deferred_queue_observation":true}),
            &snapshot,
        );
        for row in snapshot.rows.iter().filter(|row| row.table == "allowances") {
            let mut state: AllowanceState = crate::decode_stored(row.value.clone())?;
            if !matches!(
                state.delivery.output,
                Out::Effect {
                    kind: tl3_kernel::EffectKind::Enqueue,
                    ..
                }
            ) || (!snapshot
                .pending_effects
                .iter()
                .any(|effect| effect.key == row.id)
                && !snapshot
                    .rows
                    .iter()
                    .any(|watch| watch.table == "watches" && watch.id == row.id))
            {
                continue;
            }
            // Reuse the enqueue allowance. A deferred read creates no new polling timer.
            let due = self
                .now_ms()
                .saturating_add(u64::from(self.allowance(&state.delivery).silence_secs) * 1000)
                .max(state.quota_ms.unwrap_or(0));
            if state.due_ms.is_none_or(|old| old > due) {
                state.due_ms = Some(due);
                commit
                    .rows
                    .push(put("allowances", row.id.clone(), json!(state)));
            }
        }
        if !commit.rows.is_empty() {
            self.store.commit(&commit)?;
        }
        Ok(())
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

    /// One timer for the earliest outstanding allowance, including outage probes.
    pub fn next_look_in(&mut self) -> Result<Option<Duration>> {
        let now = self.now_ms();
        Ok(self
            .store
            .load()?
            .rows
            .iter()
            .filter(|r| r.table == "allowances")
            .map(|r| crate::decode_stored::<AllowanceState>(r.value.clone()))
            .collect::<std::result::Result<Vec<_>, _>>()?
            .iter()
            .filter_map(|s| s.due_ms)
            .min()
            .map(|due| Duration::from_millis(due.saturating_sub(now))))
    }

    pub fn look_due(&mut self) -> Result<()> {
        self.looks(false, None)
    }

    /// Restart and webhook-gap recovery may request a look without consuming time.
    pub fn look_now(&mut self) -> Result<()> {
        self.looks(true, None)
    }

    pub fn look_key(&mut self, key: &str) -> Result<()> {
        self.looks(true, Some(key))
    }

    fn looks(&mut self, force: bool, key: Option<&str>) -> Result<()> {
        self.retry_input_incident_posts()?;
        let snapshot = self.store.load()?;
        let states = snapshot
            .rows
            .iter()
            .filter(|r| r.table == "allowances")
            .map(|r| crate::decode_stored::<AllowanceState>(r.value.clone()))
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let now = self.now_ms();
        let mut github_due = false;
        for mut state in states {
            if key.is_some_and(|key| key != state.delivery.key) {
                continue;
            }
            if !force && !state.due_ms.is_some_and(|due| due <= now) {
                continue;
            }
            if let Some(effect) = self.store.effect(&state.delivery.key)? {
                state.delivery = hydrate(&effect, &snapshot)?;
            }
            github_due |= upstream(state.delivery.output) == Upstream::GitHub;
            if let Err(error) = self.look_state(&snapshot, &mut state, now) {
                self.look_failed(&mut state, error)?;
                self.save_allowance(&state)?;
            }
        }
        if key.is_none() && (github_due || force) {
            let after = self
                .store
                .load()?
                .rows
                .iter()
                .find(|r| r.table == "delivery_looks" && r.id == "github")
                .and_then(|r| r.value.as_u64());
            let snapshot = self.store.load()?;
            let since = snapshot
                .rows
                .iter()
                .find(|r| r.table == "delivery_looks" && r.id == "github_started")
                .and_then(|r| r.value.as_u64());
            let since = if let Some(since) = since {
                since
            } else if after.is_some() {
                0
            } else {
                let mut commit = Commit::new(
                    "github-delivery-start",
                    json!({"delivery_start":now}),
                    &snapshot,
                );
                commit
                    .rows
                    .push(put("delivery_looks", "github_started".into(), json!(now)));
                self.store.commit(&commit)?;
                now
            };
            match self
                .adapters
                .get(Upstream::GitHub)?
                .recover_deliveries(after.unwrap_or_default(), since)
            {
                Ok(Some(next)) if Some(next) > after => {
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
                Err(error) if !crate::input_errors::recoverable(&error) => return Err(error),
                Err(error) => eprintln!("GitHub delivery look failed: {error}"),
            }
        }
        Ok(())
    }

    fn look_state(
        &mut self,
        snapshot: &Snapshot,
        state: &mut AllowanceState,
        now: u64,
    ) -> Result<()> {
        state.due_ms = None;
        if let Some(id) = &state.incident
            && let Some(row) = snapshot
                .rows
                .iter()
                .find(|r| r.table == "incidents" && r.id == *id && r.value["open"] == true)
        {
            let incident: Incident = crate::decode_stored(row.value.clone())?;
            // A healthy GET cannot prove that command restrictions were fixed.
            // Keep this setup refusal for explicit incident clearance, even
            // after the failed delivery's adapter records have retired.
            if incident.signature.class == "queue_command_refused" {
                self.save_allowance(state)?;
                return Ok(());
            }
            if incident.effect.as_deref() == Some(&state.delivery.key) {
                if !incident.needs_rob {
                    match self
                        .adapters
                        .adapter(&state.delivery)?
                        .probe(&state.delivery)
                    {
                        Ok(true) => {
                            self.probe_passed(state)?;
                            self.retry_held_effects(upstream(state.delivery.output));
                            self.rearm(state);
                        }
                        Ok(false) => self.rearm(state),
                        Err(error) => self.look_failed(state, error)?,
                    }
                }
                self.save_allowance(state)?;
                return Ok(());
            }
        }
        if state.quota_ms.is_some_and(|reset| reset > now) {
            state.due_ms = state.quota_ms;
            self.save_allowance(state)?;
            return Ok(());
        }
        if state.quota_ms.is_some() {
            let effect = self
                .store
                .effect(&state.delivery.key)?
                .expect("quota effect");
            if effect.state == EffectState::Confirmed {
                let mut delivery = state.delivery.clone();
                delivery.epoch = self.store.load()?.epoch.get();
                match self
                    .adapters
                    .adapter(&delivery)?
                    .resume_after_quota(&delivery)
                {
                    Ok(Some(completion)) => {
                        self.look_observations(&delivery, completion)?;
                        state.progress = None;
                        state.progress_ms = now;
                        self.rearm(state);
                    }
                    result => {
                        let error = match result {
                            Err(error) => error,
                            Ok(None) => {
                                crate::Error::Invalid("adapter cannot resume a quota ending")
                            }
                            Ok(Some(_)) => unreachable!(),
                        };
                        self.look_failed(state, error)?;
                        self.save_allowance(state)?;
                        return Ok(());
                    }
                }
            }
            state.quota_ms = None;
            self.remove_hold(&state.delivery);
            self.save_allowance(state)?;
            return Ok(());
        }
        if state.stopping {
            self.finish_stall(state)?;
            return Ok(());
        }
        let result = self
            .adapters
            .adapter(&state.delivery)?
            .look(&state.delivery);
        let result = match result {
            Err(error)
                if error.is_github_not_found()
                    && state.failure.as_deref() == Some("not_found")
                    && matches!(state.delivery.output, Out::Effect { .. }) =>
            {
                Ok(Look::Absent)
            }
            result => result,
        };
        if matches!(&result, Ok(look) if !matches!(look, Look::Waiting { .. } | Look::Pending)) {
            state.read_failures = 0;
        }
        if !matches!(result, Ok(Look::Waiting { .. })) && state.external_wait.is_some() {
            self.adapters
                .get(Upstream::PrismaBuild)?
                .watch_wait(&state.delivery, None)?;
            state.external_wait = None;
            state.wait_ended = false;
        }
        match result {
            Ok(Look::Found(mut completion)) => {
                let effect = self
                    .store
                    .effect(&state.delivery.key)?
                    .expect("allowance effect");
                let token_only =
                    completion.progress.is_some() && completion.observations.is_empty();
                let changed = if completion.progress.is_some() {
                    let snapshot = self.store.load()?;
                    let mut commit = Commit::new(
                        format!(
                            "look-progress:{}:{}",
                            state.delivery.key,
                            snapshot.revision.get()
                        ),
                        json!({"progress_read":state.delivery.key}),
                        &snapshot,
                    );
                    let changed = self.record_progress(
                        &snapshot,
                        &mut commit,
                        &state.delivery.key,
                        completion.progress.as_ref(),
                    )?;
                    if !commit.rows.is_empty() {
                        self.store.commit(&commit)?;
                    }
                    changed == Some(true)
                } else {
                    false
                };
                if changed {
                    state.progress_ms = now;
                    state.due_ms = Some(now.saturating_add(
                        u64::from(self.allowance(&state.delivery).no_progress_secs) * 1000,
                    ));
                }
                // The typed watch row is the sole token store.
                completion.progress = None;
                if token_only {
                    if effect.state.is_pending() {
                        self.complete(&effect.key, completion)?;
                    }
                    if self.look_alive(state, changed)? {
                        return Ok(());
                    }
                } else {
                    let found = serde_json::to_string(&completion)?;
                    let new = effect.state.is_pending()
                        || (state.found.as_ref() != Some(&found)
                            && !completion.observations.is_empty());
                    state.found = Some(found);
                    if new {
                        if completion.releases_holds() {
                            self.retry_held_effects(upstream(state.delivery.output));
                        }
                        if effect.state.is_pending() {
                            self.complete(&effect.key, completion)?;
                        } else {
                            self.look_observations(&state.delivery, completion)?;
                        }
                    }
                }
            }
            Ok(Look::Absent) => {
                let effect = self
                    .store
                    .effect(&state.delivery.key)?
                    .expect("allowance effect");
                if (self.is_held(&state.delivery) || state.failure.as_deref() == Some("not_found"))
                    && effect.state.is_pending()
                    && effect.state != EffectState::Cancelling
                    && matches!(state.delivery.output, Out::Effect { .. })
                {
                    // Absence or a repeated 404 spends the existing retry allowance.
                    self.complete(
                        &effect.key,
                        Completion {
                            failure_class: None,
                            progress: None,
                            ack: Ack::Retryable,
                            observations: vec![],
                        },
                    )?;
                    return Ok(());
                }
            }
            Ok(Look::Alive { progress }) => {
                let changed = state.progress.as_ref() != Some(&progress);
                state.progress = Some(progress);
                if self.look_alive(state, changed)? {
                    return Ok(());
                }
            }
            Ok(Look::Waiting { wait, checkout }) => {
                if state.external_wait.as_ref() != Some(&wait) {
                    state.external_wait = Some(wait.clone());
                    state.wait_ended = false;
                }
                let record = if wait.upstream == "prismabuild" {
                    self.adapters
                        .get(Upstream::PrismaBuild)?
                        .wait_record(&wait.key, &checkout)
                } else {
                    Ok(crate::executor::WaitRecord::Ended)
                };
                match record {
                    Ok(crate::executor::WaitRecord::Alive) => {
                        self.adapters
                            .get(Upstream::PrismaBuild)?
                            .watch_wait(&state.delivery, Some(&wait))?;
                        state.read_failures = 0;
                        state.wait_ended = false;
                        state.progress = None;
                    }
                    Ok(crate::executor::WaitRecord::Ended) => {
                        self.adapters
                            .get(Upstream::PrismaBuild)?
                            .watch_wait(&state.delivery, None)?;
                        state.read_failures = 0;
                        if !state.wait_ended {
                            state.wait_ended = true;
                            state.progress_ms = now;
                        }
                        if self.look_alive(state, false)? {
                            return Ok(());
                        }
                    }
                    Err(error) => self.look_failed(state, error)?,
                }
            }
            Ok(Look::Healthy) => {
                if !state.healthy {
                    self.retry_held_effects(upstream(state.delivery.output));
                }
                state.healthy = true;
                self.probe_passed(state)?;
            }
            Ok(Look::Quota { reset_ms }) => {
                state.quota_ms = Some(reset_ms);
                state.due_ms = Some(reset_ms.max(now));
            }
            Ok(Look::Pending) => {
                self.look_failed(state, crate::Error::Invalid("upstream record unavailable"))?;
            }
            Err(crate::Error::Quota { retry_after }) => {
                let reset =
                    now.saturating_add(retry_after.as_millis().min(u64::MAX as u128) as u64);
                state.quota_ms = Some(reset);
                state.due_ms = Some(reset.max(now));
            }
            Err(error) => {
                self.look_failed(state, error)?;
            }
        }
        if let Some(id) = &state.incident {
            let incident = self
                .store
                .load()?
                .rows
                .into_iter()
                .find(|r| r.table == "incidents" && r.id == *id)
                .map(|r| crate::decode_stored::<Incident>(r.value))
                .transpose()?;
            if let Some(incident) = incident.filter(|i| i.open) {
                if !incident.needs_rob {
                    match self
                        .adapters
                        .adapter(&state.delivery)?
                        .probe(&state.delivery)
                    {
                        Ok(true) => {
                            state.healthy = true;
                            self.retry_held_effects(upstream(state.delivery.output));
                            self.probe_passed(state)?;
                        }
                        Ok(false) => self.rearm(state),
                        Err(error) => self.look_failed(state, error)?,
                    }
                }
            } else {
                state.incident = None;
            }
        }
        self.save_allowance(state)?;
        Ok(())
    }

    pub(crate) fn record_progress(
        &self,
        snapshot: &Snapshot,
        commit: &mut Commit,
        key: &str,
        token: Option<&crate::executor::ProgressToken>,
    ) -> Result<Option<bool>> {
        let Some(token) = token else { return Ok(None) };
        let Some(row) = snapshot
            .rows
            .iter()
            .find(|r| r.table == "watches" && r.id == key)
        else {
            return Ok(None);
        };
        let previous: Option<crate::executor::WatchProgress> = row
            .value
            .get("progress")
            .map(|value| crate::decode_stored(value.clone()))
            .transpose()?;
        let progress = crate::executor::WatchProgress {
            previous: previous.as_ref().map(|p| p.current.clone()),
            current: token.clone(),
        };
        let changed = progress.changed();
        if previous.as_ref() != Some(&progress) {
            let mut value = row.value.clone();
            value["progress"] = json!(progress);
            commit.rows.push(put("watches", key.into(), value));
        }
        if changed
            && let Some(row) = snapshot
                .rows
                .iter()
                .find(|r| r.table == "allowances" && r.id == key)
        {
            let mut state: AllowanceState = crate::decode_stored(row.value.clone())?;
            state.progress_ms = self.now_ms();
            state.due_ms = Some(state.progress_ms.saturating_add(
                u64::from(self.allowance(&state.delivery).no_progress_secs) * 1000,
            ));
            state.progress = None;
            state.found = None;
            commit
                .rows
                .push(put("allowances", key.into(), json!(state)));
        }
        Ok(Some(changed))
    }

    fn look_alive(&mut self, state: &mut AllowanceState, changed: bool) -> Result<bool> {
        let now = self.now_ms();
        let allowance = self.allowance(&state.delivery).clone();
        if changed {
            state.progress_ms = now;
            self.retry_held_effects(upstream(state.delivery.output));
        } else if now.saturating_sub(state.progress_ms)
            >= u64::from(allowance.no_progress_secs) * 1000
        {
            match upstream(state.delivery.output) {
                Upstream::Workers => {
                    state.stopping = true;
                    state.failure = Some("no_progress".into());
                    self.save_allowance(state)?;
                    self.finish_stall(state)?;
                    return Ok(true);
                }
                Upstream::GitHub | Upstream::Mergify => {
                    state.failure = Some("queue_unfinished".into());
                    self.save_allowance(state)?;
                    self.observe(
                        &format!("stall:{}:{}", state.delivery.key, state.progress_ms),
                        Observation::Kernel(Event::Fact {
                            node: state.delivery.node(),
                            fact: Fact::UpstreamStalled,
                        }),
                    )?;
                }
                // PrismaBuild owns progress. Worktrees has no progress contract.
                Upstream::PrismaBuild | Upstream::Worktrees => {}
            }
        }
        if changed
            || now.saturating_sub(state.progress_ms) < u64::from(allowance.no_progress_secs) * 1000
        {
            state.due_ms = Some(
                state
                    .progress_ms
                    .saturating_add(u64::from(allowance.no_progress_secs) * 1000),
            );
        }
        Ok(false)
    }

    fn finish_stall(&mut self, state: &mut AllowanceState) -> Result<()> {
        let mut delivery = state.delivery.clone();
        delivery.epoch = self.store.load()?.epoch.get();
        let completion = match self.adapters.adapter(&delivery)?.stop_stalled(&delivery) {
            Ok(completion) => completion,
            Err(error) => {
                self.look_failed(state, error)?;
                return self.save_allowance(state);
            }
        };
        self.look_observations(&delivery, completion)?;
        state.stopping = false;
        state.due_ms = None;
        self.save_allowance(state)
    }

    fn rearm(&self, state: &mut AllowanceState) {
        state.due_ms = Some(
            self.now_ms()
                .saturating_add(u64::from(self.allowance(&state.delivery).silence_secs) * 1000),
        );
    }

    fn look_failed(&mut self, state: &mut AllowanceState, error: crate::Error) -> Result<()> {
        if !crate::input_errors::recoverable(&error) {
            return Err(error);
        }
        eprintln!(
            "allowance action failed for {}: {error}",
            state.delivery.key
        );
        if let crate::Error::Quota { retry_after } = &error {
            let reset = self
                .now_ms()
                .saturating_add(retry_after.as_millis().min(u64::MAX as u128) as u64);
            state.quota_ms = Some(reset);
            state.due_ms = Some(reset);
            return Ok(());
        }
        state.failure = Some(failure_class(&error).into());
        state.healthy = false;
        self.rearm(state);
        let snapshot = self.store.load()?;
        if state.incident.as_ref().is_some_and(|id| {
            snapshot
                .rows
                .iter()
                .any(|r| r.table == "incidents" && r.id == *id && r.value["open"] == true)
        }) {
            return Ok(());
        }
        if matches!(
            state.delivery.output,
            Out::Notice {
                notice: tl3_kernel::Notice::Incident { .. },
                ..
            }
        ) {
            return Ok(());
        }
        state.read_failures += 1;
        if state.read_failures < 2 {
            return Ok(());
        }
        state.read_failures = 0;
        state.read_incidents = state.read_incidents.saturating_add(1);
        let needs_rob = incident_needs_rob(IncidentCause::ReadFailure {
            incidents: u64::from(state.read_incidents),
        });
        let id = incident_id(&snapshot, "read");
        state.incident = Some(id.clone());
        let request = crate::request(&snapshot, state.delivery.node())?;
        let signature = IncidentSignature {
            upstream: upstream(state.delivery.output),
            class: failure_class(&error).into(),
        };
        let app_login = self
            .adapters
            .adapter(&state.delivery)?
            .app_login(request.repo.installation);
        let summary = incident_summary(&request.title, &signature, needs_rob, app_login.as_deref());
        let mut commit = Commit::new(
            format!("incident:{id}"),
            json!({"read_outage":state.delivery.key}),
            &snapshot,
        );
        commit.rows.push(put(
            "incidents",
            id.clone(),
            json!(Incident {
                id: id.clone(),
                node: Some(state.delivery.node()),
                input_failure: None,
                open: true,
                needs_rob,
                blocking: false,
                summary: summary.clone(),
                signature: signature.clone(),
                effect: Some(state.delivery.key.clone()),
                post_key: Some(format!("incident:{id}")),
            }),
        ));
        let mut delivery = state.delivery.clone();
        delivery.key = format!("incident:{id}");
        delivery.output = Out::Notice {
            node: delivery.node(),
            notice: tl3_kernel::Notice::Incident { needs_rob },
        };
        delivery.epoch = snapshot.epoch.get();
        delivery.note = Some(incident_note(&request.repo, request.issue, &summary));
        delivery.question_context = Some(json!({"issue_title":request.title,
            "request":format!("{} (https://github.com/{}/{}/issues/{})", request.title,
                request.repo.owner, request.repo.name, request.issue), "signature":signature, "incident_id":id}));
        if let Some(o) = &self.bootstrap.operations {
            delivery.repository = crate::Repository {
                owner: o.owner.clone(),
                name: o.name.clone(),
                installation: o.installation_id,
            };
            delivery.issue = o.number;
        }
        commit
            .rows
            .push(put("allowances", state.delivery.key.clone(), json!(state)));
        commit.rows.push(self.allowance_row(delivery.clone()));
        commit.effects.push(delivery.into_effect());
        // Preserve the upstream key and unknown outcome. A read outage cannot
        // acknowledge the write or authorize a replacement operation.
        self.store.commit(&commit)?;
        Ok(())
    }

    fn look_observations(&mut self, delivery: &Delivery, completion: Completion) -> Result<()> {
        let snapshot = self.store.load()?;
        let mut commit = Commit::new(
            format!(
                "look-observations:{}:{}",
                delivery.key,
                snapshot.revision.get()
            ),
            json!(&completion),
            &snapshot,
        );
        for (index, observation) in completion.observations.iter().enumerate() {
            self.queue_observation(&snapshot, &mut commit, &delivery.key, index, observation)?;
            if let Some(source) = crate::observation_upstream(&snapshot, observation)? {
                self.retry_held_effects(source);
            }
        }
        if !commit.rows.is_empty() {
            self.store.commit(&commit)?;
        }
        Ok(())
    }

    fn probe_passed(&mut self, state: &AllowanceState) -> Result<()> {
        let Some(id) = &state.incident else {
            return Ok(());
        };
        let snapshot = self.store.load()?;
        let Some(row) = snapshot
            .rows
            .iter()
            .find(|r| r.table == "incidents" && r.id == *id)
        else {
            return Ok(());
        };
        let mut incident: Incident = crate::decode_stored(row.value.clone())?;
        if !incident.open || incident.needs_rob {
            return Ok(());
        }
        if incident.effect.is_some() {
            incident.open = false;
            let mut commit =
                Commit::new(format!("probe:{id}"), json!({"probe_passed":id}), &snapshot);
            self.retry_notice(&snapshot, &incident, &mut commit)?;
            commit
                .rows
                .push(put("incidents", id.clone(), json!(incident)));
            self.store.commit(&commit)?;
            return Ok(());
        }
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

    pub(crate) fn resume_incident_look(
        &self,
        snapshot: &Snapshot,
        incident: &Incident,
        commit: &mut Commit,
    ) -> Result<()> {
        if let Some(key) = &incident.effect
            && let Some(row) = snapshot
                .rows
                .iter()
                .find(|r| r.table == "allowances" && r.id == *key)
        {
            let mut state: AllowanceState = crate::decode_stored(row.value.clone())?;
            state.due_ms = Some(self.now_ms());
            commit
                .rows
                .push(put("allowances", key.clone(), json!(state)));
        }
        Ok(())
    }

    fn save_allowance(&mut self, state: &AllowanceState) -> Result<()> {
        let snapshot = self.store.load()?;
        if ended_effect_look(&snapshot, &snapshot::decode(&snapshot)?, &state.delivery) {
            return Ok(());
        }
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
            let mut state: AllowanceState = crate::decode_stored(row.value.clone())?;
            state.failure = Some(class.into());
            state.healthy = false;
            if state.quota_ms.is_none() {
                state.due_ms = Some(self.now_ms().saturating_add(
                    u64::from(self.allowance(&state.delivery).silence_secs) * 1000,
                ));
            }
            commit
                .rows
                .push(put("allowances", key.into(), json!(state)));
        }
        Ok(())
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
                    self.adapters
                        .get(Upstream::PrismaBuild)?
                        .watch_wait(&state.delivery, None)?;
                }
                state.wait_ended = false;
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
            let mut state: AllowanceState = crate::decode_stored(row.value.clone())?;
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
            let mut state: AllowanceState = crate::decode_stored(row.value.clone())?;
            let reset = self
                .now_ms()
                .saturating_add(retry_after.as_millis().min(u64::MAX as u128) as u64);
            state.quota_ms = Some(reset);
            state.due_ms = Some(reset.max(self.now_ms()));
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
            .map(|r| crate::decode_stored::<AllowanceState>(r.value.clone()))
            .collect::<Result<Vec<_>>>()?
            .into_iter()
            .filter(|s| {
                s.delivery.node() == node && matches!(s.delivery.output, Out::Effect { .. })
            })
            .max_by_key(|s| {
                let seq = match s.delivery.output {
                    Out::Effect { seq, .. } => seq,
                    Out::Notice { .. } | Out::Comment { .. } => unreachable!(),
                };
                (s.delivery.generation, seq)
            });
        if let Some(mut state) = cause {
            if let Some(value) = commit.rows.iter().rev().find_map(|row| match row {
                RowChange::Put { table, id, value }
                    if table == "allowances" && id == &state.delivery.key =>
                {
                    Some(value)
                }
                _ => None,
            }) {
                state = crate::decode_stored(value.clone())?;
            }
            state.incident = Some(id.into());
            state.healthy = false;
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
        // Repair snapshots from before closed requests retired recovery incidents.
        self.resolve_closed_effect_incidents(snapshot, &world, &mut commit)?;
        self.retire_ended_effect_looks(snapshot, &world, &mut commit)?;
        for row in snapshot.rows.iter().filter(|r| r.table == "allowances") {
            let state: AllowanceState = crate::decode_stored(row.value.clone())?;
            if ended_effect_look(snapshot, &world, &state.delivery) {
                continue;
            }
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
                if state.external_wait.is_some()
                    && !self.retire_wait(snapshot, &state.delivery, &mut commit)?
                {
                    continue;
                }
                commit.rows.push(RowChange::Delete {
                    table: "allowances".into(),
                    id: row.id.clone(),
                });
            }
        }
        // Backfill allowances once for a snapshot written before this feature.
        for effect in &snapshot.pending_effects {
            let delivery = hydrate(effect, snapshot)?;
            if ended_effect_look(snapshot, &world, &delivery) {
                continue;
            }
            if !snapshot
                .rows
                .iter()
                .any(|r| r.table == "allowances" && r.id == effect.key)
            {
                commit.rows.push(self.allowance_row(delivery));
            }
        }
        for row in snapshot.rows.iter().filter(|r| r.table == "watches") {
            let delivery: Delivery = crate::decode_stored(row.value.clone())?;
            let node = &world.nodes[delivery.node().0 as usize];
            if let Out::Effect { kind, .. } = delivery.output
                && !ended_effect_look(snapshot, &world, &delivery)
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

fn ended_effect_look(snapshot: &Snapshot, world: &World, delivery: &Delivery) -> bool {
    // Summaries and correction notices still have to finish delivery.
    let Out::Effect { node, .. } = delivery.output else {
        return false;
    };
    let node = &world.nodes[node.0 as usize];
    matches!(node.phase, Phase::Closed(_))
        || (matches!(node.phase, Phase::Stopping(_))
            && delivery.generation < node.generation
            // Pending cancellation still needs its quota and reconciliation evidence.
            && !snapshot.pending_effects.iter().any(|effect| effect.key == delivery.key))
}

pub(crate) fn quota_waiting(snapshot: &Snapshot, key: &str, _now: u64) -> Result<bool> {
    let state = snapshot
        .rows
        .iter()
        .find(|r| r.table == "allowances" && r.id == key)
        .map(|r| crate::decode_stored::<AllowanceState>(r.value.clone()))
        .transpose()?;
    Ok(state.is_some_and(|s| s.quota_ms.is_some()))
}

pub(crate) fn failure_class(error: &crate::Error) -> &'static str {
    match error {
        crate::Error::Quota { .. } => "quota",
        crate::Error::Permanent { class, .. } => class,
        crate::Error::Io(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
            "permission"
        }
        crate::Error::Io(_) => "transport",
        crate::Error::Adapter(error) => error
            .downcast_ref::<tl3_github::Error>()
            .map_or("transport", |error| error.failure_class().as_str()),
        crate::Error::MessageRejected => "message_rejected",
        _ => "setup",
    }
}

pub(crate) fn outage_waiting(snapshot: &Snapshot, key: &str) -> bool {
    snapshot.rows.iter().any(|r| {
        r.table == "incidents" && r.value["open"] == true && r.value["effect"].as_str() == Some(key)
    })
}

pub(crate) fn incident_summary(
    title: &str,
    signature: &IncidentSignature,
    needs_rob: bool,
    app_login: Option<&str>,
) -> String {
    let source = match signature.upstream {
        Upstream::Workers => "the agent worker",
        Upstream::Worktrees => "the Git worktree",
        Upstream::PrismaBuild => "PrismaBuild",
        Upstream::Mergify => "Mergify",
        Upstream::GitHub => "GitHub",
    };
    let reason = match signature.class.as_str() {
        "no_progress" => "stopped making progress",
        "queue_unfinished" => "has not finished its queue check",
        "permission" => "refused access",
        "queue_command_permission" => {
            "could not post the queue command. Check TL3's GitHub App repository permission Issues: Read and write and approve any updated installation permissions"
        }
        "queue_command_refused" => "refused the queue command",
        "actions_write_required" => {
            "cannot rerun workflows. Grant TL3's GitHub App repository permission Actions: Read and write, then approve the installation's updated permissions"
        }
        "actions_run_expired" => "cannot rerun a workflow run older than 30 days",
        "actions_refused" => {
            "refused the workflow request. Check the workflow and repository restrictions"
        }
        "not_found" => "could not find the requested resource",
        "invalid_request" => "rejected the request",
        // Incidents stored before Q46 retain this class. Publication now reports Conflict.
        "unrebased_head" => {
            "refused to publish because the checked head conflicts with current main"
        }
        "dirty_worktree" => "has uncommitted or untracked files",
        "invalid_branch" => "rejected an invalid branch name",
        "invalid_commit" => "rejected an invalid commit id",
        "invalid_remote" => "rejected an invalid remote",
        "invalid_worktree_path" => "rejected a path outside its configured root",
        "invalid_token" => "rejected its installation token",
        "missing_branch" => "could not find the branch or base",
        "reserved_branch" => "refused to change the reserved main branch",
        "worktree_occupied" => "found a path or branch owned by another checkout",
        "worktree_recovery" => "needs recovery after an interrupted rebase",
        "transport" => "could not be reached",
        "upstream_server" => "reported a service error",
        "invalid_response" => "returned an unreadable response",
        "quota" => "reached its usage allowance",
        "message_rejected" => "could not prepare a readable message",
        "setup" => "could not complete its setup",
        _ => "could not finish the operation",
    };
    let next = if needs_rob {
        "Rob must clear this recurring problem."
    } else {
        "Work is waiting for recovery."
    };
    let setup = if signature.class == "queue_command_refused" {
        format!(
            " In the served repo's .mergify.yml, allow {} in commands_restrictions for queue and requeue, then clear this incident.",
            app_login.unwrap_or("TL3's App bot login")
        )
    } else {
        String::new()
    };
    format!("For \"{title}\", {source} {reason}.{setup} {next}")
}

pub(crate) fn queue_incident_detail(snapshot: &Snapshot, node: NodeId) -> Result<Option<String>> {
    use crate::executor::{ProgressToken, WatchProgress};
    let mut progress = None;
    for row in snapshot.rows.iter().filter(|r| r.table == "watches") {
        let delivery: Delivery = crate::decode_stored(row.value.clone())?;
        if delivery.node() == node
            && let Some(value) = row.value.get("progress")
        {
            progress = Some(crate::decode_stored::<WatchProgress>(value.clone())?);
            break;
        }
    }
    let Some(progress) = progress else {
        return Ok(None);
    };
    let ProgressToken::Mergify(progress) = progress.current;
    let Some(check) = progress.queue_check else {
        return Ok(None);
    };
    let title = check.title.as_deref().unwrap_or("Merge queue");
    let status = match check.status.as_str() {
        "in_progress" => "still in progress",
        "queued" => "still queued",
        "completed" => "completed",
        _ => "unfinished",
    };
    Ok(Some(match progress.merged_at {
        Some(time) => format!(
            "GitHub merged the change at {time}, but Mergify's check \"{title}\" is {status}."
        ),
        None => format!("Mergify's check \"{title}\" is {status}."),
    }))
}

#[cfg(test)]
mod git_failure_tests {
    use super::*;
    #[test]
    fn deterministic_git_failures_are_permanent_incidents() {
        use tl3_worktrees::Error as W;
        for (error, expected) in [
            (W::BranchName, "invalid_branch"),
            (W::CommitId, "invalid_commit"),
            (W::Remote, "invalid_remote"),
            (W::WorktreePath, "invalid_worktree_path"),
            (W::Token, "invalid_token"),
            (W::Response, "invalid_response"),
            (W::MissingBranch, "missing_branch"),
            (W::MainBranch, "reserved_branch"),
            (W::WorktreeOccupied, "worktree_occupied"),
            (W::DirtyWorktree, "dirty_worktree"),
            (W::Recovery, "worktree_recovery"),
        ] {
            assert_eq!(error.permanent_class(), Some(expected));
            let error = crate::Error::from(error);
            let crate::Error::Permanent { class, source } = &error else {
                panic!("retain the permanent Git error and its class");
            };
            assert_eq!(*class, expected);
            assert!(source.downcast_ref::<W>().is_some());
            assert_eq!(failure_class(&error), expected, "{error}");
            if expected == "dirty_worktree" {
                assert!(
                    incident_summary(
                        "request",
                        &IncidentSignature {
                            upstream: Upstream::Worktrees,
                            class: failure_class(&error).into()
                        },
                        false,
                        None
                    )
                    .contains("has uncommitted or untracked files")
                );
            }
        }
    }

    #[test]
    fn ambiguous_git_failures_remain_transport() {
        use tl3_worktrees::Error as W;
        for error in [
            W::Io,
            W::Spawn,
            W::Poisoned,
            W::Git {
                operation: "fetch",
                code: Some(128),
            },
        ] {
            assert_eq!(failure_class(&error.into()), "transport");
        }
    }
}

#[cfg(test)]
mod incident_tests {
    use super::*;

    #[test]
    fn legacy_incident_signature_remains_readable() {
        let signature: IncidentSignature =
            serde_json::from_value(json!("Workers:transport")).unwrap();
        assert_eq!(signature.upstream, Upstream::Workers);
        assert_eq!(signature.class, "transport");
        assert_eq!(
            json!(signature),
            json!({"upstream":"Workers", "class":"transport"})
        );
        assert!(
            incident_summary("Greeting", &signature, false, None)
                .contains("the agent worker could not be reached")
        );
    }

    #[test]
    fn escalation_preserves_each_cause() {
        for needs_rob in [false, true] {
            assert_eq!(
                incident_needs_rob(IncidentCause::Kernel { needs_rob }),
                needs_rob
            );
        }
        for incidents in [1, 2, 3] {
            assert_eq!(
                incident_needs_rob(IncidentCause::ReadFailure { incidents }),
                incidents > 1
            );
            assert_eq!(
                incident_needs_rob(IncidentCause::NoticeFailure { incidents }),
                incidents > 1
            );
        }
    }
}

impl Controller {
    pub(crate) fn check_setup_incident(
        &self,
        snapshot: &Snapshot,
        commit: &mut Commit,
        node: NodeId,
        permissions: &[String],
    ) -> Result<()> {
        let id = format!("required-checks:{}", node.0);
        let key = format!("incident:{id}");
        let request = crate::request(snapshot, node)?;
        let summary = format!(
            "Required-check setup: {}. TL3 continues with the available requirements and check runs.",
            permissions.join("; ")
        );
        let signature = IncidentSignature {
            upstream: Upstream::Mergify,
            class: "setup".into(),
        };
        commit.rows.push(put(
            "incidents",
            id.clone(),
            json!(Incident {
                id: id.clone(),
                node: Some(node),
                input_failure: None,
                open: true,
                needs_rob: false,
                blocking: false,
                summary: summary.clone(),
                signature: signature.clone(),
                effect: None,
                post_key: Some(key.clone()),
            }),
        ));
        let world = snapshot::decode(snapshot)?;
        let current = &world.nodes[node
            .index()
            .ok_or(crate::Error::Invalid("node index overflow"))?];
        let mut delivery = Delivery {
            key,
            epoch: snapshot.epoch.get(),
            output: Out::Notice {
                node,
                notice: tl3_kernel::Notice::Incident { needs_rob: false },
            },
            generation: current.generation,
            head_sha: crate::commit_sha(snapshot, current.head).map(str::to_owned),
            repository: request.repo.clone(),
            issue: request.issue,
            pull_request: None,
            proposed: None,
            paths: vec![],
            attempt: current.attempt,
            round: current.round,
            replanned: false,
            checks: None,
            job_generation: None,
            question: None,
            note: Some(incident_note(&request.repo, request.issue, &summary)),
            scope_removals: None,
            marker_key: None,
            steering: None,
            request_text: None,
            question_context: Some(
                json!({"issue_title":request.title, "signature":signature,"incident_id":id}),
            ),
        };
        if let Some(operations) = &self.bootstrap.operations {
            delivery.repository = crate::Repository {
                owner: operations.owner.clone(),
                name: operations.name.clone(),
                installation: operations.installation_id,
            };
            delivery.issue = operations.number;
        }
        commit.rows.push(self.allowance_row(delivery.clone()));
        commit.effects.push(delivery.into_effect());
        Ok(())
    }
}
