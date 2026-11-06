use super::*;
use serde_json::json;
use crate::mergify_io::io::mock_std as std;
use lifecycle::Command;
use tl3_mergify::decide::recovery::{budget, command, eligibility, schedule};
/// The queue state and facts an existing read already observed (4g).
pub(super) type Observed<'a> = Option<(&'a tl3_mergify::Observation, &'a [Observation])>;

#[derive(Serialize, Deserialize)]
struct Recovery {
    eligible_since_ms: Option<u64>,
    command: Command,
    /// An old executor may already have sent the effect-keyed command.
    command_key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    retry: Option<queue_retry::RetryTransition>,
}
impl Adapter {
    fn recovery_key(&self, delivery: &Delivery) -> Result<String> {
        let (number, head) = enqueue(delivery)?;
        Ok(format!(
            "mergify-requeue:{}/{}:{number}:{head}",
            delivery.repository.owner.to_ascii_lowercase(),
            delivery.repository.name.to_ascii_lowercase()
        ))
    }
    fn recovery_path(&self, delivery: &Delivery) -> Result<std::path::PathBuf> {
        let digest = Sha256::digest(self.recovery_key(delivery)?);
        Ok(self
            .records
            .path(&format!("requeue:{digest:x}"))
            .with_extension("requeue"))
    }
    pub(super) fn retain_refused_command(&self, delivery: &Delivery) -> Result<()> {
        self.save_recovery(
            delivery,
            &Recovery {
                eligible_since_ms: None,
                command: Command::Refused,
                command_key: delivery.key.clone(),
                retry: None,
            },
        )
    }
    pub(super) fn retain_legacy_command(&self, delivery: &Delivery) -> Result<()> {
        let Some(attempt) = self.baseline(delivery)? else {
            return Ok(());
        };
        let Some(command) = Self::legacy_command(attempt.requeue) else {
            return Ok(());
        };
        // Closed nodes have already lost their PR binding. Their head receipt
        // survives retirement independently of these effect-keyed files.
        if !budget::legacy_binding(delivery.pull_request.is_some(), delivery.head_sha.is_some()) {
            return Ok(());
        }
        if self
            .recovery(delivery)?
            .is_some_and(|recovery| !budget::inspect_history(Self::command_stage(recovery.command)))
        {
            return Ok(());
        }
        self.save_recovery(
            delivery,
            &Recovery {
                eligible_since_ms: None,
                command,
                command_key: delivery.key.clone(),
                retry: None,
            },
        )
    }
    fn recovery(&self, delivery: &Delivery) -> Result<Option<Recovery>> {
        let path = self.recovery_path(delivery)?;
        match std::fs::read(path) {
            Ok(bytes) => Ok(Some(serde_json::from_slice(&bytes)?)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error.into()),
        }
    }
    pub(super) fn has_recovery(&self, delivery: &Delivery) -> Result<bool> {
        Ok(self.recovery(delivery)?.is_some())
    }
    fn save_recovery(&self, delivery: &Delivery, recovery: &Recovery) -> Result<()> {
        // This record belongs to the head, so retire_delivery must leave it intact.
        self.records.write(
            &self.recovery_path(delivery)?,
            &serde_json::to_vec(recovery)?,
        )?;
        if recovery.command == Command::Sent {
            self.publish_queue_retry(delivery, recovery)?;
        }
        Ok(())
    }
    fn recovery_eligible(
        &self,
        delivery: &Delivery,
        state: &tl3_mergify::Observation,
        snapshot: Option<&Snapshot>,
    ) -> Result<bool> {
        let (number, head) = enqueue(delivery)?;
        let pull = &state.pull_request;
        let mut read = eligibility::Eligibility {
            current_head: pull.head.sha == head,
            open: pull.state == "open",
            merged: pull.merged_at.is_some(),
            draft: pull.draft,
            mergeable: pull.mergeable == Some(true),
            repaired: None,
            waiting: state.state == QueueState::Waiting,
            settled: checks_settled(state),
            eligible_check: state
                .check
                .as_ref()
                .is_some_and(tl3_mergify::eligible_waiting),
            queue_label: None,
            held: None,
            checks_ready: None,
            reruns_pending: None,
        };
        loop {
            match eligibility::recovery_eligible(read) {
                eligibility::EligibilityStep::Stop => return Ok(false),
                eligibility::EligibilityStep::Eligible => return Ok(true),
                eligibility::EligibilityStep::Repaired => {
                    read.repaired = Some(self.repaired_head(delivery, snapshot)?);
                }
                eligibility::EligibilityStep::Issue => {
                    let github = &self.clients[&delivery.repository.installation];
                    let repo = full_name(&delivery.repository);
                    let issue = self.runtime.block_on(github.issue(&repo, number))?;
                    read.queue_label =
                        Some(issue.labels.iter().any(|label| label.name == QUEUE_LABEL));
                }
                eligibility::EligibilityStep::Events => {
                    let github = &self.clients[&delivery.repository.installation];
                    let repo = full_name(&delivery.repository);
                    let events = self.runtime.block_on(github.issue_events(&repo, number))?;
                    read.held = Some(self.human_hold_in_events(delivery, snapshot, &events)?);
                }
                eligibility::EligibilityStep::Checks => {
                    let github = &self.clients[&delivery.repository.installation];
                    let repo = full_name(&delivery.repository);
                    let checks = self
                        .runtime
                        .block_on(head_checks::read(github, &repo, pull))?;
                    read.checks_ready = Some(checks.ready && checks.unavailable.is_empty());
                }
                eligibility::EligibilityStep::Reruns => {
                    read.reruns_pending = Some(self.reruns_pending(delivery)?);
                }
            }
        }
    }
    fn command_budget(&self, delivery: &Delivery) -> Result<Recovery> {
        let mut recovery = self.recovery(delivery)?.unwrap_or(Recovery {
            eligible_since_ms: None,
            command: Command::Ready,
            command_key: self.recovery_key(delivery)?,
            retry: None,
        });
        if !budget::inspect_history(Self::command_stage(recovery.command)) {
            return Ok(recovery);
        }
        let (number, head) = enqueue(delivery)?;
        let mut candidates = vec![delivery.clone()];
        if let Some(store) = &self.store {
            for effect in tl3_store::Store::open(store)?.effects()? {
                let prior: Delivery = crate::decode_stored(effect.payload)?;
                if budget::archive_candidate(
                    budget::Binding {
                        repository: prior.repository == delivery.repository,
                        pull_request: prior.pull_request == Some(number),
                        head: prior.head_sha.as_deref() == Some(head),
                    },
                    matches!(
                        prior.output,
                        Out::Effect {
                            kind: EffectKind::Enqueue,
                            ..
                        }
                    ),
                ) {
                    candidates.push(prior);
                }
            }
        }
        let github = &self.clients[&delivery.repository.installation];
        let comments = self
            .runtime
            .block_on(github.comments(&full_name(&delivery.repository), number))?;
        let head_key = self.recovery_key(delivery)?;
        for position in 0..=candidates.len() {
            let key = match budget::effect_key_index(position) {
                None => head_key.clone(),
                Some(index) => candidates[index].key.clone(),
            };
            let marker = GitHub::marker(&key);
            if comments.iter().any(|comment| {
                budget::sent(
                    comment.user.login == format!("{}[bot]", github.app_slug()),
                    comment.body.lines().next() == Some("@mergifyio queue"),
                    comment.body.lines().any(|line| line == marker),
                )
            }) {
                recovery.command = Command::Sent;
                recovery.command_key = key;
                self.save_recovery(delivery, &recovery)?;
                return Ok(recovery);
            }
        }
        for prior in candidates {
            if let Some(attempt) = self.baseline(&prior)? {
                let Some(command) = Self::legacy_command(attempt.requeue) else {
                    continue;
                };
                recovery.command = command;
                recovery.command_key = prior.key;
                self.save_recovery(delivery, &recovery)?;
                break;
            }
        }
        Ok(recovery)
    }
    fn reconcile_command(
        &self,
        delivery: &Delivery,
        recovery: &mut Recovery,
        sent_at: &mut Option<chrono::DateTime<chrono::Utc>>,
    ) -> Result<Option<Reconciliation>> {
        let effects =
            self.recovery_decide(delivery, recovery, lifecycle::QueueEvent::CommandStatus)?;
        match effects.as_slice() {
            [lifecycle::Effect::Continue] => return Ok(None),
            [lifecycle::Effect::Complete(lifecycle::Complete::Permanent(failure))] => {
                return Ok(Some(refused(failure.class())));
            }
            [lifecycle::Effect::Read(lifecycle::Need::CommandReply)] => {}
            _ => return Err(Error::Invalid("unexpected command reconciliation effect")),
        }
        let github = self.clients[&delivery.repository.installation].clone();
        let repo = full_name(&delivery.repository);
        let (number, _) = enqueue(delivery)?;
        let refusal = self
            .runtime
            .block_on(Mergify::new(github.clone()).command_refusal(
                &repo,
                number,
                &recovery.command_key,
            ))
            .map_err(adapter_error)?
            .is_some();
        let comment = if refusal {
            false
        } else {
            let found = self.runtime.block_on(github.reconcile_comments(
                &repo,
                number,
                &recovery.command_key,
            ))?;
            *sent_at = found.as_ref().map(|comment| comment.created_at);
            found.is_some()
        };
        let found = command::receipt_found(refusal, comment);
        let effects = self.recovery_decide(
            delivery,
            recovery,
            lifecycle::QueueEvent::CommandReceipt {
                refused: refusal,
                found,
            },
        )?;
        match effects.as_slice() {
            [lifecycle::Effect::Complete(lifecycle::Complete::Permanent(failure))] => {
                Ok(Some(refused(failure.class())))
            }
            // An absent comment cannot disprove an in-flight POST.
            [lifecycle::Effect::Complete(lifecycle::Complete::Pending)] => {
                Ok(Some(Reconciliation::Pending))
            }
            _ => Err(Error::Invalid("unexpected command receipt effect")),
        }
    }
    pub(super) fn recover_waiting(
        &mut self,
        delivery: &Delivery,
        snapshot: Option<&Snapshot>,
        observed: Observed<'_>,
    ) -> Result<Reconciliation> {
        let saved = self.recovery(delivery)?;
        let gate = match schedule::recovery_gate(saved.is_some(), None) {
            schedule::RecoveryGate::Repaired => {
                schedule::recovery_gate(false, Some(self.repaired_head(delivery, snapshot)?))
            }
            gate => gate,
        };
        if gate == schedule::RecoveryGate::Pending {
            return Ok(Reconciliation::Pending);
        }
        let mut sent_at = None;
        if let Some(mut recovery) = saved
            && let Some(result) = self.reconcile_command(delivery, &mut recovery, &mut sent_at)?
        {
            return self.command_ending(delivery, result, sent_at, observed);
        }
        let (state, _) = self.read(delivery, snapshot)?;
        if !self.recovery_eligible(delivery, &state, snapshot)? {
            let saved = self.recovery(delivery)?;
            if schedule::ineligible_tick(saved.is_some())
                && let Some(mut recovery) = saved
            {
                self.recovery_decide(
                    delivery,
                    &mut recovery,
                    lifecycle::QueueEvent::RecoveryTick {
                        eligible: false,
                        now_ms: 0,
                    },
                )?;
            }
            return Ok(Reconciliation::Pending);
        }
        let now_ms = self.recovery_now_ms();
        // Poll the saved timer without rereading the delivery archive. Dispatch
        // adopts historical commands again before consuming the head budget.
        let mut recovery = match self.recovery(delivery)? {
            Some(recovery) if schedule::saved_timer(recovery.eligible_since_ms) => recovery,
            _ => self.command_budget(delivery)?,
        };
        if let Some(result) = self.reconcile_command(delivery, &mut recovery, &mut sent_at)? {
            return Ok(result);
        }
        let effects = self.recovery_decide(
            delivery,
            &mut recovery,
            lifecycle::QueueEvent::RecoveryTick {
                eligible: true,
                now_ms,
            },
        )?;
        match effects.as_slice() {
            [lifecycle::Effect::Read(lifecycle::Need::Command)] => {
                self.queue_command(delivery, Some(now_ms), None)
            }
            [lifecycle::Effect::Complete(lifecycle::Complete::Pending)] => {
                Ok(Reconciliation::Pending)
            }
            _ => Err(Error::Invalid("unexpected recovery timer effect")),
        }
    }
    pub(super) fn queue_command(
        &mut self,
        delivery: &Delivery,
        recovery_tick_ms: Option<u64>,
        observed: Observed<'_>,
    ) -> Result<Reconciliation> {
        let waiting = recovery_tick_ms.is_some();
        let mut recovery = self.command_budget(delivery)?;
        let mut sent_at = None;
        if let Some(result) = self.reconcile_command(delivery, &mut recovery, &mut sent_at)? {
            return self.command_ending(delivery, result, sent_at, observed);
        }
        let snapshot = self.snapshot()?;
        let (fresh, _) = self.read(delivery, snapshot.as_ref())?;
        if let Some(snapshot) = &snapshot
            && let Some(hold) =
                queue_retry::held(snapshot, delivery.node(), enqueue(delivery)?.1)?
            && (hold.context.branch != fresh.pull_request.base.branch
                || hold.witness.as_ref().is_none_or(|gate| {
                    gate.context.branch != fresh.pull_request.base.branch
                        || !gate.healthy_for(&hold.reporters, hold.reporters_complete)
                }))
        {
            // 4g (PQ #3318): an enqueue whose own read shows the held head
            // dequeued ends there; it never reports a send it did not make.
            if let Some((state, facts)) = observed
                && Self::dequeued(enqueue(delivery)?.1, &fresh)
            {
                return Ok(Self::ended(state, facts));
            }
            return Ok(Reconciliation::Pending);
        }
        let gate = match schedule::dispatch_gate(waiting, None) {
            schedule::DispatchGate::Eligibility => schedule::dispatch_gate(
                waiting,
                Some(self.recovery_eligible(delivery, &fresh, snapshot.as_ref())?),
            ),
            gate => gate,
        };
        let eligible = gate == schedule::DispatchGate::Holds;
        let (number, head) = enqueue(delivery)?;
        let github = self.clients[&delivery.repository.installation].clone();
        let repo = full_name(&delivery.repository);
        // Preserve the eligibility short circuit before the issue-events read.
        let (held, paused) = if gate == schedule::DispatchGate::Holds {
            let events = self.runtime.block_on(github.issue_events(&repo, number))?;
            (
                self.human_hold_in_events(delivery, snapshot.as_ref(), &events)?,
                snapshot.as_ref().is_some_and(|snapshot| {
                    snapshot.rows.iter().any(|row| {
                        row.table == "nodes"
                            && row.id == delivery.node().0.to_string()
                            && row.value["paused"] == true
                    })
                }),
            )
        } else {
            (false, false)
        };
        let (next, effects) = lifecycle::decide(
            lifecycle::EnqueueState {
                recovery: Some(lifecycle::Recovery {
                    eligible_since_ms: recovery.eligible_since_ms,
                    command: recovery.command,
                }),
                ..Default::default()
            },
            lifecycle::QueueEvent::DispatchCommand(lifecycle::CommandRead {
                head: lifecycle_head(fresh.pull_request.head.sha == head),
                open: fresh.pull_request.state == "open",
                merged: fresh.pull_request.merged_at.is_some(),
                draft: fresh.pull_request.draft,
                held,
                paused,
                waiting,
                eligible,
                now_ms: recovery_tick_ms.unwrap_or_default(),
            }),
        );
        let next = next
            .recovery
            .ok_or(Error::Invalid("command decision lost its state"))?;
        recovery.eligible_since_ms = next.eligible_since_ms;
        recovery.command = next.command;
        for effect in effects {
            match effect {
                lifecycle::Effect::SaveRecovery => {
                    if schedule::fence(Self::command_stage(recovery.command)) {
                        self.fence(delivery)?;
                    }
                    if recovery.command == Command::Sending
                        && recovery.retry.is_none()
                        && snapshot.as_ref().is_some_and(|snapshot| {
                            snapshot.rows.iter().any(|row| {
                                row.table == "queue_holds"
                                    && row.id == queue_retry::key(delivery.node(), head)
                            })
                        })
                    {
                        recovery.retry =
                            self.queue_retry_binding(delivery, &fresh, &recovery.command_key)?;
                    }
                    self.save_recovery(delivery, &recovery)?;
                }
                lifecycle::Effect::Complete(lifecycle::Complete::Pending) => {
                    return Ok(Reconciliation::Pending);
                }
                lifecycle::Effect::Complete(lifecycle::Complete::Permanent(failure)) => {
                    return Ok(refused(failure.class()));
                }
                lifecycle::Effect::Write(lifecycle::Write::Command) => {
                    let result = self.runtime.block_on(Mergify::new(github.clone()).requeue(
                        &repo,
                        number,
                        head,
                        &recovery.command_key,
                    ));
                    let reply = match &result {
                        Ok(()) => command::Reply::Accepted,
                        Err(tl3_mergify::Error::GitHub(tl3_github::Error::Http { status })) => {
                            command::Reply::Http(*status)
                        }
                        Err(tl3_mergify::Error::GitHub(tl3_github::Error::RateLimited {
                            ..
                        })) => command::Reply::RateLimited,
                        Err(_) => command::Reply::Other,
                    };
                    let classified = command::command_result(reply);
                    let outcome = match classified {
                        command::Outcome::Accepted => lifecycle::CommandResult::Accepted,
                        command::Outcome::PermissionDenied => {
                            lifecycle::CommandResult::PermissionDenied
                        }
                        command::Outcome::Rejected => lifecycle::CommandResult::Rejected,
                        command::Outcome::Unknown => lifecycle::CommandResult::Unknown,
                    };
                    self.recovery_decide(
                        delivery,
                        &mut recovery,
                        lifecycle::QueueEvent::CommandDone(outcome),
                    )?;
                    if command::permission_refusal(classified) {
                        return Ok(refused("queue_command_permission"));
                    }
                    if let Err(error) = result {
                        return Err(adapter_error(error));
                    }
                }
                _ => return Err(Error::Invalid("unexpected recovery dispatch effect")),
            }
        }
        Ok(Reconciliation::Pending)
    }
    fn recovery_decide(
        &self,
        delivery: &Delivery,
        recovery: &mut Recovery,
        event: lifecycle::QueueEvent,
    ) -> Result<Vec<lifecycle::Effect>> {
        let (next, effects) = lifecycle::decide(
            lifecycle::EnqueueState {
                recovery: Some(lifecycle::Recovery {
                    eligible_since_ms: recovery.eligible_since_ms,
                    command: recovery.command,
                }),
                ..Default::default()
            },
            event,
        );
        let next = next
            .recovery
            .ok_or(Error::Invalid("recovery decision lost its state"))?;
        recovery.eligible_since_ms = next.eligible_since_ms;
        recovery.command = next.command;
        let mut remaining = Vec::new();
        for effect in effects {
            if effect == lifecycle::Effect::SaveRecovery {
                self.save_recovery(delivery, recovery)?;
            } else {
                remaining.push(effect);
            }
        }
        Ok(remaining)
    }
    /// 4g / #864, found by live probe (PQ #3309): Mergify answered a Sent
    /// receipt when, on the same open head, its queue check later completed in
    /// a dequeue. Such a receipt is spent, not awaiting acceptance.
    fn answered(
        sent_at: Option<chrono::DateTime<chrono::Utc>>,
        head: &str,
        state: &tl3_mergify::Observation,
    ) -> bool {
        sent_at.is_some_and(|sent| {
            Self::dequeued(head, state)
                && state
                    .check
                    .as_ref()
                    .and_then(|check| check.completed_at)
                    .is_some_and(|done| done > sent)
        })
    }
    /// 4g: a pending command result whose receipt Mergify already answered ends
    /// this queue attempt at that dequeue. Nothing is reported sent; the
    /// dequeue facts from the same read go to #854 attribution (Q50/Q51).
    fn command_ending(
        &self,
        delivery: &Delivery,
        result: Reconciliation,
        sent_at: Option<chrono::DateTime<chrono::Utc>>,
        observed: Observed<'_>,
    ) -> Result<Reconciliation> {
        let Some((state, facts)) = observed else {
            return Ok(result);
        };
        if !matches!(result, Reconciliation::Pending)
            || !Self::answered(sent_at, enqueue(delivery)?.1, state)
        {
            return Ok(result);
        }
        Ok(Self::ended(state, facts))
    }
    /// 4g: Mergify's own queue check shows this open head dequeued.
    fn dequeued(head: &str, state: &tl3_mergify::Observation) -> bool {
        let pull = &state.pull_request;
        pull.head.sha == head
            && pull.state == "open"
            && pull.merged_at.is_none()
            && matches!(
                state.state,
                QueueState::FailedCi | QueueState::Dequeued | QueueState::Conflict
            )
    }
    /// 4g: the attempt ends at the observed dequeue; its facts go to #854.
    fn ended(state: &tl3_mergify::Observation, facts: &[Observation]) -> Reconciliation {
        let mut completion = Completion::confirmed(facts.to_vec());
        completion.progress = Some(crate::executor::ProgressToken::Mergify(Progress::from(
            state,
        )));
        Reconciliation::Found(Box::new(completion))
    }
    fn recovery_now_ms(&self) -> u64 {
        self.clock
            .now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
            .min(u64::MAX as u128) as u64
    }
    fn command_stage(command: Command) -> budget::Stage {
        match command {
            Command::Ready => budget::Stage::Ready,
            Command::Sending => budget::Stage::Sending,
            Command::Sent | Command::Refused | Command::PermissionDenied | Command::Rejected => {
                budget::Stage::Spent
            }
        }
    }
    fn legacy_command(requeue: Option<Requeue>) -> Option<Command> {
        let legacy = match requeue {
            Some(Requeue::Sending) => budget::Legacy::Sending,
            Some(Requeue::Refused) => budget::Legacy::Refused,
            Some(Requeue::Ready) | None => budget::Legacy::Unused,
        };
        budget::legacy_command(legacy, Command::Sending, Command::Refused)
    }

}
fn refused(class: &'static str) -> Reconciliation {
    Reconciliation::Found(Box::new(Completion::permanent(class)))
}

