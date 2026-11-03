//! Recovery of an eligible repaired head after Mergify forgot its queue membership.
use super::*;
use std::time::UNIX_EPOCH;

pub(crate) const PROGRESS_PREFIX: &str = "mergify:awaiting Mergify re-queue";
const GRACE_MS: u64 = 300_000;

#[derive(Clone, Copy, Serialize, Deserialize)]
enum Command {
    Ready,
    Sending,
    Sent,
    Refused,
    PermissionDenied,
    Rejected,
}
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
    fn recovery(&self, delivery: &Delivery) -> Result<Option<Recovery>> {
        let path = self
            .records
            .path(&self.recovery_key(delivery)?)
            .with_extension("requeue");
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
            &self
                .records
                .path(&self.recovery_key(delivery)?)
                .with_extension("requeue"),
            &serde_json::to_vec(recovery)?,
        )
    }
    /// Stored queue facts retain the verified dequeue across later heads and
    /// label removal. Labels alone never establish this history or approval.
    pub(super) fn repaired_head(&self, delivery: &Delivery) -> Result<bool> {
        let Some(store) = &self.store else {
            return Ok(false);
        };
        let snapshot = tl3_store::Store::open(store)?.load()?;
        let world: World = crate::snapshot::decode(&snapshot)?;
        let Some(node) = world.nodes.get(delivery.node().0 as usize) else {
            return Ok(false);
        };
        let (number, head) = enqueue(delivery)?;
        if !heal_phase(Some(node.phase))
            || node.paused
            || node.waiting_incident
            || !node.pr_open
            || node.head != node.approved
            || node.checked != node.approved
            || approved_head(&snapshot, node.approved)? != head
            || crate::request(&snapshot, delivery.node())?.repo != delivery.repository
            || !snapshot.rows.iter().any(|row| {
                row.table == "pull_requests"
                    && row.id == delivery.node().0.to_string()
                    && row.value["number"] == number
            })
        {
            return Ok(false);
        }
        for row in snapshot
            .rows
            .iter()
            .filter(|row| row.table == "queue_facts")
        {
            let fact: QueueFact = crate::decode_stored(row.value.clone())?;
            if fact.repository == delivery.repository
                && fact.number == number
                && matches!(fact.evidence,
                    Evidence::CiFailed { head: ref prior, head_check: false, .. }
                    | Evidence::Conflict { head: ref prior, .. } if prior != head)
            {
                return Ok(true);
            }
        }
        Ok(false)
    }
    fn recovery_eligible(
        &self,
        delivery: &Delivery,
        state: &tl3_mergify::Observation,
    ) -> Result<bool> {
        let (number, head) = enqueue(delivery)?;
        let pull = &state.pull_request;
        if pull.head.sha != head
            || pull.state != "open"
            || pull.merged_at.is_some()
            || pull.draft
            || pull.mergeable != Some(true)
            || !self.repaired_head(delivery)?
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
        let after = self
            .baseline(delivery)?
            .map_or(0, |attempt| attempt.last_event);
        if person_facts(&delivery.repository, number, &events, false)
            .iter()
            .any(|fact| {
                matches!(fact.evidence, Evidence::DequeuedByPerson { event }
                | Evidence::ClosedByPerson { event } if event > after)
            })
        {
            return Ok(false);
        }
        let checks = self
            .runtime
            .block_on(head_checks::read(github, &repo, pull))?;
        Ok(checks.ready && checks.unavailable.is_empty() && !self.reruns_pending(delivery)?)
    }
    pub(super) fn recover_waiting(&mut self, delivery: &Delivery) -> Result<Reconciliation> {
        let mut saved = self.recovery(delivery)?;
        if saved.is_none() && !self.repaired_head(delivery)? {
            return Ok(Reconciliation::Pending);
        }
        if let Some(recovery) = &saved {
            match recovery.command {
                Command::Refused => return Ok(refused("queue_command_refused")),
                Command::PermissionDenied => return Ok(refused("queue_command_permission")),
                Command::Rejected => {
                    return Err(Error::Invalid(
                        "Mergify refused the bounded re-queue command",
                    ));
                }
                Command::Ready | Command::Sending | Command::Sent => {}
            }
        }
        let github = self.clients[&delivery.repository.installation].clone();
        let repo = full_name(&delivery.repository);
        let (number, head) = enqueue(delivery)?;
        if let Some(recovery) = saved.as_mut()
            && matches!(recovery.command, Command::Sending | Command::Sent)
        {
            let mergify = Mergify::new(github.clone());
            if self
                .runtime
                .block_on(mergify.command_refusal(&repo, number, &recovery.command_key))
                .map_err(adapter_error)?
                .is_some()
            {
                recovery.command = Command::Refused;
                self.save_recovery(delivery, recovery)?;
                return Ok(refused("queue_command_refused"));
            }
            if self
                .runtime
                .block_on(github.reconcile_comments(&repo, number, &recovery.command_key))?
                .is_some()
            {
                recovery.command = Command::Sent;
                self.save_recovery(delivery, recovery)?;
            }
            // An absent comment after Sending cannot rule out an in-flight POST.
            return Ok(Reconciliation::Pending);
        }
        let (state, _) = self.read(delivery)?;
        if !self.recovery_eligible(delivery, &state)? {
            if let Some(recovery) = saved.as_mut() {
                recovery.eligible_since_ms = None;
                self.save_recovery(delivery, recovery)?;
            }
            return Ok(Reconciliation::Pending);
        }
        let key = self.recovery_key(delivery)?;
        let now = self
            .clock
            .now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
            .min(u64::MAX as u128) as u64;
        let mut recovery = saved.unwrap_or(Recovery {
            eligible_since_ms: None,
            command: Command::Ready,
            command_key: key,
        });
        // Adopt a legacy dispatch rather than sending a second command on upgrade.
        if self
            .baseline(delivery)?
            .is_some_and(|attempt| matches!(attempt.requeue, Some(Requeue::Sending)))
        {
            recovery.command = Command::Sending;
            recovery.command_key = delivery.key.clone();
        } else if self
            .runtime
            .block_on(github.reconcile_comments(&repo, number, &recovery.command_key))?
            .is_some()
        {
            recovery.command = Command::Sent;
        }
        let since = *recovery.eligible_since_ms.get_or_insert(now);
        self.save_recovery(delivery, &recovery)?;
        if !matches!(recovery.command, Command::Ready) || now.saturating_sub(since) < GRACE_MS {
            return Ok(Reconciliation::Pending);
        }
        // Reread the exact head, proof, required checks, queue policy and human
        // decisions at dispatch. The timer permits one command, never completion.
        let (fresh, _) = self.read(delivery)?;
        if !self.recovery_eligible(delivery, &fresh)? {
            recovery.eligible_since_ms = None;
            self.save_recovery(delivery, &recovery)?;
            return Ok(Reconciliation::Pending);
        }
        self.fence(delivery)?;
        recovery.command = Command::Sending;
        self.save_recovery(delivery, &recovery)?;
        match self.runtime.block_on(Mergify::new(github).requeue(
            &repo,
            number,
            head,
            &recovery.command_key,
        )) {
            Ok(()) => recovery.command = Command::Sent,
            Err(tl3_mergify::Error::GitHub(tl3_github::Error::Http { status: 403 })) => {
                recovery.command = Command::PermissionDenied;
                self.save_recovery(delivery, &recovery)?;
                return Ok(refused("queue_command_permission"));
            }
            Err(error) => {
                if matches!(
                    error,
                    tl3_mergify::Error::GitHub(
                        tl3_github::Error::RateLimited { .. }
                            | tl3_github::Error::Http { status: 400..=499 }
                    )
                ) {
                    recovery.command = Command::Rejected;
                }
                self.save_recovery(delivery, &recovery)?;
                return Err(adapter_error(error));
            }
        }
        self.save_recovery(delivery, &recovery)?;
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
            "{PROGRESS_PREFIX}; PR https://github.com/{}/pull/{number}; head {head}; verified prior dequeue on an earlier head; {status}",
            full_name(&delivery.repository)
        )))
    }
}
fn refused(class: &'static str) -> Reconciliation {
    Reconciliation::Found(Box::new(Completion::permanent(class)))
}
