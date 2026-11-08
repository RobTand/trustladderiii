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
                    let action = state_decisions::found_action(
                        effect.state.is_pending(),
                        setup_gap,
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
                let record = if matches!(wait.upstream.as_str(), "prismabuild" | "flux") {
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