impl Adapter {
    pub(super) fn reconcile_queue_destination(
        &self,
        delivery: &Delivery,
        fresh: &tl3_mergify::Observation,
        snapshot: Option<&Snapshot>,
    ) -> Result<()> {
        let (Some(snapshot), Some(settings)) = (snapshot, &self.store) else {
            return Ok(());
        };
        let (number, head) = enqueue(delivery)?;
        let retargeted = |snapshot: &Snapshot| -> Result<Option<queue_retry::Hold>> {
            let hold = queue_retry::held(snapshot, delivery.node(), head)?;
            Ok(hold.filter(|hold| {
                hold.repository == delivery.repository
                    && hold.number == number
                    && fresh.pull_request.head.sha == head
                    && hold.context.branch != fresh.pull_request.base.branch
            }))
        };
        if retargeted(snapshot)?.is_none() {
            return Ok(());
        }
        // The caller's snapshot predates the upstream PR read. Rebind against
        // the latest revision so a concurrent commit cannot fail this read.
        let mut store = tl3_store::Store::open(settings)?;
        let snapshot = &store.load()?;
        let Some(mut hold) = retargeted(snapshot)? else {
            return Ok(());
        };
        hold.context = queue_retry::QueueContext {
            base: fresh.pull_request.base.sha.clone(),
            branch: fresh.pull_request.base.branch.clone(),
            batch: None,
            members: vec![number],
        };
        hold.witness = None;
        hold.witnessed_ms = None;
        let mut commit = tl3_store::Commit::new(
            format!(
                "queue-destination:{}:{}:{}:{}",
                delivery.node().0,
                head,
                hold.context.branch,
                snapshot.revision.get()
            ),
            json!({"queue_destination":hold.context.branch}),
            snapshot,
        );
        commit.rows.push(queue_retry::put(
            "queue_holds",
            queue_retry::key(delivery.node(), head),
            json!(hold),
        ));
        store.commit(&commit)?;
        Ok(())
    }
}

