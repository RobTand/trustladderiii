use super::*;
use crate::mergify_io::io::mock_std as std;
use lifecycle::Requeue;
#[derive(Serialize, Deserialize)]
struct Attempt {
    before: Option<CheckRun>,
    last_event: u64,
    #[serde(default)]
    requeue: Option<Requeue>,
}
impl Attempt {
    fn check_changed(&self, current: &Option<CheckRun>) -> bool {
        let mut current = current.clone();
        // Metadata added after an intent was saved does not prove queue acceptance.
        if let Some(check) = &mut current {
            check.completed_at = self.before.as_ref().and_then(|before| before.completed_at);
            check.check_suite = self
                .before
                .as_ref()
                .and_then(|before| before.check_suite.clone());
        }
        self.before != current
    }
}

fn lifecycle_error(failure: lifecycle::Failure) -> Error {
    if failure == lifecycle::Failure::PullClosed {
        adapter_error(tl3_mergify::Error::PullRequestClosed)
    } else if failure == lifecycle::Failure::ChecksPending {
        Error::ChecksPending
    } else {
        Error::Invalid(failure.class())
    }
}

fn lifecycle_head(current: bool) -> tl3_mergify::decide::Head {
    if current {
        tl3_mergify::decide::Head::Current
    } else {
        tl3_mergify::decide::Head::Other
    }
}


