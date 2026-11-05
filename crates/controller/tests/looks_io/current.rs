use super::*;
controller!();
impl Controller {
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
                if decisions::incident_action(true, incident_view(&incident, &state.delivery))
                    == decisions::IncidentAction::Probe
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
            match decisions::incident_action(false, incident_view(&incident, &state.delivery)) {
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
            if resume > now {
                // #759: every model this step may use is out. Say so once
                // when that has lasted LADDER_OUT_MS, so an outage can't wait silently.
                let bound = state
                    .unserved_since_ms
                    .filter(|_| !state.ladder_out_noticed)
                    .map(|since| since.saturating_add(LADDER_OUT_MS));
                if bound.is_some_and(|bound| bound <= now) {
                    state.ladder_out_noticed = true;
                    state.due_ms = Some(resume);
                    return self.ladder_out_note(state, resume, now);
                }
                state.due_ms = Some(bound.map_or(resume, |bound| bound.min(resume)));
                self.save_allowance(state)?;
                return Ok(());
            }
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
        if matches!(&result, Ok(look) if !matches!(look, Look::Waiting { .. } | Look::Pending | Look::Alive { .. }))
        {
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
            Ok(Look::Found(completion)) => {
                let mut completion = *completion;
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
                    let mut setup_gap = false;
                    for observation in &completion.observations {
                        if let Observation::Queue(fact) = observation
                            && fact.setup()
                            && !fact.recorded(snapshot)?
                        {
                            setup_gap = true;
                            break;
                        }
                    }
                    let new = effect.state.is_pending()
                        || setup_gap
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
                if self.hold_class(&state.delivery) == Some(CHECKS_PENDING) {
                    // Waiting for checks is not a failure: release the guard to run
                    // again rather than spending a retry on the effect's absence.
                    self.remove_hold(&state.delivery);
                } else if (self.is_held(&state.delivery)
                    || state.failure.as_deref() == Some("not_found"))
                    && effect.state.is_pending()
                    && effect.state != EffectState::Cancelling
                    && matches!(state.delivery.output, Out::Effect { .. })
                {
                    // Absence or a repeated 404 spends the existing retry allowance.
                    self.complete(
                        &effect.key,
                        Completion {
                            diagnostic: None,
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
                // An unacknowledged effect that stopped moving past its allowance
                // keeps its failure count, so the stall can escalate (#356).
                // Inside the allowance an alive look is a successful read (#373).
                let within = now.saturating_sub(state.progress_ms)
                    < u64::from(self.allowance(&state.delivery).no_progress_secs) * 1000;
                if changed
                    || within
                    || !self
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
                self.probe_passed(Some(state), state.incident.as_deref())?;
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
            let action = decisions::stall_action(
                stall_upstream(upstream(state.delivery.output)),
                state.progress.as_deref().is_some_and(|progress| {
                    progress.starts_with(crate::adapters::mergify::REQUEUE_PROGRESS_PREFIX)
                }),
            );
            match action {
                decisions::StallAction::StopWorker => {
                    state.stopping = true;
                    state.failure = Some("no_progress".into());
                    self.save_allowance(state)?;
                    self.finish_stall(state)?;
                    return Ok(true);
                }
                decisions::StallAction::Requeue | decisions::StallAction::Unfinished => {
                    let class = match action {
                        decisions::StallAction::Requeue => "queue_requeue",
                        _ => "queue_unfinished",
                    };
                    state.failure = Some(class.into());
                    self.save_allowance(state)?;
                    self.observe(
                        &format!("stall:{}:{}", state.delivery.key, state.progress_ms),
                        Observation::Kernel(Event::Fact {
                            node: state.delivery.node(),
                            fact: Fact::UpstreamStalled,
                        }),
                    )?;
                    // The kernel acts on a stall only in Queued. An effect still
                    // unacknowledged past its allowance is a failed look, so it
                    // raises the read incident at the allowances' pace (#356).
                    if self
                        .store
                        .effect(&state.delivery.key)?
                        .is_some_and(|effect| effect.state.is_pending())
                    {
                        self.look_failed(
                            state,
                            crate::Error::Permanent {
                                class,
                                source: "the upstream never acknowledged the request".into(),
                            },
                        )?;
                    }
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
        if let Some(progress) = state.progress.as_deref().filter(|progress| {
            progress.starts_with(crate::adapters::mergify::REQUEUE_PROGRESS_PREFIX)
        }) {
            summary.push(' ');
            summary.push_str(progress);
        } else if let Some(detail) = queue_command_detail(&state.delivery, &signature.class) {
            summary.push(' ');
            summary.push_str(&detail);
        }
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
pub(crate) fn failure_class(error: &crate::Error) -> &'static str {
    match error {
        crate::Error::CounterExhausted(_) => crate::input_errors::COUNTER_HEADROOM,
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
        // Mergify's own read errors aren't GitHub errors or outages (#373).
        crate::Error::Adapter(error) if error.is::<tl3_mergify::Error>() => {
            match error.downcast_ref::<tl3_mergify::Error>() {
                Some(tl3_mergify::Error::ChangedDuringRead) => "inconsistent_read",
                Some(tl3_mergify::Error::MergeabilityPending) => "mergeability_pending",
                Some(tl3_mergify::Error::HeadChanged) => "head_changed",
                Some(tl3_mergify::Error::PullRequestClosed) => "pull_request_closed",
                Some(tl3_mergify::Error::GitHub(error)) => error.failure_class().as_str(),
                None => "transport",
            }
        }
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
pub(crate) fn incident_summary(
    title: &str,
    signature: &IncidentSignature,
    needs_rob: bool,
    app_login: Option<&str>,
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
        _ if signature.class == crate::adapters::prismabuild::HARNESS => "TL3's check harness",
        Upstream::Workers => "the agent worker",
        Upstream::Worktrees => "the Git worktree",
        Upstream::PrismaBuild => "PrismaBuild",
        Upstream::Mergify => "Mergify",
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
        "queue_unfinished" => "has not finished its queue check",
        "queue_requeue" => "has left this change awaiting Mergify re-queue",
        "permission" => "refused access",
        "queue_command_permission" => {
            "could not post the queue command. Check TL3's GitHub App repository permission Issues: Read and write and approve any updated installation permissions"
        }
        "queue_command_refused" => "refused the queue command",
        "queue_command_rejected" => "could not post the queue command because GitHub refused it",
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
pub(crate) fn queue_command_detail(delivery: &Delivery, class: &str) -> Option<String> {
    let reason = match class {
        "queue_command_refused" => "command refused by Mergify after posting",
        "queue_command_permission" | "queue_command_rejected" => {
            "command refused by GitHub; no command sent"
        }
        _ => return None,
    };
    Some(format!(
        "PR https://github.com/{}/{}/pull/{}, head {}: awaiting Mergify re-queue; {reason}.",
        delivery.repository.owner,
        delivery.repository.name,
        delivery.pull_request?,
        delivery.head_sha.as_deref()?
    ))
}
fn incident_view(incident: &Incident, delivery: &Delivery) -> decisions::IncidentView {
    decisions::IncidentView {
        needs_rob: incident.needs_rob,
        refused_command: incident.signature.class == "queue_command_refused",
        own_effect: incident.effect.as_deref() == Some(&delivery.key),
    }
}
fn stall_upstream(source: Upstream) -> decisions::StallUpstream {
    match source {
        Upstream::Workers => decisions::StallUpstream::Workers,
        Upstream::GitHub => decisions::StallUpstream::GitHub,
        Upstream::Mergify => decisions::StallUpstream::Mergify,
        Upstream::PrismaBuild => decisions::StallUpstream::PrismaBuild,
        Upstream::Worktrees => decisions::StallUpstream::Worktrees,
    }
}