impl Adapter {
    fn queue_retry_binding(
        &self,
        delivery: &Delivery,
        fresh: &tl3_mergify::Observation,
        command_key: &str,
    ) -> Result<Option<queue_retry::RetryTransition>> {
        let Some(snapshot) = self.snapshot()? else {
            return Ok(None);
        };
        let (number, head) = enqueue(delivery)?;
        let Some(hold) = queue_retry::held(&snapshot, delivery.node(), head)? else {
            return Ok(None);
        };
        if hold.context.branch != fresh.pull_request.base.branch
            || hold.witness.as_ref().is_none_or(|gate| {
                gate.context.branch != fresh.pull_request.base.branch
                    || !gate.healthy_for(&hold.reporters, hold.reporters_complete)
            })
        {
            return Ok(None);
        }
        let mut before = vec![hold.identity];
        for row in &snapshot.rows {
            let fact = if row.table == "queue_facts" {
                Some(crate::decode_stored::<QueueFact>(row.value.clone())?)
            } else if row.table == "inbox" {
                match crate::decode_stored::<Observation>(row.value.clone())? {
                    Observation::Queue(fact) => Some(fact),
                    _ => None,
                }
            } else {
                None
            };
            if let Some(fact) = fact
                && fact.repository == delivery.repository
                && fact.number == number
                && matches!(&fact.evidence, Evidence::CiFailed {head:failed,..} if failed == head)
            {
                before.push(fact.identity());
            }
        }
        if let Some(check) = &fresh.check {
            before.push(
                QueueFact {
                    repository: delivery.repository.clone(),
                    number,
                    evidence: Evidence::CiFailed {
                        check: check.id,
                        completed_at: check.completed_at.map(|at| at.to_rfc3339()),
                        head: head.into(),
                        head_check: false,
                        infra: false,
                        details: Default::default(),
                    },
                }
                .identity(),
            );
        }
        before.sort();
        before.dedup();
        Ok(Some(queue_retry::RetryTransition {
            command_key: command_key.into(),
            before,
        }))
    }
}

