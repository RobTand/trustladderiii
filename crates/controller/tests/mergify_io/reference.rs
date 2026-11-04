use super::*;
use crate::mergify_io::io::mock_std as std;
#[derive(Clone, Copy, Serialize, Deserialize)]
enum Requeue {
    Ready,
    Sending,
    Refused,
}

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


impl Adapter {
    fn completion(
        &self,
        delivery: &Delivery,
        snapshot: Option<&Snapshot>,
    ) -> Result<Reconciliation> {
        let mut baseline = self.baseline(delivery)?;
        if baseline
            .as_ref()
            .is_some_and(|attempt| matches!(attempt.requeue, Some(Requeue::Refused)))
        {
            self.retain_refused_command(delivery)?;
            return Ok(Reconciliation::Found(Box::new(Completion::permanent(
                "queue_command_refused",
            ))));
        }
        let (state, mut observations) = self.read(delivery, snapshot)?;
        let previously_confirmed =
            matches!(self.records.reconcile(delivery)?, Reconciliation::Found(_));
        if state.state != QueueState::Merged && self.unresolved_human_hold(delivery, snapshot)? {
            return Ok(Reconciliation::Pending);
        }
        if !previously_confirmed
            && state.state != QueueState::Merged
            && let Some(attempt) = baseline.as_mut()
            && matches!(attempt.requeue, Some(Requeue::Sending))
        {
            let github = self.clients[&delivery.repository.installation].clone();
            if self
                .runtime
                .block_on(Mergify::new(github).command_refusal(
                    &full_name(&delivery.repository),
                    enqueue(delivery)?.0,
                    &delivery.key,
                ))
                .map_err(adapter_error)?
                .is_some()
            {
                attempt.requeue = Some(Requeue::Refused);
                self.retain_refused_command(delivery)?;
                self.records.write(
                    &self.records.path(&delivery.key).with_extension("intent"),
                    &serde_json::to_vec(attempt)?,
                )?;
                return Ok(Reconciliation::Found(Box::new(Completion::permanent(
                    "queue_command_refused",
                ))));
            }
        }
        if !previously_confirmed && self.reruns_pending(delivery)? {
            return Ok(Reconciliation::Pending);
        }
        let current_head = state.pull_request.head.sha == enqueue(delivery)?.1;
        if !current_head && state.state != QueueState::Merged && !previously_confirmed {
            return Ok(Reconciliation::Found(Box::new(Completion {
                diagnostic: None,
                ack: Ack::Retryable,
                failure_class: None,
                observations,
                progress: None,
            })));
        }
        if current_head
            && state.pull_request.merged_at.is_none()
            && state.pull_request.mergeable == Some(false)
        {
            observations.push(Observation::Kernel(tl3_kernel::Event::Fact {
                node: delivery.node(),
                fact: tl3_kernel::Fact::Conflict,
            }));
            return Ok(Reconciliation::Found(Box::new(Completion::confirmed(
                observations,
            ))));
        }
        let active = matches!(state.state, QueueState::Queued | QueueState::ChecksRunning);
        let terminal = matches!(
            state.state,
            QueueState::Dequeued
                | QueueState::FailedCi
                | QueueState::Conflict
                | QueueState::DequeuedByPerson
        );
        let mut person_removed = false;
        if baseline.is_some() || self.repaired_head(delivery, snapshot)? {
            let github = &self.clients[&delivery.repository.installation];
            let events = self.runtime.block_on(
                github.issue_events(&full_name(&delivery.repository), enqueue(delivery)?.0),
            )?;
            let mut person_observations = Vec::new();
            for fact in person_facts(
                &delivery.repository,
                enqueue(delivery)?.0,
                &events,
                state.pull_request.merged_at.is_some(),
            ) {
                let event = match fact.evidence {
                    Evidence::DequeuedByPerson { event } | Evidence::ClosedByPerson { event } => {
                        event
                    }
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
                    person_observations.push(Observation::Queue(fact));
                }
            }
            person_removed = !person_observations.is_empty();
            observations.extend(person_observations);
        }
        if previously_confirmed
            || person_removed
            || state.state == QueueState::Merged
            || (current_head
                && (active
                    || (terminal
                        && baseline.as_ref().is_some_and(|before| {
                            before.requeue.is_none() && before.check_changed(&state.check)
                        }))))
        {
            let mut completion = Completion::confirmed(observations);
            completion.progress = Some(crate::executor::ProgressToken::Mergify(Progress::from(
                &state,
            )));
            return Ok(Reconciliation::Found(Box::new(completion)));
        }
        let github = &self.clients[&delivery.repository.installation];
        let (number, _) = enqueue(delivery)?;
        let issue = self
            .runtime
            .block_on(github.issue(&full_name(&delivery.repository), number))?;
        // A repaired head's eligible waiting check is not dequeue completion.
        // Its bounded recovery is reconciled by repository/PR/head below.
        if state.state == QueueState::Waiting
            && issue.labels.iter().any(|label| label.name == QUEUE_LABEL)
            && (self.has_recovery(delivery)? || self.repaired_head(delivery, snapshot)?)
        {
            return Ok(Reconciliation::Pending);
        }
        // Without a durable intent this key has never reached the label write.
        if baseline.is_none() {
            return Ok(if self.dequeue_pending(delivery, &state, &issue)? {
                Reconciliation::Pending
            } else {
                Reconciliation::Absent
            });
        }
        if let Some(requeue) = baseline.as_ref().and_then(|attempt| attempt.requeue) {
            return Ok(
                if self.has_recovery(delivery)?
                    || matches!(requeue, Requeue::Sending)
                    || self.dequeue_pending(delivery, &state, &issue)?
                {
                    Reconciliation::Pending
                } else {
                    Reconciliation::Absent
                },
            );
        }
        if issue.labels.iter().any(|label| label.name == QUEUE_LABEL)
            || self
                .records
                .path(&delivery.key)
                .with_extension("applied")
                .exists()
        {
            Ok(Reconciliation::Pending)
        } else {
            Ok(Reconciliation::Absent)
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
        if self.dequeue_pending(delivery, &state, &issue)? {
            return Err(Error::Invalid("Mergify dequeue is still finishing"));
        }
        self.rerun_head_checks(delivery, &state)?;
        if !self
            .runtime
            .block_on(head_checks::read(&github, &repo, &state.pull_request))?
            .ready
        {
            return Err(Error::ChecksPending);
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
        let path = self.records.path(&delivery.key).with_extension("intent");
        self.records.write(&path, &serde_json::to_vec(&attempt)?)?;
        self.fence(delivery)?;
        if attempt.requeue.is_some() {
            if let Reconciliation::Found(completion) = self.queue_command(delivery, false)? {
                return Ok(*completion);
            }
        } else {
            match self
                .runtime
                .block_on(Mergify::new(github).queue(&repo, number, head))
            {
                Ok(()) => {}
                Err(tl3_mergify::Error::HeadChanged) => {
                    return match self.completion(delivery, self.snapshot()?.as_ref())? {
                        Reconciliation::Found(completion) => Ok(*completion),
                        _ => Err(Error::Invalid("PR changed during enqueue")),
                    };
                }
                Err(error) => return Err(adapter_error(error)),
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
        if !retry || state.dequeue_rule_concluded {
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
        // Mergify can leave the previous cycle's dequeued label after re-entry.
        let fresh = label_event("dequeued").is_some_and(|dequeued| {
            dequeued.event == "labeled"
                && label_event("queued").map_or_else(
                    || {
                        state
                            .check
                            .as_ref()
                            .and_then(|check| check.completed_at)
                            .is_some_and(|finished| dequeued.created_at >= finished)
                    },
                    |queued| dequeued.id > queued.id,
                )
        });
        Ok(!fresh)
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
