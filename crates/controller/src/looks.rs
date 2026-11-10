use crate::executor::Output as Out;
use crate::{
    Controller, Result,
    executor::{Completion, Delivery, Look, Observation, Upstream, upstream},
    hydrate, put, snapshot,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::time::{Duration, UNIX_EPOCH};
use tl3_controller_core::{looks as decisions, looks_state as state_decisions};
use tl3_kernel::{Ack, Event, Fact, NodeId, Phase, World};
use tl3_store::{Commit, EffectState, RowChange, Snapshot};

/// #759: how long every model a step may use can be out before an operations note.
const LADDER_OUT_MS: u64 = 30 * 60 * 1000;
/// #759: the operations note's fixed, typed reason.
const LADDER_OUT_REASON: &str = "model_ladder_unavailable";

mod model_quota;
pub use model_quota::QuotaModel;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct AllowanceState {
    pub delivery: Delivery,
    #[serde(default)]
    pub maintenance: bool,
    due_ms: Option<u64>,
    progress: Option<String>,
    progress_ms: u64,
    quota_ms: Option<u64>,
    /// Q53 / #828: the adapter's earliest ladder retry, distinct from a
    /// maintenance look that may run sooner to post an operations note.
    #[serde(default)]
    quota_retry_ms: Option<u64>,
    #[serde(default)]
    last_quota_ms: Option<u64>,
    #[serde(default)]
    last_quota_model: Option<String>,
    #[serde(default)]
    last_quota_family: Option<String>,
    #[serde(default)]
    last_quota_finished_ms: Option<i64>,
    #[serde(default)]
    last_quota_attempt: Option<i64>,
    #[serde(default)]
    quota_probe_at_ms: Option<u64>,
    #[serde(default)]
    quota_probe_model: Option<QuotaModel>,
    /// #759: since this step's first model ending with no model turn served
    /// since. A progress checkpoint clears it.
    #[serde(default)]
    unserved_since_ms: Option<u64>,
    /// #759: the operations note for this run of endings is posted.
    #[serde(default)]
    ladder_out_noticed: bool,
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
    /// When the last failed read was counted.
    #[serde(default)]
    counted_failure_ms: Option<u64>,
    #[serde(default)]
    external_wait: Option<tl3_store::ExternalWait>,
    #[serde(default)]
    wait_ended: bool,
}

impl AllowanceState {
    pub(crate) fn new(delivery: Delivery, now: u64, delay: u64) -> Self {
        Self {
            delivery,
            maintenance: false,
            due_ms: Some(now.saturating_add(delay)),
            progress: None,
            progress_ms: now,
            quota_ms: None,
            quota_retry_ms: None,
            last_quota_ms: None,
            last_quota_model: None,
            last_quota_family: None,
            last_quota_finished_ms: None,
            last_quota_attempt: None,
            quota_probe_at_ms: None,
            quota_probe_model: None,
            unserved_since_ms: None,
            ladder_out_noticed: false,
            stopping: false,
            incident: None,
            healthy: false,
            found: None,
            failure: None,
            read_failures: 0,
            read_incidents: 0,
            counted_failure_ms: None,
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
    /// Absent on incidents stored before Q51; see `Incident::cause`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cause: Option<Cause>,
}

/// Who caused an incident (Q51, Rob, 2026-11-01). A new TL3 release clears
/// TL3-side incidents once; upstream incidents keep the probe and Rob rules.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Cause {
    /// GitHub, Mergify, PrismaBuild or the model proxy.
    Upstream,
    /// TL3's own code, prompts, tools or harness environment.
    Tl3,
}

pub(crate) struct AttachedIncident {
    pub upstream: Upstream,
    pub class: String,
    pub diagnostic: Option<crate::publication_failure::Diagnostic>,
    pub input_failure: Option<crate::worker_inputs::Context>,
}

impl Cause {
    /// Workers and worktrees are TL3's own harness: agent runs, reply
    /// validation, Q50 blockers and local Git checkouts. A worktree failure
    /// that reaches GitHub, a Git remote or its token, is upstream. Agent runs
    /// that fail because the model proxy is down still count as TL3-side;
    /// the controller can't yet tell them from worker bugs.
    pub(crate) fn of(upstream: Upstream, class: &str) -> Self {
        if matches!(
            class,
            "worker_input_schema"
                | "redaction_infra"
                | "pr_evidence"
                | "pr_obligation"
                | "pr_policy"
        ) {
            return Cause::Tl3;
        }
        match upstream {
            Upstream::Workers => Cause::Tl3,
            Upstream::Worktrees if matches!(class, "git_remote" | "invalid_token") => {
                Cause::Upstream
            }
            Upstream::Worktrees => Cause::Tl3,
            // The trusted check harness is TL3's code, running inside a PrismaBuild action.
            Upstream::PrismaBuild if class == crate::checks::HARNESS => Cause::Tl3,
            Upstream::GitHub | Upstream::Delivery | Upstream::PrismaBuild => Cause::Upstream,
        }
    }
}

impl Incident {
    pub(crate) fn cause(&self) -> Cause {
        self.cause.unwrap_or(if self.input_failure.is_some() {
            Cause::Upstream
        } else {
            Cause::of(self.signature.upstream, &self.signature.class)
        })
    }
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
                    Upstream::Delivery,
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

fn stall_upstream(source: Upstream) -> decisions::StallUpstream {
    match source {
        Upstream::Workers => decisions::StallUpstream::Workers,
        Upstream::GitHub => decisions::StallUpstream::GitHub,
        Upstream::Delivery => decisions::StallUpstream::Delivery,
        Upstream::PrismaBuild => decisions::StallUpstream::PrismaBuild,
        Upstream::Worktrees => decisions::StallUpstream::Worktrees,
    }
}

fn incident_view(
    snapshot: &Snapshot,
    incident: &Incident,
    delivery: &Delivery,
) -> decisions::IncidentView {
    decisions::IncidentView {
        needs_rob: incident.needs_rob,
        refused_command: incident.signature.upstream == Upstream::Delivery
            && snapshot
                .rows
                .iter()
                .any(|row| row.table == "publication_failures" && row.id == delivery.key),
        own_effect: incident.effect.as_deref() == Some(&delivery.key),
    }
}

pub(crate) fn incident_needs_rob(cause: IncidentCause) -> bool {
    let cause = match cause {
        IncidentCause::Kernel { needs_rob } => decisions::IncidentCause::Kernel { needs_rob },
        IncidentCause::ReadFailure { incidents } => {
            decisions::IncidentCause::ReadFailure { incidents }
        }
        IncidentCause::NoticeFailure { incidents } => {
            decisions::IncidentCause::NoticeFailure { incidents }
        }
    };
    decisions::incident_needs_rob(cause)
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

    /// Q53 / #828: old allowances lack the ladder deadline introduced for status.
    /// Ask the scheduling adapter once on open; a #759 maintenance look may be
    /// earlier than the retry, so its due time is not a substitute for this field.
    pub(crate) fn migrate_model_quota_deadlines(&mut self) -> Result<()> {
        let snapshot = self.store.load()?;
        let now = self.now_ms();
        let mut commit = Commit::new(
            format!("quota-deadlines:{}", snapshot.revision.get()),
            json!({"migrate_model_quota_deadlines": true}),
            &snapshot,
        );
        for row in snapshot.rows.iter().filter(|row| row.table == "allowances") {
            let mut state: AllowanceState = crate::decode_stored(row.value.clone())?;
            if state.maintenance || state.quota_retry_ms.is_some() || state.last_quota_ms.is_none()
            {
                continue;
            }
            let Some(announced) = state.quota_ms else {
                continue;
            };
            state.quota_retry_ms = Some(self.adapters.adapter(&state.delivery)?.quota_resume_ms(
                &state.delivery,
                announced,
                now,
            )?);
            commit
                .rows
                .push(put("allowances", row.id.clone(), json!(state)));
        }
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
        self.release_terminal_model_probes(snapshot, commit, None)?;
        self.retire_completed_model_attempts(snapshot, commit)?;
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

    /// Only committed intents can remove an external wait subscription. A failed
    /// removal waits for its incident's clearance, without a retry polling loop.
    pub(crate) fn retire_committed_waits(&mut self, snapshot: &Snapshot) -> Result<bool> {
        for row in snapshot.rows.iter().filter(|r| r.table == "retiring_waits") {
            if let Some(id) = row.value["incident"].as_str()
                && snapshot
                    .rows
                    .iter()
                    .any(|r| r.table == "incidents" && r.id == id && r.value["open"] == true)
            {
                continue;
            }
            let delivery: Delivery = crate::decode_stored(row.value["delivery"].clone())?;
            let mut commit = Commit::new(
                format!("retire-wait:{}:{}", row.id, snapshot.revision.get()),
                json!({"retire_wait": row.id}),
                snapshot,
            );
            match self
                .adapters
                .get(Upstream::PrismaBuild)?
                .watch_wait(&delivery, None)
            {
                Ok(()) => commit.rows.push(RowChange::Delete {
                    table: row.table.clone(),
                    id: row.id.clone(),
                }),
                Err(error) => {
                    let request = crate::request(snapshot, delivery.node())?;
                    let scope = format!("retire-wait:{}", delivery.key);
                    let existing = snapshot
                        .rows
                        .iter()
                        .find(|r| {
                            r.table == "incidents"
                                && r.value["open"] == true
                                && r.value["signature"]["upstream"] == json!(Upstream::PrismaBuild)
                                && r.value["signature"]["class"] == failure_class(&error)
                                && r.value["input_failure"]["scope"] == scope
                        })
                        .map(|r| r.id.clone());
                    self.input_failed(
                        snapshot,
                        &scope,
                        Some(crate::input_errors::IssueTarget {
                            repository: request.repo,
                            issue: request.issue,
                        }),
                        &request.title,
                        Some(delivery.node()),
                        Upstream::PrismaBuild,
                        error,
                        &mut commit,
                    )?;
                    let id = commit
                        .rows
                        .iter()
                        .find_map(|r| match r {
                            RowChange::Put { table, id, .. } if table == "incidents" => {
                                Some(id.clone())
                            }
                            _ => None,
                        })
                        .or(existing)
                        .ok_or(crate::Error::Invalid("wait retirement incident missing"))?;
                    let mut value = row.value.clone();
                    value["incident"] = json!(id);
                    commit
                        .rows
                        .push(put("retiring_waits", row.id.clone(), value));
                }
            }
            self.store.commit(&commit)?;
            return Ok(true);
        }
        Ok(false)
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
            Upstream::PrismaBuild => &self.allowances.checks,
            Upstream::GitHub | Upstream::Delivery => &self.allowances.github_delivery,
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
        let maintenance = self.adapters.next_cleanup_maintenance_ms(now);
        let errors = self.adapters.cleanup_maintenance_errors();
        if let Err(error) = self.record_cleanup_maintenance(errors) {
            eprintln!("cannot persist cleanup maintenance timer incidents: {error}");
        }
        let snapshot = self.store.load()?;
        // Same-second priority rereads share the timer (Q39).
        let reread = crate::priority::next_reread_ms(&snapshot)?;
        Ok(snapshot
            .rows
            .iter()
            .filter(|r| r.table == "allowances")
            .map(|r| crate::decode_stored::<AllowanceState>(r.value.clone()))
            .collect::<std::result::Result<Vec<_>, _>>()?
            .iter()
            .filter_map(|s| s.due_ms)
            .chain(reread)
            .chain(crate::memory::next_probe_ms(&snapshot)?)
            .chain(maintenance)
            .min()
            .map(|due| Duration::from_millis(due.saturating_sub(now))))
    }

    fn look_cleanup_maintenance(&mut self) -> Result<()> {
        let now = self.now_ms();
        let pending = self.adapters.cleanup_maintenance(now);
        self.record_cleanup_maintenance(pending)
    }

    fn record_cleanup_maintenance(
        &mut self,
        pending: crate::executor::CleanupMaintenance,
    ) -> Result<()> {
        let snapshot = self.store.load()?;
        let mut commit = Commit::new(
            format!("cleanup-maintenance:{}", snapshot.revision.get()),
            json!({"cleanup_maintenance":true}),
            &snapshot,
        );
        for (upstream, report) in pending {
            let error = report.as_ref().err().cloned();
            let report = report.unwrap_or_else(|_| crate::run_backend::MaintenanceReport {
                pending: vec!["adapter-maintenance".into()],
                skipped_busy: false,
                unobserved: vec![],
            });
            let owners = report.pending;
            let outstanding: std::collections::BTreeSet<_> = owners
                .iter()
                .chain(report.unobserved.iter())
                .map(|owner| {
                    format!(
                        "runtime-cleanup:{}",
                        crate::run_backend::durable::digest(
                            format!("{upstream:?}:{owner}").as_bytes()
                        )
                    )
                })
                .collect();
            for row in snapshot.rows.iter().filter(|row| row.table == "incidents") {
                let mut incident: Incident = crate::decode_stored(row.value.clone())?;
                if incident.signature.upstream == upstream
                    && incident.signature.class == "runtime_cleanup"
                    && incident.open
                    && error.is_none()
                    && !outstanding.contains(&incident.id)
                {
                    incident.open = false;
                    commit
                        .rows
                        .push(put("incidents", incident.id.clone(), json!(incident)));
                }
            }
            for owner in owners {
                let id = format!(
                    "runtime-cleanup:{}",
                    crate::run_backend::durable::digest(format!("{upstream:?}:{owner}").as_bytes())
                );
                let incident = json!(Incident {
                    id: id.clone(),
                    node: None,
                    input_failure: None,
                    open: true,
                    needs_rob: false,
                    blocking: false,
                    summary: match &error {
                        Some(error) => format!(
                            "Maintenance for {upstream:?} failed: {error}. Other Looks continue; retry in 300 seconds."
                        ),
                        None if report.unobserved.contains(&owner) => format!(
                            "Runtime maintenance for {owner} is blocked by a persistently held owner lock. Cleanup remains unproven; retry in 300 seconds."
                        ),
                        None => format!(
                            "Runtime maintenance for {owner} needs attention. Cleanup debt remains until positive stop evidence; inspect the durable maintenance-errors journal for failed cleanup or held dispatch."
                        ),
                    },
                    signature: IncidentSignature {
                        upstream,
                        class: "runtime_cleanup".into()
                    },
                    effect: None,
                    post_key: None,
                    cause: Some(Cause::Upstream),
                });
                if !snapshot
                    .rows
                    .iter()
                    .any(|row| row.table == "incidents" && row.id == id && row.value == incident)
                {
                    commit.rows.push(put("incidents", id, incident));
                }
            }
        }
        if !commit.rows.is_empty() {
            self.store.commit(&commit)?;
        }
        Ok(())
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
        if let Err(error) = self.look_cleanup_maintenance() {
            eprintln!("cannot persist cleanup maintenance incidents: {error}");
        }
        self.look_memory_probe(force, key)?;
        self.retry_input_incident_posts()?;
        let mut snapshot = self.store.load()?;
        // Same-second priority rereads that are due share this timer (Q39).
        if key.is_none() && self.reconcile_due(&snapshot)? {
            snapshot = self.store.load()?;
        }
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
        if state.maintenance {
            let incident = state.incident.as_ref().and_then(|id| {
                snapshot
                    .rows
                    .iter()
                    .find(|r| r.table == "incidents" && r.id == *id && r.value["open"] == true)
            });
            if let Some(row) = incident {
                let incident: Incident = crate::decode_stored(row.value.clone())?;
                if decisions::incident_action(
                    true,
                    incident_view(snapshot, &incident, &state.delivery),
                ) == decisions::IncidentAction::Probe
                {
                    match self
                        .adapters
                        .adapter(&state.delivery)?
                        .probe(&state.delivery)
                    {
                        Ok(true) => self.probe_passed(Some(state), state.incident.as_deref())?,
                        Ok(false) | Err(_) => self.rearm(state),
                    }
                }
            }
            self.save_allowance(state)?;
            return Ok(());
        }
        if let Some(id) = &state.incident
            && let Some(row) = snapshot
                .rows
                .iter()
                .find(|r| r.table == "incidents" && r.id == *id && r.value["open"] == true)
        {
            let incident: Incident = crate::decode_stored(row.value.clone())?;
            match decisions::incident_action(
                false,
                incident_view(snapshot, &incident, &state.delivery),
            ) {
                decisions::IncidentAction::Hold => {
                    // A healthy GET cannot prove command restrictions were fixed (#608).
                    self.save_allowance(state)?;
                    return Ok(());
                }
                decisions::IncidentAction::Probe => {
                    match self
                        .adapters
                        .adapter(&state.delivery)?
                        .probe(&state.delivery)
                    {
                        Ok(true) => {
                            self.probe_passed(Some(state), state.incident.as_deref())?;
                            self.retry_held_effects(upstream(state.delivery.output));
                            self.rearm(state);
                        }
                        Ok(false) => self.rearm(state),
                        Err(error) => self.look_failed(state, error)?,
                    }
                    self.save_allowance(state)?;
                    return Ok(());
                }
                decisions::IncidentAction::Continue => {}
            }
        }
        if let Some(announced) = state.quota_ms {
            self.expire_model_quotas(now)?;
            let resume = self.adapters.adapter(&state.delivery)?.quota_resume_ms(
                &state.delivery,
                announced,
                now,
            )?;
            state.quota_retry_ms = Some(resume);
            let Some(retry) = self.quota_retry(state, resume, now)? else {
                state.due_ms = None;
                self.save_allowance(state)?;
                return Ok(());
            };
            state.quota_retry_ms = Some(retry);
            match state_decisions::quota_wait(
                retry,
                now,
                state.unserved_since_ms,
                state.ladder_out_noticed,
                LADDER_OUT_MS,
            ) {
                state_decisions::QuotaWait::Resume => {}
                state_decisions::QuotaWait::Notify { due } => {
                    state.ladder_out_noticed = true;
                    state.due_ms = Some(due);
                    return self.ladder_out_note(state, retry, now);
                }
                state_decisions::QuotaWait::Wait { due } => {
                    state.due_ms = Some(due);
                    self.save_allowance(state)?;
                    return Ok(());
                }
            }
        }
        if state_decisions::entry_action(state.quota_ms.is_some(), state.stopping)
            == state_decisions::EntryAction::ResumeQuota
        {
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
                        state.found = None;
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
            state.quota_retry_ms = None;

            self.remove_hold(&state.delivery);
            self.save_allowance(state)?;
            return Ok(());
        }
        if state_decisions::entry_action(state.quota_ms.is_some(), state.stopping)
            == state_decisions::EntryAction::FinishStall
        {
            self.finish_stall(state)?;
            return Ok(());
        }
        let result = self
            .adapters
            .adapter(&state.delivery)?
            .look(&state.delivery);
        let result = match result {
            Err(error)
                if state_decisions::missing_is_absent(
                    error.is_github_not_found(),
                    state.failure.as_deref() == Some("not_found"),
                    matches!(state.delivery.output, Out::Effect { .. }),
                ) =>
            {
                Ok(Look::Absent)
            }
            result => result,
        };
        let read =
            state_decisions::read_decision(look_kind(&result), state.external_wait.is_some());
        if read.reset_failures {
            state.read_failures = 0;
        }
        if read.clear_wait {
            self.adapters
                .get(Upstream::PrismaBuild)?
                .watch_wait(&state.delivery, None)?;
            state.external_wait = None;
            state.wait_ended = false;
        }
        match result {
            Ok(Look::Found(completion)) => {
                let completion = *completion;
                let effect = self
                    .store
                    .effect(&state.delivery.key)?
                    .expect("allowance effect");
                let found = serde_json::to_string(&completion)?;
                let action = state_decisions::found_action(
                    effect.state.is_pending(),
                    false,
                    state.found.as_ref() != Some(&found),
                    !completion.observations.is_empty(),
                );
                state.found = Some(found);
                if action != state_decisions::FoundAction::Skip {
                    if completion.releases_holds() {
                        self.retry_held_effects(upstream(state.delivery.output));
                    }
                    if action == state_decisions::FoundAction::Complete {
                        self.complete(&effect.key, completion)?;
                    } else {
                        // The final save must retain the class recorded with the observations.
                        if let Some(class) = &completion.failure_class {
                            state.failure = Some(class.clone());
                        }
                        let refusal = completion.ack == Ack::DeliveryAccepted
                            && completion.diagnostic.is_some();
                        self.look_observations(&state.delivery, completion)?;
                        if refusal {
                            state.incident = self
                                .store
                                .load()?
                                .rows
                                .iter()
                                .find(|row| {
                                    row.table == "allowances" && row.id == state.delivery.key
                                })
                                .and_then(|row| row.value["incident"].as_str().map(str::to_owned));
                        }
                    }
                }
            }
            Ok(Look::Absent) => {
                let effect = self
                    .store
                    .effect(&state.delivery.key)?
                    .expect("allowance effect");
                let action = if self.hold_class(&state.delivery) == Some(CHECKS_PENDING) {
                    state_decisions::absence_action(true, false, false, false, false, false)
                } else {
                    state_decisions::absence_action(
                        false,
                        self.is_held(&state.delivery),
                        state.failure.as_deref() == Some("not_found"),
                        effect.state.is_pending(),
                        effect.state == EffectState::Cancelling,
                        matches!(state.delivery.output, Out::Effect { .. }),
                    )
                };
                if action == state_decisions::AbsenceAction::ReleaseHold {
                    self.remove_hold(&state.delivery);
                } else if action == state_decisions::AbsenceAction::Retry {
                    // Absence or a repeated 404 spends the existing retry allowance.
                    self.complete(
                        &effect.key,
                        Completion {
                            diagnostic: None,
                            failure_class: None,
                            ack: Ack::Retryable,
                            observations: vec![],
                        },
                    )?;
                    return Ok(());
                }
            }
            Ok(Look::Alive { progress }) => {
                let changed = state.progress.as_ref() != Some(&progress);
                // An unacknowledged effect that stopped moving past its allowance
                // keeps its failure count, so the stall can escalate (#356).
                // Inside the allowance an alive look is a successful read (#373).
                if state_decisions::alive_resets_early(
                    changed,
                    now,
                    state.progress_ms,
                    u64::from(self.allowance(&state.delivery).no_progress_secs) * 1000,
                ) || !self
                    .store
                    .effect(&state.delivery.key)?
                    .is_some_and(|effect| effect.state.is_pending())
                {
                    state.read_failures = 0;
                }
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
                let record = if wait.upstream == "flux" {
                    self.adapters
                        .get(Upstream::PrismaBuild)?
                        .wait_record(&wait.key, &checkout)
                } else {
                    Ok(crate::executor::WaitRecord::Ended)
                };
                match state_decisions::wait_action(wait_status(&record), state.wait_ended) {
                    state_decisions::WaitAction::WatchAlive => {
                        self.adapters
                            .get(Upstream::PrismaBuild)?
                            .watch_wait(&state.delivery, Some(&wait))?;
                        state.read_failures = 0;
                        state.wait_ended = false;
                        state.progress = None;
                    }
                    state_decisions::WaitAction::WatchEnded { start_clock } => {
                        self.adapters
                            .get(Upstream::PrismaBuild)?
                            .watch_wait(&state.delivery, None)?;
                        state.read_failures = 0;
                        if start_clock {
                            state.wait_ended = true;
                            state.progress_ms = now;
                        }
                        if self.look_alive(state, false)? {
                            return Ok(());
                        }
                    }
                    state_decisions::WaitAction::Fail => {
                        self.look_failed(state, record.err().expect("failed wait record"))?
                    }
                }
            }
            Ok(Look::Healthy) => {
                if !state.healthy {
                    self.retry_held_effects(upstream(state.delivery.output));
                }
                state.healthy = true;
                self.probe_passed(Some(state), state.incident.as_deref())?;
            }
            Ok(Look::Quota { reset_ms }) => {
                state.quota_ms = Some(reset_ms);
                state.due_ms = Some(state_decisions::quota_due(reset_ms, now));
            }
            Ok(Look::Pending) => {
                self.look_failed(state, crate::Error::Invalid("upstream record unavailable"))?;
            }
            Err(crate::Error::Quota { retry_after }) => {
                let reset =
                    now.saturating_add(retry_after.as_millis().min(u64::MAX as u128) as u64);
                state.quota_ms = Some(reset);
                state.due_ms = Some(state_decisions::quota_due(reset, now));
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
                if decisions::probe_allowed(true, incident.needs_rob) {
                    match self
                        .adapters
                        .adapter(&state.delivery)?
                        .probe(&state.delivery)
                    {
                        Ok(true) => {
                            state.healthy = true;
                            self.retry_held_effects(upstream(state.delivery.output));
                            self.probe_passed(Some(state), state.incident.as_deref())?;
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

    fn look_alive(&mut self, state: &mut AllowanceState, changed: bool) -> Result<bool> {
        let now = self.now_ms();
        let allowance = self.allowance(&state.delivery).clone();
        if changed {
            state.progress_ms = now;
            self.retry_held_effects(upstream(state.delivery.output));
        } else if decisions::stalled(
            now,
            state.progress_ms,
            u64::from(allowance.no_progress_secs) * 1000,
        ) {
            let action = decisions::stall_action(stall_upstream(upstream(state.delivery.output)));
            match action {
                decisions::StallAction::StopWorker => {
                    state.stopping = true;
                    state.failure = Some("no_progress".into());
                    self.save_allowance(state)?;
                    self.finish_stall(state)?;
                    return Ok(true);
                }
                decisions::StallAction::Unfinished => {
                    self.look_failed(
                        state,
                        crate::Error::Permanent {
                            class: "unfinished",
                            source: "the upstream never acknowledged the request".into(),
                        },
                    )?;
                }
                // PrismaBuild owns progress. Worktrees has no progress contract.
                decisions::StallAction::Ignore => {}
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
            Ok(completion) if delivery.is_postmortem() => crate::postmortems::stalled(completion),
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
        state.due_ms = Some(decisions::rearm_at(
            self.now_ms(),
            u64::from(self.allowance(&state.delivery).silence_secs) * 1000,
        ));
    }

    fn look_failed(&mut self, state: &mut AllowanceState, error: crate::Error) -> Result<()> {
        if !crate::input_errors::recoverable(&error) {
            return Err(error);
        }
        self.record_publication_refusal(&state.delivery, &error)?;
        eprintln!(
            "allowance action failed for {}: {error}",
            state.delivery.key
        );
        if let crate::Error::Quota { retry_after } = &error {
            let reset = decisions::rearm_at(
                self.now_ms(),
                retry_after.as_millis().min(u64::MAX as u128) as u64,
            );
            state.quota_ms = Some(reset);
            state.due_ms = Some(reset);
            return Ok(());
        }
        state.failure = Some(failure_class(&error).into());
        state.healthy = false;
        self.rearm(state);
        // #403: a postmortem raises no incident; nothing waits on it.
        if !decisions::incident_for_operation(state.delivery.is_postmortem()) {
            return Ok(());
        }
        let snapshot = self.store.load()?;
        if !decisions::incident_allows_failure(state.incident.as_ref().is_some_and(|id| {
            snapshot
                .rows
                .iter()
                .any(|r| r.table == "incidents" && r.id == *id && r.value["open"] == true)
        })) {
            return Ok(());
        }
        if !decisions::incident_for_notice(matches!(
            state.delivery.output,
            Out::Notice {
                notice: tl3_kernel::Notice::Incident { .. },
                ..
            }
        )) {
            return Ok(());
        }
        // #356: looks woken by webhooks can land seconds apart. Failed reads
        // within one allowance count once, so escalation keeps the allowance's pace.
        let now = self.now_ms();
        let period = u64::from(self.allowance(&state.delivery).silence_secs) * 1000;
        if !decisions::count_failure(state.counted_failure_ms, now, period) {
            return Ok(());
        }
        state.counted_failure_ms = Some(now);
        let mut count = decisions::FailureCount {
            failures: state.read_failures,
            incidents: state.read_incidents,
            raise: false,
        };
        decisions::record_failure(&mut count);
        state.read_failures = count.failures;
        if !count.raise {
            return Ok(());
        }
        state.read_incidents = count.incidents;
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
        let mut summary =
            incident_summary(&request.title, &signature, needs_rob, app_login.as_deref());
        crate::worker_inputs::append(&mut summary, crate::worker_inputs::of(&error));
        let mut commit = Commit::new(
            format!("incident:{id}"),
            json!({"read_outage":state.delivery.key}),
            &snapshot,
        );
        let diagnostic =
            crate::publication_failure::retained(&snapshot, &commit, &state.delivery.key)?;
        if let Some(diagnostic) = &diagnostic {
            summary.push_str(&format!(
                " Recorded occurrence {}. {}",
                state.read_incidents,
                diagnostic.render()
            ));
        }
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
                cause: Some(Cause::of(signature.upstream, &signature.class)),
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
                request.repo.owner, request.repo.name, request.issue), "signature":signature, "incident_id":id,
            "publication_diagnostic":diagnostic, "occurrences":state.read_incidents}));
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

    /// Durable delivery responsibility survives a permanent upstream refusal.
    /// Retain its incident beside acceptance, so later wakes cannot issue another PUT.
    pub(crate) fn accepted_delivery_refusal(
        &self,
        snapshot: &Snapshot,
        delivery: &Delivery,
        completion: &Completion,
        commit: &mut Commit,
    ) -> Result<()> {
        let Some(diagnostic) = &completion.diagnostic else {
            return Ok(());
        };
        if completion.ack != Ack::DeliveryAccepted
            || !matches!(
                delivery.output,
                Out::Effect {
                    kind: tl3_kernel::EffectKind::Enqueue,
                    ..
                }
            )
        {
            return Ok(());
        }
        let world = snapshot::decode(snapshot)?;
        let node = &world.nodes[delivery.node().0 as usize];
        let publishing = node.phase == Phase::Publishing
            && node.generation == delivery.generation
            && matches!((node.effect, delivery.output),
                (Some(effect), Out::Effect {seq, kind:tl3_kernel::EffectKind::Enqueue, ..})
                    if effect.seq == seq && effect.kind == tl3_kernel::EffectKind::Enqueue && !effect.cancelling);
        if !(publishing
            || node.operation_pending(tl3_kernel::EffectKind::Enqueue, delivery.generation))
            || node.effect.is_some_and(|effect| effect.cancelling)
            || snapshot.rows.iter().any(|row| {
                row.table == "incidents"
                    && row.value["open"] == true
                    && row.value["effect"] == delivery.key
            })
        {
            return Ok(());
        }
        let id = incident_id(snapshot, "delivery");
        let class = completion
            .failure_class
            .as_deref()
            .unwrap_or("direct_merge_refused");
        let signature = IncidentSignature {
            upstream: Upstream::Delivery,
            class: class.into(),
        };
        let request = crate::request(snapshot, delivery.node())?;
        let summary = format!(
            "{} {}",
            incident_summary(&request.title, &signature, false, None),
            diagnostic.render()
        );
        let mut post = delivery.clone();
        post.key = format!("incident:{id}");
        post.output = Out::Notice {
            node: delivery.node(),
            notice: tl3_kernel::Notice::Incident { needs_rob: false },
        };
        post.epoch = snapshot.epoch.get();
        post.note = Some(incident_note(&request.repo, request.issue, &summary));
        post.question_context = Some(json!({"issue_title":request.title,"signature":signature,
            "incident_id":id,"publication_diagnostic":diagnostic}));
        if let Some(o) = &self.bootstrap.operations {
            post.repository = crate::Repository {
                owner: o.owner.clone(),
                name: o.name.clone(),
                installation: o.installation_id,
            };
            post.issue = o.number;
        }
        commit.rows.push(put(
            "publication_failures",
            delivery.key.clone(),
            json!(diagnostic),
        ));
        commit.rows.push(put(
            "incidents",
            id.clone(),
            json!(Incident {
                id: id.clone(),
                node: Some(delivery.node()),
                input_failure: None,
                open: true,
                needs_rob: false,
                blocking: false,
                summary,
                signature: signature.clone(),
                effect: Some(delivery.key.clone()),
                post_key: Some(post.key.clone()),
                cause: Some(Cause::of(signature.upstream, class)),
            }),
        ));
        if let Some(row) = snapshot
            .rows
            .iter()
            .find(|row| row.table == "allowances" && row.id == delivery.key)
        {
            let mut state: AllowanceState = crate::decode_stored(row.value.clone())?;
            state.incident = Some(id);
            state.failure = Some(class.into());
            state.healthy = false;
            self.rearm(&mut state);
            commit
                .rows
                .push(put("allowances", delivery.key.clone(), json!(state)));
        }
        commit.rows.push(self.allowance_row(post.clone()));
        commit.effects.push(post.into_effect());
        Ok(())
    }

    pub(crate) fn look_observations(
        &mut self,
        delivery: &Delivery,
        completion: Completion,
    ) -> Result<()> {
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
        self.release_terminal_model_probes(&snapshot, &mut commit, None)?;
        self.retire_completed_model_attempts(&snapshot, &mut commit)?;
        if let Some(class) = completion.failure_class.as_deref() {
            self.failure_allowance(&snapshot, &delivery.key, class, &mut commit)?;
        }
        self.accepted_delivery_refusal(&snapshot, delivery, &completion, &mut commit)?;
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

    pub(crate) fn probe_passed(
        &mut self,
        state: Option<&AllowanceState>,
        id: Option<&str>,
    ) -> Result<()> {
        let Some(id) = id else {
            return Ok(());
        };
        let snapshot = self.store.load()?;
        let Some(row) = snapshot
            .rows
            .iter()
            .find(|r| r.table == "incidents" && r.id == id)
        else {
            return Ok(());
        };
        let mut incident: Incident = crate::decode_stored(row.value.clone())?;
        if !decisions::probe_allowed(incident.open, incident.needs_rob) {
            return Ok(());
        }
        if incident.effect.is_some() || state.is_none() {
            incident.open = false;
            let mut commit =
                Commit::new(format!("probe:{id}"), json!({"probe_passed":id}), &snapshot);
            self.retry_notice(&snapshot, &incident, &mut commit)?;
            commit
                .rows
                .push(put("incidents", id.to_owned(), json!(incident)));
            self.store.commit(&commit)?;
            return Ok(());
        }
        let state = state.expect("kernel incident has an allowance");
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

    pub(crate) fn clear_model_quota(
        &self,
        snapshot: &Snapshot,
        model: Option<&str>,
        commit: &mut Commit,
    ) {
        if let Some(row) = snapshot
            .rows
            .iter()
            .find(|row| row.table == "model_quotas" && Some(row.id.as_str()) == model)
        {
            commit.rows.push(RowChange::Delete {
                table: row.table.clone(),
                id: row.id.clone(),
            });
        }
    }

    /// #828 / Q53: operator reset evidence ends this model's hold atomically.
    pub(crate) fn operator_clear_quota(
        &mut self,
        snapshot: &Snapshot,
        model: &str,
        now: u64,
        commit: &mut Commit,
    ) -> Result<()> {
        self.clear_model_quota(snapshot, Some(model), commit);
        for row in snapshot.rows.iter().filter(|row| row.table == "allowances") {
            let mut state: AllowanceState = crate::decode_stored(row.value.clone())?;
            if state.quota_ms.is_some()
                && self
                    .adapters
                    .adapter(&state.delivery)?
                    .quota_model_eligible(&state.delivery, model)?
            {
                // Keep the quota-resume path, but do not reconstruct the old reset
                // from the worker's ending when quota_resume_ms next reads it.
                state.quota_ms = Some(now);
                state.quota_retry_ms = Some(now);
                // Operator evidence ends this allowance's bounded-hint regime.
                // Keep any live probe's shared lease until its actual ending.
                state.quota_probe_at_ms = None;
                state.quota_probe_model = None;
                state.due_ms = Some(now);
                commit
                    .rows
                    .push(put("allowances", row.id.clone(), json!(state)));
            }
        }
        Ok(())
    }

    pub(crate) fn quota_clear_reply(
        &self,
        snapshot: &Snapshot,
        repository: &crate::Repository,
        issue: u64,
        comment: u64,
        line: String,
        commit: &mut Commit,
    ) {
        let delivery = Delivery {
            key: format!("model-quota-clear-reply:{comment}"),
            epoch: snapshot.epoch.get(),
            output: Out::Comment {
                node: NodeId(0),
                kind: crate::executor::CommentKind::OperationsNote,
            },
            generation: 0,
            head_sha: None,
            repository: repository.clone(),
            issue,
            pull_request: None,
            proposed: None,
            paths: Default::default(),
            attempt: 1,
            round: 0,
            replanned: false,
            checks: None,
            job_generation: None,
            question: None,
            note: Some(line.clone()),
            scope_removals: None,
            marker_key: None,
            question_context: None,
            steering: Some(crate::steering::Task::ConsoleNote { text: line }),
            request_text: None,
            evidence: None,
        };
        commit.rows.push(self.allowance_row(delivery.clone()));
        commit.effects.push(delivery.into_effect());
    }

    pub(crate) fn quota_allowance(
        &mut self,
        snapshot: &Snapshot,
        key: &str,
        reset_ms: u64,
        model: Option<&str>,
        commit: &mut Commit,
    ) -> Result<()> {
        let ending = self.store.latest_worker_quota(key, model)?;
        let finished_ms = ending
            .as_ref()
            .filter(|e| e.facts.outcome == "quota" && Some(e.facts.model.as_str()) == model)
            .and_then(|e| e.facts.finished_ms);
        let attempt = ending.as_ref().map(|e| e.attempt);
        let family = model.and_then(|model| {
            ending
                .as_ref()
                .and_then(|ending| ending.family.clone())
                .or_else(|| {
                    attempt.and_then(|attempt| self.attempt_family(snapshot, key, attempt, model))
                })
        });
        if let Some(attempt) = attempt {
            self.release_model_probe(snapshot, key, Some(attempt), commit)?;
        }
        let cleared = model.zip(family.as_deref()).zip(finished_ms).is_some_and(
            |((model, family), finished_ms)| {
                model_quota::success_after(snapshot, model, family, finished_ms)
            },
        );
        // Q53 / #828: maximum reset and latest ending are independent facts.
        // Equal (or shorter) hints must still advance the availability evidence.
        if let Some(model) = model
            && !cleared
        {
            let old = snapshot
                .rows
                .iter()
                .find(|row| row.table == "model_quotas" && row.id == model);
            let mut value = old.map_or_else(|| json!({}), |row| row.value.clone());
            let previous = value["reset_ms"].as_u64();
            let reset = if state_decisions::record_quota(previous, reset_ms) {
                reset_ms
            } else {
                previous.expect("existing reset")
            };
            let old_finished = value["finished_ms"].as_i64();
            // A legacy shared row could come from a different call. Replaying
            // this ending cannot establish that row's missing identity/order.
            if old.is_none()
                || old_finished.is_some()
                    && value["family"].as_str().is_some()
                    && finished_ms > old_finished
            {
                value["family"] = json!(family);
                value["finished_ms"] = json!(finished_ms);
            }
            value["reset_ms"] = json!(reset);
            commit.rows.push(put("model_quotas", model.into(), value));
        }
        if let Some(row) = snapshot
            .rows
            .iter()
            .find(|r| r.table == "allowances" && r.id == key)
        {
            let mut state: AllowanceState = crate::decode_stored(row.value.clone())?;
            // A replayed ending of the same model never restarts its wait.
            if (attempt.is_some() && state.last_quota_attempt == attempt
                || attempt.is_none()
                    && state_decisions::quota_replayed(state.last_quota_ms, true, reset_ms))
                && state.last_quota_model.as_deref() == model
            {
                return Ok(());
            }
            state.last_quota_ms = Some(reset_ms);
            state.last_quota_model = model.map(str::to_owned);
            state.last_quota_family = family;
            state.last_quota_finished_ms = finished_ms;
            state.last_quota_attempt = attempt;
            state.quota_ms = Some(reset_ms);
            state.quota_retry_ms = None;

            state.unserved_since_ms.get_or_insert(self.now_ms());
            // The look asks the adapter whether another model can take the step now.
            state.due_ms = Some(self.now_ms());
            commit
                .rows
                .push(put("allowances", key.into(), json!(state)));
        }
        Ok(())
    }

    /// A model's quota ends at its announced reset; later steps may use it again.
    fn expire_model_quotas(&mut self, now: u64) -> Result<()> {
        let snapshot = self.store.load()?;
        let mut commit = Commit::new(
            format!("model-quota-reset:{}", snapshot.revision.get()),
            json!({"model_quota_reset": now}),
            &snapshot,
        );
        for row in snapshot.rows.iter().filter(|row| {
            row.table == "model_quotas" && row.value["reset_ms"].as_u64().is_none_or(|r| r <= now)
        }) {
            commit.rows.push(RowChange::Delete {
                table: row.table.clone(),
                id: row.id.clone(),
            });
        }
        if !commit.rows.is_empty() {
            self.store.commit(&commit)?;
        }
        Ok(())
    }

    /// #759: one operations note that every model this step may use has been
    /// out for LADDER_OUT_MS, in the commit that marks it posted. Its reason
    /// is the fixed `model_ladder_unavailable`; the text names only config
    /// model names, the request and the wait's end.
    fn ladder_out_note(&mut self, state: &AllowanceState, resume: u64, now: u64) -> Result<()> {
        let snapshot = self.store.load()?;
        if ended_effect_look(&snapshot, &snapshot::decode(&snapshot)?, &state.delivery) {
            return Ok(());
        }
        let since = state.unserved_since_ms.unwrap_or(now);
        let mut commit = Commit::new(
            format!("model-ladder-out:{}:{since}", state.delivery.key),
            json!({"model_ladder_out": state.delivery.key, "since_ms": since}),
            &snapshot,
        );
        commit
            .rows
            .push(put("allowances", state.delivery.key.clone(), json!(state)));
        if let Some(operations) = &self.bootstrap.operations {
            let node = state.delivery.node();
            let request = crate::request(&snapshot, node)?;
            let role = crate::adapters::workers::protocol::delivery_role(&state.delivery)
                .unwrap_or("worker");
            let out: Vec<&str> = snapshot
                .rows
                .iter()
                .filter(|row| {
                    row.table == "model_quotas"
                        && row.value["reset_ms"].as_u64().is_some_and(|r| r > now)
                })
                .map(|row| row.id.as_str())
                .collect();
            let until = chrono::DateTime::from_timestamp_millis(resume as i64)
                .map(|at| at.format("%Y-%m-%d %H:%M:%S UTC").to_string())
                .unwrap_or_else(|| "an unknown time".into());
            let line = format!(
                "Operations notice ({LADDER_OUT_REASON}): every {role} model has been unavailable or out of quota for {} minutes. {}/{}#{} waits for the earliest probe or reset, at {until}. Models out now: {}. If every model is out, check CLIProxyAPI.",
                LADDER_OUT_MS / 60_000,
                request.repo.owner,
                request.repo.name,
                request.issue,
                out.join(", "),
            );
            let delivery = Delivery {
                key: format!("model-ladder-out:{}:{since}", state.delivery.key),
                epoch: snapshot.epoch.get(),
                output: Out::Comment {
                    node,
                    kind: crate::executor::CommentKind::OperationsNote,
                },
                generation: state.delivery.generation,
                head_sha: None,
                repository: crate::Repository {
                    owner: operations.owner.clone(),
                    name: operations.name.clone(),
                    installation: operations.installation_id,
                },
                issue: operations.number,
                pull_request: None,
                proposed: None,
                paths: Default::default(),
                attempt: state.delivery.attempt,
                round: state.delivery.round,
                replanned: state.delivery.replanned,
                checks: None,
                job_generation: None,
                question: None,
                note: Some(line.clone()),
                scope_removals: None,
                marker_key: None,
                question_context: None,
                steering: Some(crate::steering::Task::ConsoleNote { text: line }),
                request_text: None,
                evidence: None,
            };
            commit.rows.push(self.allowance_row(delivery.clone()));
            commit.effects.push(delivery.into_effect());
        }
        self.store.commit(&commit)?;
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
    ) -> Result<AttachedIncident> {
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
            let diagnostic =
                crate::publication_failure::retained(snapshot, commit, &state.delivery.key)?;
            let input_failure =
                crate::worker_inputs::retained(snapshot, commit, &state.delivery.key)?;
            commit
                .rows
                .push(put("allowances", state.delivery.key.clone(), json!(state)));
            Ok(AttachedIncident {
                upstream,
                class: state.failure.unwrap_or_else(|| "operation_failed".into()),
                diagnostic,
                input_failure,
            })
        } else {
            Ok(AttachedIncident {
                upstream: Upstream::GitHub,
                class: "operation_failed".into(),
                diagnostic: None,
                input_failure: None,
            })
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
            let maintenance_pending = state.maintenance
                && snapshot.rows.iter().any(|r| {
                    (r.table == crate::approval_delivery::REPAIRS
                        && state.delivery.key == format!("approval-repair:{}", r.id))
                        || (r.table == "github_message_reconciliations"
                            && state.delivery.key == format!("github-cleanup:{}", r.id))
                });
            // Q53 / #817: a current quota deadline covers a lost result watch.
            // Keep that durable retry until the existing quota path releases it.
            // A quota hold belongs to a kernel node's maker effect: a delivery
            // whose node the world does not hold has none.
            let quota_held = match world.nodes.get(state.delivery.node().0 as usize) {
                Some(node) => crate::dispatch_liveness::quota_held(
                    snapshot,
                    &[],
                    state.delivery.node(),
                    node,
                )?,
                None => false,
            };
            if !pending && !watched && !incident_open && !maintenance_pending && !quota_held {
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

/// Q53 / #828 and #817(c): a model-quota retry has a durable deadline and
/// keeps the original attempt's idempotency key. Missing model/family means
/// older evidence or unavailable configuration, never an inferred identity.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelQuotaHold {
    pub key: String,
    pub node: NodeId,
    pub generation: u64,
    pub role: tl3_kernel::Role,
    pub model: Option<String>,
    pub family: Option<String>,
    pub reset_ms: u64,
    pub retry_at_ms: u64,
}

/// Pure projection of the same allowance records used by `quota_waiting` and
/// `look_state`. Configuration supplies family metadata only; it never decides
/// whether an attempt is held or computes another retry deadline.
pub fn model_quota_holds(
    snapshot: &Snapshot,
    models: Option<&tl3_config::Models>,
) -> Result<Vec<ModelQuotaHold>> {
    let mut holds = Vec::new();
    for row in snapshot.rows.iter().filter(|row| row.table == "allowances") {
        let state: AllowanceState = crate::decode_stored(row.value.clone())?;
        let Out::Effect {
            kind: tl3_kernel::EffectKind::StartRun(role),
            ..
        } = state.delivery.output
        else {
            continue;
        };
        let Some(reset_ms) = state.quota_ms else {
            continue;
        };
        // Transport/API quota is a different wait, without a model ending.
        if state.last_quota_ms.is_none() {
            continue;
        }
        let ladder = models.map(|models| match role {
            tl3_kernel::Role::Ingest => &models.ingest,
            tl3_kernel::Role::Audit => &models.audit,
            tl3_kernel::Role::Plan => &models.planner,
            tl3_kernel::Role::Make => &models.maker,
            tl3_kernel::Role::Review => &models.reviewer,
        });
        let family = state.last_quota_family.or_else(|| {
            ladder
                .and_then(|ladder| {
                    ladder
                        .iter()
                        .find(|model| Some(&model.name) == state.last_quota_model.as_ref())
                })
                .map(|model| model.family.clone())
        });
        holds.push(ModelQuotaHold {
            node: state.delivery.node(),
            key: state.delivery.key,
            generation: state.delivery.generation,
            role,
            model: state.last_quota_model,
            family,
            reset_ms,
            retry_at_ms: state.quota_retry_ms.unwrap_or(reset_ms),
        });
    }
    Ok(holds)
}

impl ModelQuotaHold {
    pub(crate) fn waiting_on(&self) -> String {
        let role = match self.role {
            tl3_kernel::Role::Ingest => "ingest",
            tl3_kernel::Role::Audit => "audit",
            tl3_kernel::Role::Plan => "planner",
            tl3_kernel::Role::Make => "maker",
            tl3_kernel::Role::Review => "reviewer",
        };
        let at = |ms| {
            i64::try_from(ms)
                .ok()
                .and_then(chrono::DateTime::from_timestamp_millis)
                .map(|at| at.to_rfc3339_opts(chrono::SecondsFormat::AutoSi, true))
                .unwrap_or_else(|| format!("{ms}ms since epoch"))
        };
        format!(
            "model quota ({role}, {}) until {}; earliest retry {}",
            self.model.as_deref().unwrap_or("model not recorded"),
            at(self.reset_ms),
            at(self.retry_at_ms)
        )
    }
}

/// The hold class of an enqueue that is waiting for required checks.
pub(crate) const CHECKS_PENDING: &str = "checks_pending";

pub(crate) fn failure_class(error: &crate::Error) -> &'static str {
    match error {
        crate::Error::CounterExhausted(_) => crate::input_errors::COUNTER_HEADROOM,
        crate::Error::InvalidKernelResult => crate::input_errors::INVALID_RESULT,
        crate::Error::WorkerInputs(_) => crate::worker_inputs::CLASS,
        crate::Error::WorkerDispatchMissing => crate::dispatch_liveness::MISSING,
        crate::Error::PrerequisiteNumber => "prerequisite_number",
        crate::Error::ChecksPending => CHECKS_PENDING,
        crate::Error::ApprovalAmbiguous => crate::approval_delivery::AMBIGUOUS,
        crate::Error::RedactionInfra(_) => "redaction_infra",
        crate::Error::Quota { .. } => "quota",
        crate::Error::Permanent { class, .. } => class,
        crate::Error::Io(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
            "permission"
        }
        crate::Error::Io(_) => "transport",
        crate::Error::Adapter(error) => match error.downcast_ref::<tl3_worktrees::Error>() {
            // Git's own message showed a network or remote cause.
            Some(tl3_worktrees::Error::Network { .. }) => "git_remote",
            _ => error
                .downcast_ref::<tl3_github::Error>()
                .map_or("transport", |error| error.failure_class().as_str()),
        },
        crate::Error::MessageRejected => "message_rejected",
        _ => "setup",
    }
}

pub(crate) fn outage_waiting(snapshot: &Snapshot, key: &str) -> bool {
    snapshot.rows.iter().any(|r| {
        r.table == "incidents"
            && r.value["open"] == true
            && r.value["effect"].as_str() == Some(key)
            && r.value["signature"]["class"] != crate::approval_delivery::AMBIGUOUS
    })
}

pub(crate) fn incident_summary(
    title: &str,
    signature: &IncidentSignature,
    needs_rob: bool,
    _app_login: Option<&str>,
) -> String {
    let source = match signature.upstream {
        _ if signature.class == crate::worker_inputs::CLASS => "TL3's worker input builder",
        _ if matches!(
            signature.class.as_str(),
            "pr_evidence" | "pr_obligation" | "pr_policy"
        ) =>
        {
            "TL3's publication validation"
        }
        _ if signature.class == crate::checks::HARNESS => "TL3's check harness",
        Upstream::Workers => "the agent worker",
        Upstream::Worktrees => "the Git worktree",
        Upstream::PrismaBuild => "PrismaBuild",
        Upstream::Delivery => "Delivery",
        Upstream::GitHub => "GitHub",
    };
    let reason = match signature.class.as_str() {
        "worker_input_schema" => {
            "could not start because TL3 built inputs that violate the role schema"
        }
        "pr_evidence" => "refused publication because required independent evidence is missing",
        "pr_obligation" => "refused publication because a branch or body obligation is unmet",
        "pr_policy" => "refused publication because the body breaks repository policy",
        "no_progress" => "stopped making progress",
        "permission" => "refused access",
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
        "inconsistent_read" => "kept changing while TL3 read it",
        "mergeability_pending" => "has not yet worked out whether the pull request can merge",
        "head_changed" => "shows a different head from the approved one",
        "pull_request_closed" => "shows the pull request closed or merged",
        "upstream_server" => "reported a service error",
        "invalid_response" => "returned an unreadable response",
        "quota" => "reached its usage allowance",
        "message_rejected" => "could not prepare a readable message",
        "prerequisite_number" => {
            "sent a merge prerequisite update missing its issue or pull request number"
        }
        "setup" => "could not complete its setup",
        "worker_dispatch_missing" => {
            "has no tracked worker run. Comment restart on the request's issue to start it again"
        }
        "environment_blocked" => "reported that TL3's environment blocked the work",
        "git_remote" => "could not reach the Git remote",
        "harness" => "rejected TL3's own check input inside the PrismaBuild action",
        crate::postmortems::PAUSED => {
            "failed postmortem runs, so claims are paused until the incident is cleared or recovered"
        }
        _ => "could not finish the operation",
    };
    let next = if needs_rob {
        "Rob must clear this problem."
    } else {
        "Work is waiting for recovery."
    };
    format!("For \"{title}\", {source} {reason}. {next}")
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
    fn git_remote_failures_in_worktrees_are_upstream() {
        // The operation names crates/worktrees gives its network calls.
        for operation in [
            "fetch base",
            "fetch pull head",
            "fetch pinned pull commit",
            "publish head",
            "read remote branch",
            "delete recorded remote branch",
        ] {
            let network: crate::Error = tl3_worktrees::Error::Network {
                operation,
                code: Some(128),
            }
            .into();
            assert_eq!(failure_class(&network), "git_remote", "{operation}");
            // Without a clear network message, the failure stays TL3-side (#309).
            let ambiguous: crate::Error = tl3_worktrees::Error::Git {
                operation,
                code: Some(128),
            }
            .into();
            assert_eq!(failure_class(&ambiguous), "transport", "{operation}");
        }
        let local: crate::Error = tl3_worktrees::Error::Git {
            operation: "abort superseded merge",
            code: Some(1),
        }
        .into();
        assert_ne!(failure_class(&local), "git_remote");
        assert_eq!(
            Cause::of(Upstream::Worktrees, "git_remote"),
            Cause::Upstream
        );
        assert_eq!(
            Cause::of(Upstream::Worktrees, "invalid_token"),
            Cause::Upstream
        );
        assert_eq!(
            Cause::of(Upstream::Worktrees, failure_class(&local)),
            Cause::Tl3
        );
        assert_eq!(Cause::of(Upstream::Workers, "transport"), Cause::Tl3);
        assert_eq!(Cause::of(Upstream::PrismaBuild, "setup"), Cause::Upstream);
        // #326: TL3's trusted harness rejecting TL3's own check input is TL3-side.
        assert_eq!(Cause::of(Upstream::PrismaBuild, "harness"), Cause::Tl3);
        let summary = incident_summary(
            "request",
            &IncidentSignature {
                upstream: Upstream::PrismaBuild,
                class: "harness".into(),
            },
            false,
            None,
        );
        assert!(summary.contains("TL3's check harness"), "{summary}");
    }

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

fn look_kind(result: &Result<Look>) -> state_decisions::LookKind {
    match result {
        Ok(Look::Found(_)) => state_decisions::LookKind::Found,
        Ok(Look::Absent) => state_decisions::LookKind::Absent,
        Ok(Look::Alive { .. }) => state_decisions::LookKind::Alive,
        Ok(Look::Waiting { .. }) => state_decisions::LookKind::Waiting,
        Ok(Look::Healthy) => state_decisions::LookKind::Healthy,
        Ok(Look::Quota { .. }) => state_decisions::LookKind::Quota,
        Ok(Look::Pending) => state_decisions::LookKind::Pending,
        Err(_) => state_decisions::LookKind::Failed,
    }
}
fn wait_status(result: &Result<crate::executor::WaitRecord>) -> state_decisions::WaitStatus {
    match result {
        Ok(crate::executor::WaitRecord::Alive) => state_decisions::WaitStatus::Alive,
        Ok(crate::executor::WaitRecord::Ended) => state_decisions::WaitStatus::Ended,
        Err(_) => state_decisions::WaitStatus::Failed,
    }
}