impl Adapter {
    fn completion(
        &self,
        delivery: &Delivery,
        snapshot: Option<&Snapshot>,
    ) -> Result<Reconciliation> {
        let mut baseline = self.baseline(delivery)?;
        let mut core = lifecycle::EnqueueState {
            intent: baseline.as_ref().map(|attempt| lifecycle::Intent {
                requeue: attempt.requeue,
            }),
            ..Default::default()
        };
        let mut event = lifecycle::QueueEvent::Begin;
        let mut observed = None;
        let mut read = None;
        let mut observations = Vec::new();
        let mut issue = None;
        loop {
            let (next, effects) = lifecycle::decide(core, event);
            core = next;
            let mut requested = None;
            for effect in effects {
                match effect {
                    lifecycle::Effect::SaveRecovery => self.retain_refused_command(delivery)?,
                    lifecycle::Effect::SaveIntent => {
                        let attempt = baseline
                            .as_mut()
                            .ok_or(Error::Invalid("enqueue intent missing"))?;
                        attempt.requeue = core.intent.and_then(|intent| intent.requeue);
                        self.records.write(
                            &self.records.path(&delivery.key).with_extension("intent"),
                            &serde_json::to_vec(attempt)?,
                        )?;
                    }
                    lifecycle::Effect::Read(need) => requested = Some(need),
                    lifecycle::Effect::Complete(result) => {
                        return Ok(match result {
                            lifecycle::Complete::Confirmed { progress, conflict } => {
                                if conflict {
                                    observations.push(Observation::Kernel(
                                        tl3_kernel::Event::Fact {
                                            node: delivery.node(),
                                            fact: tl3_kernel::Fact::Conflict,
                                        },
                                    ));
                                }
                                let mut completion = Completion::confirmed(observations);
                                if progress {
                                    completion.progress = observed.as_ref().map(|state| {
                                        crate::executor::ProgressToken::Mergify(Progress::from(
                                            state,
                                        ))
                                    });
                                }
                                Reconciliation::Found(Box::new(completion))
                            }
                            lifecycle::Complete::Retryable => {
                                Reconciliation::Found(Box::new(Completion {
                                    diagnostic: None,
                                    ack: Ack::Retryable,
                                    failure_class: None,
                                    observations,
                                    progress: None,
                                }))
                            }
                            lifecycle::Complete::Permanent(failure) => Reconciliation::Found(
                                Box::new(Completion::permanent(failure.class())),
                            ),
                            lifecycle::Complete::Pending => Reconciliation::Pending,
                            lifecycle::Complete::Absent => Reconciliation::Absent,
                        });
                    }
                    _ => return Err(Error::Invalid("unexpected enqueue reconciliation effect")),
                }
            }
            match requested.ok_or(Error::Invalid("enqueue reconciliation needs a read"))? {
                lifecycle::Need::Queue => {
                    let (state, facts) = self.read(delivery, snapshot)?;
                    observations = facts;
                    core.confirmed =
                        matches!(self.records.reconcile(delivery)?, Reconciliation::Found(_));
                    read = Some(lifecycle::QueueRead {
                        head: lifecycle_head(state.pull_request.head.sha == enqueue(delivery)?.1),
                        queue: state.state,
                        merged: state.pull_request.merged_at.is_some(),
                        conflicts: state.pull_request.mergeable == Some(false),
                        held: state.state != QueueState::Merged
                            && self.unresolved_human_hold(delivery, snapshot)?,
                        check_changed: baseline
                            .as_ref()
                            .is_some_and(|before| before.check_changed(&state.check)),
                        refusal: None,
                        reruns_pending: None,
                        person_removed: None,
                        issue: None,
                    });
                    observed = Some(state);
                }
                lifecycle::Need::Refusal => {
                    let github = self.clients[&delivery.repository.installation].clone();
                    read.as_mut().unwrap().refusal = Some(
                        self.runtime
                            .block_on(Mergify::new(github).command_refusal(
                                &full_name(&delivery.repository),
                                enqueue(delivery)?.0,
                                &delivery.key,
                            ))
                            .map_err(adapter_error)?
                            .is_some(),
                    );
                }
                lifecycle::Need::Reruns => {
                    read.as_mut().unwrap().reruns_pending = Some(self.reruns_pending(delivery)?)
                }
                lifecycle::Need::Person => {
                    let mut removed = false;
                    if baseline.is_some() || self.repaired_head(delivery, snapshot)? {
                        let events = self.runtime.block_on(
                            self.clients[&delivery.repository.installation].issue_events(
                                &full_name(&delivery.repository),
                                enqueue(delivery)?.0,
                            ),
                        )?;
                        for fact in person_facts(
                            &delivery.repository,
                            enqueue(delivery)?.0,
                            &events,
                            observed.as_ref().unwrap().pull_request.merged_at.is_some(),
                        ) {
                            let event = match fact.evidence {
                                Evidence::DequeuedByPerson { event }
                                | Evidence::ClosedByPerson { event } => event,
                                _ => continue,
                            };
                            let fresh = if let Some(snapshot) = snapshot {
                                fact.unresolved_hold(snapshot)?
                            } else {
                                baseline
                                    .as_ref()
                                    .is_some_and(|attempt| event > attempt.last_event)
                            };
                            if fresh {
                                removed = true;
                                observations.push(Observation::Queue(fact));
                            }
                        }
                    }
                    read.as_mut().unwrap().person_removed = Some(removed);
                }
                lifecycle::Need::Issue => {
                    let current = self.runtime.block_on(
                        self.clients[&delivery.repository.installation]
                            .issue(&full_name(&delivery.repository), enqueue(delivery)?.0),
                    )?;
                    read.as_mut().unwrap().issue = Some(lifecycle::IssueRead {
                        queue_label: current.labels.iter().any(|label| label.name == QUEUE_LABEL),
                        recovery: None,
                        requeue_recovery: None,
                        repaired: None,
                        dequeue_pending: None,
                        applied: None,
                    });
                    issue = Some(current);
                }
                lifecycle::Need::RecoveryFile => {
                    read.as_mut().unwrap().issue.as_mut().unwrap().recovery =
                        Some(self.has_recovery(delivery)?)
                }
                lifecycle::Need::RequeueRecoveryFile => {
                    read.as_mut()
                        .unwrap()
                        .issue
                        .as_mut()
                        .unwrap()
                        .requeue_recovery = Some(self.has_recovery(delivery)?)
                }
                lifecycle::Need::RepairedHead => {
                    read.as_mut().unwrap().issue.as_mut().unwrap().repaired =
                        Some(self.repaired_head(delivery, snapshot)?)
                }
                lifecycle::Need::Dequeue => {
                    read.as_mut()
                        .unwrap()
                        .issue
                        .as_mut()
                        .unwrap()
                        .dequeue_pending = Some(self.dequeue_pending(
                        delivery,
                        observed.as_ref().unwrap(),
                        issue.as_ref().unwrap(),
                    )?);
                }
                lifecycle::Need::Applied => {
                    read.as_mut().unwrap().issue.as_mut().unwrap().applied = Some(
                        self.records
                            .path(&delivery.key)
                            .with_extension("applied")
                            .exists(),
                    )
                }
                _ => return Err(Error::Invalid("unexpected enqueue read request")),
            }
            event = lifecycle::QueueEvent::Reconcile(read.unwrap());
        }
    }
    fn execute(&mut self, delivery: &Delivery) -> Result<Completion> {
        self.fence(delivery)?;
        match self.read_enqueue(delivery)? {
            Reconciliation::Found(completion) => {
                if completion.ack == Ack::Confirmed
                    && !matches!(self.records.reconcile(delivery)?, Reconciliation::Found(_))
                {
                    self.records
                        .deliver(delivery, Completion::confirmed(vec![]))?;
                }
                return Ok(*completion);
            }
            Reconciliation::Pending => {
                return Err(Error::Invalid("Mergify acceptance is still unknown"));
            }
            Reconciliation::Absent => {}
        }
        let snapshot = self.snapshot()?;
        let (state, _) = self.read(delivery, snapshot.as_ref())?;
        let (number, head) = enqueue(delivery)?;
        let github = self.clients[&delivery.repository.installation].clone();
        let repo = full_name(&delivery.repository);
        let issue = self.runtime.block_on(github.issue(&repo, number))?;
        let pending = self.dequeue_pending(delivery, &state, &issue)?;
        let (_, gate) = lifecycle::decide(
            lifecycle::EnqueueState::default(),
            lifecycle::QueueEvent::ExecuteGate {
                dequeue_pending: pending,
                checks_ready: None,
            },
        );
        for effect in gate {
            match effect {
                lifecycle::Effect::Error(failure) => return Err(lifecycle_error(failure)),
                lifecycle::Effect::Read(lifecycle::Need::HeadChecks) => {
                    self.rerun_head_checks(delivery, &state)?;
                    let ready = self
                        .runtime
                        .block_on(head_checks::read(&github, &repo, &state.pull_request))?
                        .ready;
                    let (_, gate) = lifecycle::decide(
                        lifecycle::EnqueueState::default(),
                        lifecycle::QueueEvent::ExecuteGate {
                            dequeue_pending: false,
                            checks_ready: Some(ready),
                        },
                    );
                    for effect in gate {
                        if let lifecycle::Effect::Error(failure) = effect {
                            return Err(lifecycle_error(failure));
                        }
                    }
                }
                _ => return Err(Error::Invalid("unexpected enqueue execution gate")),
            }
        }
        let attempt = match self.baseline(delivery)? {
            Some(attempt) => attempt,
            None => Attempt {
                before: state.check.clone(),
                last_event: self
                    .runtime
                    .block_on(github.issue_events(&repo, number))?
                    .iter()
                    .map(|event| event.id)
                    .max()
                    .unwrap_or(0),
                requeue: issue
                    .labels
                    .iter()
                    .any(|label| label.name == QUEUE_LABEL)
                    .then_some(Requeue::Ready),
            },
        };
        let (prepared, effects) = lifecycle::decide(
            lifecycle::EnqueueState {
                intent: Some(lifecycle::Intent {
                    requeue: attempt.requeue,
                }),
                ..Default::default()
            },
            lifecycle::QueueEvent::Prepare {
                dequeue_pending: false,
                checks_ready: true,
                queue_label: issue.labels.iter().any(|label| label.name == QUEUE_LABEL),
            },
        );
        let mut effects = std::collections::VecDeque::from(effects);
        while let Some(effect) = effects.pop_front() {
            match effect {
                lifecycle::Effect::SaveIntent => {
                    self.records.write(
                        &self.records.path(&delivery.key).with_extension("intent"),
                        &serde_json::to_vec(&attempt)?,
                    )?;
                    self.fence(delivery)?;
                }
                lifecycle::Effect::Read(lifecycle::Need::Command) => {
                    if let Reconciliation::Found(completion) = self.queue_command(delivery, None)? {
                        return Ok(*completion);
                    }
                }
                lifecycle::Effect::Read(lifecycle::Need::LabelPull) => {
                    let pull = self.runtime.block_on(github.pull_request(&repo, number))?;
                    let (_, next) = lifecycle::decide(
                        prepared,
                        lifecycle::QueueEvent::LabelRead {
                            head: lifecycle_head(pull.head.sha == head),
                            open: pull.state == "open",
                            merged: pull.merged_at.is_some(),
                        },
                    );
                    effects.extend(next);
                }
                lifecycle::Effect::Write(lifecycle::Write::Label) => {
                    self.runtime
                        .block_on(github.add_label(&repo, number, QUEUE_LABEL))?;
                }
                lifecycle::Effect::Complete(lifecycle::Complete::Retryable) => {
                    return match self.completion(delivery, self.snapshot()?.as_ref())? {
                        Reconciliation::Found(completion) => Ok(*completion),
                        _ => Err(Error::Invalid("PR changed during enqueue")),
                    };
                }
                lifecycle::Effect::Error(failure) => return Err(lifecycle_error(failure)),
                _ => return Err(Error::Invalid("unexpected enqueue execution effect")),
            }
        }
        self.records.write(
            &self.records.path(&delivery.key).with_extension("applied"),
            b"true",
        )?;
        match self.read_enqueue(delivery)? {
            Reconciliation::Found(completion) => {
                if completion.ack == Ack::Confirmed {
                    self.records
                        .deliver(delivery, Completion::confirmed(vec![]))?;
                }
                Ok(*completion)
            }
            _ => Err(Error::Invalid(
                "enqueue request sent; Mergify acceptance is still unknown",
            )),
        }
    }
    fn reconcile(&mut self, delivery: &Delivery) -> Result<Reconciliation> {
        match self.read_enqueue(delivery) {
            Err(error) if error.is_github_not_found() => Err(error),
            Err(_) => Ok(Reconciliation::Pending),
            result => result,
        }
    }
    fn read_enqueue(&mut self, delivery: &Delivery) -> Result<Reconciliation> {
        // Reads retain the original effect's epoch, including after a controller upgrade.
        let snapshot = self.snapshot()?;
        let mut result = self.completion(delivery, snapshot.as_ref())?;
        if matches!(result, Reconciliation::Pending) {
            result = self.recover_waiting(delivery, snapshot.as_ref())?;
        }
        if let Reconciliation::Found(completion) = &result
            && completion.ack == Ack::Confirmed
            && !matches!(self.records.reconcile(delivery)?, Reconciliation::Found(_))
        {
            // This receipt writes no upstream state and may name an older epoch.
            self.records
                .remember(delivery, Completion::confirmed(vec![]))?;
        }
        Ok(result)
    }
    fn baseline(&self, delivery: &Delivery) -> Result<Option<Attempt>> {
        match std::fs::read(self.records.path(&delivery.key).with_extension("intent")) {
            Ok(bytes) => Ok(Some(serde_json::from_slice(&bytes)?)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error.into()),
        }
    }
    fn dequeue_pending(
        &self,
        delivery: &Delivery,
        state: &tl3_mergify::Observation,
        issue: &tl3_github::Issue,
    ) -> Result<bool> {
        let retry = issue.labels.iter().any(|label| label.name == QUEUE_LABEL)
            || matches!(
                state.state,
                QueueState::Dequeued | QueueState::FailedCi | QueueState::Conflict
            );
        if !lifecycle::dequeue_pending(retry, state.dequeue_rule_concluded, false, None, None, None)
        {
            return Ok(false);
        }
        if !issue.labels.iter().any(|label| label.name == "dequeued") {
            return Ok(true);
        }
        let events = self.runtime.block_on(
            self.clients[&delivery.repository.installation]
                .issue_events(&full_name(&delivery.repository), enqueue(delivery)?.0),
        )?;
        let label_event = |name: &str| {
            events
                .iter()
                .filter(|event| event.label.as_ref().is_some_and(|label| label.name == name))
                .max_by_key(|event| event.id)
        };
        Ok(lifecycle::dequeue_pending(
            retry,
            state.dequeue_rule_concluded,
            true,
            label_event("dequeued").map(|event| lifecycle::LabelEvent {
                id: event.id,
                labeled: event.event == "labeled",
                at: tl3_mergify::decide::Time {
                    secs: event.created_at.timestamp(),
                    nanos: event.created_at.timestamp_subsec_nanos(),
                },
            }),
            label_event("queued").map(|event| event.id),
            state
                .check
                .as_ref()
                .and_then(|check| check.completed_at)
                .map(|time| tl3_mergify::decide::Time {
                    secs: time.timestamp(),
                    nanos: time.timestamp_subsec_nanos(),
                }),
        ))
    }
    fn snapshot(&self) -> Result<Option<Snapshot>> {
        Ok(self
            .store
            .as_ref()
            .map(|store| tl3_store::Store::open(store)?.load())
            .transpose()?)
    }
    fn fence(&mut self, delivery: &Delivery) -> Result<()> {
        if delivery.epoch < self.epoch {
            return Err(Error::Invalid("Mergify effect epoch is fenced"));
        }
        self.records.fence(delivery.epoch)?;
        self.epoch = delivery.epoch;
        Ok(())
    }
}
