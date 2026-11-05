//! Recovery of an eligible repaired head after Mergify forgot its queue membership.
use super::*;
use sha2::{Digest, Sha256};
use std::time::UNIX_EPOCH;

pub(crate) const PROGRESS_PREFIX: &str = "mergify:awaiting Mergify re-queue";
use lifecycle::Command;
use tl3_mergify::decide::recovery::{budget, command, eligibility, schedule};

#[derive(Serialize, Deserialize)]
struct Recovery {
    eligible_since_ms: Option<u64>,
    command: Command,
    /// An old executor may already have sent the effect-keyed command.
    command_key: String,
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
        )
    }
    /// Stored queue facts retain the verified dequeue across later heads and
    /// label removal. Labels alone never establish this history or approval.
    pub(super) fn repaired_head(
        &self,
        delivery: &Delivery,
        snapshot: Option<&Snapshot>,
    ) -> Result<bool> {
        let Some(snapshot) = snapshot else {
            return Ok(false);
        };
        let world: World = crate::snapshot::decode(snapshot)?;
        let Some(node) = world.nodes.get(delivery.node().0 as usize) else {
            return Ok(false);
        };
        let (number, head) = enqueue(delivery)?;
        let mut repaired = eligibility::Repaired {
            phase: heal_phase(Some(node.phase)),
            paused: node.paused,
            incident: node.waiting_incident,
            open: node.pr_open,
            head_approved: node.head == node.approved,
            checked_approved: node.checked == node.approved,
            approved_head: None,
            repository: None,
            pull_request: None,
        };
        loop {
            match eligibility::repaired_head(repaired) {
                eligibility::RepairStep::Stop => return Ok(false),
                eligibility::RepairStep::ApprovedHead => {
                    repaired.approved_head = Some(approved_head(snapshot, node.approved)? == head);
                }
                eligibility::RepairStep::Repository => {
                    repaired.repository = Some(
                        crate::request(snapshot, delivery.node())?.repo == delivery.repository,
                    );
                }
                eligibility::RepairStep::PullRequest => {
                    repaired.pull_request = Some(snapshot.rows.iter().any(|row| {
                        row.table == "pull_requests"
                            && row.id == delivery.node().0.to_string()
                            && row.value["number"] == number
                    }));
                }
                eligibility::RepairStep::History => break,
            }
        }
        for row in snapshot
            .rows
            .iter()
            .filter(|row| row.table == "queue_facts")
        {
            let fact: QueueFact = crate::decode_stored(row.value.clone())?;
            let (failure, other_head) = match &fact.evidence {
                Evidence::CiFailed {
                    head: prior,
                    head_check: false,
                    ..
                } => (eligibility::PriorFailure::QueueCi, prior != head),
                Evidence::Conflict { head: prior, .. } => {
                    (eligibility::PriorFailure::Conflict, prior != head)
                }
                _ => (eligibility::PriorFailure::Other, false),
            };
            if eligibility::prior_failure(
                fact.repository == delivery.repository,
                fact.number == number,
                failure,
                other_head,
            ) {
                return Ok(true);
            }
        }
        Ok(false)
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

    pub(super) fn unresolved_human_hold(
        &self,
        delivery: &Delivery,
        snapshot: Option<&Snapshot>,
    ) -> Result<bool> {
        let Some(snapshot) = snapshot else {
            return Ok(false);
        };
        let (number, _) = enqueue(delivery)?;
        for row in snapshot
            .rows
            .iter()
            .filter(|row| row.table == "queue_facts")
        {
            let fact: QueueFact = crate::decode_stored(row.value.clone())?;
            if fact.repository == delivery.repository
                && fact.number == number
                && fact.unresolved_hold(snapshot)?
            {
                return Ok(true);
            }
        }
        Ok(false)
    }
    fn human_hold_in_events(
        &self,
        delivery: &Delivery,
        snapshot: Option<&Snapshot>,
        events: &[tl3_github::IssueActivity],
    ) -> Result<bool> {
        for fact in person_facts(&delivery.repository, enqueue(delivery)?.0, events, false) {
            if eligibility::event_hold(
                snapshot
                    .map(|snapshot| fact.unresolved_hold(snapshot))
                    .transpose()?,
            ) {
                return Ok(true);
            }
        }
        self.unresolved_human_hold(delivery, snapshot)
    }
    /// Recover effect-keyed commands using the durable delivery archive, which
    /// retains repository/PR/head bindings after local effect records retire.
    fn command_budget(&self, delivery: &Delivery) -> Result<Recovery> {
        let mut recovery = self.recovery(delivery)?.unwrap_or(Recovery {
            eligible_since_ms: None,
            command: Command::Ready,
            command_key: self.recovery_key(delivery)?,
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
    fn reconcile_command(
        &self,
        delivery: &Delivery,
        recovery: &mut Recovery,
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
            self.runtime
                .block_on(github.reconcile_comments(&repo, number, &recovery.command_key))?
                .is_some()
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
    fn recovery_now_ms(&self) -> u64 {
        self.clock
            .now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
            .min(u64::MAX as u128) as u64
    }
    pub(super) fn recover_waiting(
        &mut self,
        delivery: &Delivery,
        snapshot: Option<&Snapshot>,
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
        if let Some(mut recovery) = saved
            && let Some(result) = self.reconcile_command(delivery, &mut recovery)?
        {
            return Ok(result);
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
        if let Some(result) = self.reconcile_command(delivery, &mut recovery)? {
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
                self.queue_command(delivery, Some(now_ms))
            }
            [lifecycle::Effect::Complete(lifecycle::Complete::Pending)] => {
                Ok(Reconciliation::Pending)
            }
            _ => Err(Error::Invalid("unexpected recovery timer effect")),
        }
    }
    /// Both legacy enqueue and repaired-head recovery use this one budget.
    pub(super) fn queue_command(
        &mut self,
        delivery: &Delivery,
        recovery_tick_ms: Option<u64>,
    ) -> Result<Reconciliation> {
        let waiting = recovery_tick_ms.is_some();
        let mut recovery = self.command_budget(delivery)?;
        if let Some(result) = self.reconcile_command(delivery, &mut recovery)? {
            return Ok(result);
        }
        let snapshot = self.snapshot()?;
        let (fresh, _) = self.read(delivery, snapshot.as_ref())?;
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
    pub(super) fn requeue_progress(&self, delivery: &Delivery) -> Result<Option<String>> {
        let Some(recovery) = self.recovery(delivery)? else {
            return Ok(None);
        };
        let status = match recovery.command {
            Command::Ready => {
                if recovery.eligible_since_ms.is_some() {
                    "all entry conditions met; no command sent"
                } else {
                    "eligibility changed; no command sent"
                }
            }
            Command::Sending => "command dispatch unknown; reconciling before any further action",
            Command::Sent => "command sent/awaiting acceptance",
            Command::Refused => "command refused",
            Command::PermissionDenied => {
                "command refused; GitHub App comment permission unavailable"
            }
            Command::Rejected => "command refused by GitHub; no command sent",
        };
        let (number, head) = enqueue(delivery)?;
        Ok(Some(format!(
            "{PROGRESS_PREFIX}; PR https://github.com/{}/pull/{number}; head {head}; durable command budget for this PR/head; {status}",
            full_name(&delivery.repository)
        )))
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
