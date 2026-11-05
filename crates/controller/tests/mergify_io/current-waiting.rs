use super::*;
use crate::mergify_io::io::mock_std as std;
use lifecycle::Command;

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
        let command = match attempt.requeue {
            Some(Requeue::Sending) => Command::Sending,
            Some(Requeue::Refused) => Command::Refused,
            _ => return Ok(()),
        };
        // Closed nodes have already lost their PR binding. Their head receipt
        // survives retirement independently of these effect-keyed files.
        if delivery.pull_request.is_none() || delivery.head_sha.is_none() {
            return Ok(());
        }
        if self
            .recovery(delivery)?
            .is_some_and(|recovery| !matches!(recovery.command, Command::Ready))
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
    fn recovery_eligible(
        &self,
        delivery: &Delivery,
        state: &tl3_mergify::Observation,
        snapshot: Option<&Snapshot>,
    ) -> Result<bool> {
        let (number, head) = enqueue(delivery)?;
        let pull = &state.pull_request;
        if pull.head.sha != head
            || pull.state != "open"
            || pull.merged_at.is_some()
            || pull.draft
            || pull.mergeable != Some(true)
            || !self.repaired_head(delivery, snapshot)?
            || state.state != QueueState::Waiting
            || !checks_settled(state)
            || !state
                .check
                .as_ref()
                .is_some_and(tl3_mergify::eligible_waiting)
        {
            return Ok(false);
        }
        let github = &self.clients[&delivery.repository.installation];
        let repo = full_name(&delivery.repository);
        let issue = self.runtime.block_on(github.issue(&repo, number))?;
        if !issue.labels.iter().any(|label| label.name == QUEUE_LABEL) {
            return Ok(false);
        }
        let events = self.runtime.block_on(github.issue_events(&repo, number))?;
        if self.human_hold_in_events(delivery, snapshot, &events)? {
            return Ok(false);
        }
        let checks = self
            .runtime
            .block_on(head_checks::read(github, &repo, pull))?;
        Ok(checks.ready && checks.unavailable.is_empty() && !self.reruns_pending(delivery)?)
    }
    fn command_budget(&self, delivery: &Delivery) -> Result<Recovery> {
        let mut recovery = self.recovery(delivery)?.unwrap_or(Recovery {
            eligible_since_ms: None,
            command: Command::Ready,
            command_key: self.recovery_key(delivery)?,
        });
        if !matches!(recovery.command, Command::Ready) {
            return Ok(recovery);
        }
        let (number, head) = enqueue(delivery)?;
        let mut candidates = vec![delivery.clone()];
        if let Some(store) = &self.store {
            for effect in tl3_store::Store::open(store)?.effects()? {
                let prior: Delivery = crate::decode_stored(effect.payload)?;
                if prior.repository == delivery.repository
                    && prior.pull_request == Some(number)
                    && prior.head_sha.as_deref() == Some(head)
                    && matches!(
                        prior.output,
                        Out::Effect {
                            kind: EffectKind::Enqueue,
                            ..
                        }
                    )
                {
                    candidates.push(prior);
                }
            }
        }
        let github = &self.clients[&delivery.repository.installation];
        let comments = self
            .runtime
            .block_on(github.comments(&full_name(&delivery.repository), number))?;
        let mut keys = vec![self.recovery_key(delivery)?];
        keys.extend(candidates.iter().map(|prior| prior.key.clone()));
        for key in keys {
            let marker = GitHub::marker(&key);
            if comments.iter().any(|comment| {
                comment.user.login == format!("{}[bot]", github.app_slug())
                    && comment.body.lines().next() == Some("@mergifyio queue")
                    && comment.body.lines().any(|line| line == marker)
            }) {
                recovery.command = Command::Sent;
                recovery.command_key = key;
                self.save_recovery(delivery, &recovery)?;
                return Ok(recovery);
            }
        }
        for prior in candidates {
            if let Some(attempt) = self.baseline(&prior)? {
                recovery.command = match attempt.requeue {
                    Some(Requeue::Sending) => Command::Sending,
                    Some(Requeue::Refused) => Command::Refused,
                    _ => continue,
                };
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
        let found = !refusal
            && self
                .runtime
                .block_on(github.reconcile_comments(&repo, number, &recovery.command_key))?
                .is_some();
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
    ) -> Result<Reconciliation> {
        let saved = self.recovery(delivery)?;
        if saved.is_none() && !self.repaired_head(delivery, snapshot)? {
            return Ok(Reconciliation::Pending);
        }
        if let Some(mut recovery) = saved
            && let Some(result) = self.reconcile_command(delivery, &mut recovery)?
        {
            return Ok(result);
        }
        let (state, _) = self.read(delivery, snapshot)?;
        if !self.recovery_eligible(delivery, &state, snapshot)? {
            if let Some(mut recovery) = self.recovery(delivery)? {
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
            Some(recovery) if recovery.eligible_since_ms.is_some() => recovery,
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
    pub(super) fn queue_command(
        &mut self,
        delivery: &Delivery,
        recovery_tick_ms: Option<u64>,
    ) -> Result<Reconciliation> {
        let _ = (delivery, recovery_tick_ms);
        todo!("B1 recovery extraction stub")
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
    fn recovery_now_ms(&self) -> u64 {
        self.clock
            .now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
            .min(u64::MAX as u128) as u64
    }
}
fn refused(class: &'static str) -> Reconciliation {
    Reconciliation::Found(Box::new(Completion::permanent(class)))
}