impl Adapter {
    fn publish_queue_retry(&self, delivery: &Delivery, recovery: &Recovery) -> Result<()> {
        let (Some(settings), Some(retry)) = (&self.store, &recovery.retry) else {
            return Ok(());
        };
        let mut store = tl3_store::Store::open(settings)?;
        let snapshot = store.load()?;
        let (number, head) = enqueue(delivery)?;
        let Some(mut hold) = queue_retry::held(&snapshot, delivery.node(), head)? else {
            return Ok(());
        };
        if hold.retry.is_some() || hold.repository != delivery.repository || hold.number != number {
            return Ok(());
        }
        hold.retry = Some(retry.clone());
        let mut commit = tl3_store::Commit::new(
            format!("queue-retry:{}", retry.command_key),
            json!({"queue_retry":retry.command_key}),
            &snapshot,
        );
        commit.rows.push(queue_retry::put(
            "queue_holds",
            queue_retry::key(hold.node, head),
            json!(hold),
        ));
        // Recover a crash after the Sent receipt but before this publication.
        // Previously held post-dispatch reports must be classified again.
        for row in snapshot
            .rows
            .iter()
            .filter(|row| row.table == "queue_facts")
        {
            let fact: QueueFact = crate::decode_stored(row.value.clone())?;
            if fact.repository == delivery.repository
                && fact.number == number
                && !retry.before.contains(&fact.identity())
                && matches!(&fact.evidence, Evidence::CiFailed {head:failed,details,..} if failed == head && !matches!(details.cause.as_deref(),Some(queue_retry::CauseEvidence::Change {..})))
            {
                commit.rows.push(tl3_store::RowChange::Delete {
                    table: "queue_facts".into(),
                    id: row.id.clone(),
                });
                commit.rows.push(queue_retry::put(
                    "inbox",
                    format!("retry:{}:{}", retry.command_key, row.id),
                    json!(Observation::Queue(fact)),
                ));
            }
        }
        store.commit(&commit)?;
        Ok(())
    }
}

mod queue_retry {
    use super::*;
    use chrono::{DateTime, Utc};
    pub use crate::queue_cause::{CauseEvidence, QueueContext, GateEvidence, CheckIdentity};
/// Captured in the shared command receipt before POST and published on Sent.
/// Failure identities already observed at that fence cannot be retest results.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct RetryTransition {
    pub command_key: String,
    pub before: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Hold {
    pub repository: Repository,
    pub number: u64,
    pub node: NodeId,
    pub head: String,
    pub identity: String,
    pub context: QueueContext,
    pub failed_at: Option<DateTime<Utc>>,
    pub since_ms: u64,
    pub witness: Option<GateEvidence>,
    pub witnessed_ms: Option<u64>,
    #[serde(default)]
    pub retry: Option<RetryTransition>,
    #[serde(default)]
    pub reporters: Vec<CheckIdentity>,
    #[serde(default)]
    pub reporters_complete: bool,
}

pub(crate) fn key(node: NodeId, head: &str) -> String {
    format!("{}:{head}", node.0)
}

pub(crate) fn held(snapshot: &Snapshot, node: NodeId, head: &str) -> Result<Option<Hold>> {
    snapshot
        .rows
        .iter()
        .find(|row| row.table == "queue_holds" && row.id == key(node, head))
        .map(|row| decode_stored(row.value.clone()))
        .transpose()
}


    pub fn put(table: &str, id: String, value: Value) -> ::tl3_store::RowChange {
        ::tl3_store::RowChange::Put {table:table.into(),id,value}
    }
}
