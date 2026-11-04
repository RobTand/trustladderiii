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
    fn read(
        &self,
        delivery: &Delivery,
        snapshot: Option<&Snapshot>,
    ) -> Result<(tl3_mergify::Observation, Vec<Observation>)> {
        let (number, head) = enqueue(delivery)?;
        let github = self
            .clients
            .get(&delivery.repository.installation)
            .ok_or(Error::Invalid("missing Mergify installation client"))?;
        let repo = full_name(&delivery.repository);
        let state = self
            .runtime
            .block_on(read_queue(github, &repo, number, head))?;
        if is_merge_queue_pr(&state.pull_request) {
            return Err(Error::Invalid("cannot queue a Mergify draft PR"));
        }
        crate::evidence::validate_publication(
            delivery,
            &state.pull_request.head.branch,
            state.pull_request.draft,
            state.pull_request.body.as_deref().unwrap_or_default(),
        )?;
        let proof =
            crate::evidence::publication(delivery, delivery.request_text.as_deref().unwrap_or(""))?;
        if !proof.is_empty()
            && !state
                .pull_request
                .body
                .as_deref()
                .unwrap_or_default()
                .contains(&proof)
        {
            return Err(Error::Permanent {
                class: "pr_evidence",
                source: "Required recorded proof is missing from the PR body".into(),
            });
        }
        if self.heal_body(delivery, &state.pull_request, snapshot)? {
            // The checks read before the edit are stale; the next read decides.
            return Ok((state, Vec::new()));
        }
        let facts = self
            .runtime
            .block_on(queue_facts(
                github,
                &delivery.repository,
                &state,
                head,
                &[],
                snapshot,
            ))?
            .into_iter()
            .map(Observation::Queue)
            .collect();
        Ok((state, facts))
    }
    fn heal_body(
        &self,
        delivery: &Delivery,
        pull: &tl3_github::PullRequest,
        snapshot: Option<&Snapshot>,
    ) -> Result<bool> {
        let Some((_, policy)) = self
            .policies
            .iter()
            .find(|(repo, _)| *repo == delivery.repository)
        else {
            return Ok(false);
        };
        let rules = &policy.rules;
        if pull.state != "open" || pull.merged_at.is_some() {
            return Ok(false);
        }
        let ours = match &self.branches {
            Some(branches) => branches.matches(delivery, &pull.head.branch)?,
            None => false,
        };
        if !ours {
            return Ok(false);
        }
        let github = &self.clients[&delivery.repository.installation];
        let repo = full_name(&delivery.repository);
        let head = &pull.head.sha;
        // Keyed by PR, not by effect, so a later enqueue of the same head
        // finds the heal and never reruns twice.
        let path = self
            .records
            .path(&format!("body-heal:{repo}#{}", pull.number))
            .with_extension("body-heal");
        let mut heal: Option<BodyHeal> = match std::fs::read(&path) {
            Ok(bytes) => Some(serde_json::from_slice(&bytes)?),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(error.into()),
        };
        let body = pull.body.as_deref().unwrap_or_default();
        let gaps = crate::pr_body::gaps(body, rules);
        let pending = heal
            .as_ref()
            .is_some_and(|heal| heal.head == *head && !heal.reran);
        // Read the world only when there is something to do.
        if (gaps.is_empty() && !pending) || !heal_phase(Self::phase(delivery, snapshot)?) {
            return Ok(false);
        }
        let mut edited = false;
        let request = if gaps.is_empty() {
            None
        } else {
            // The issue's own instruction names the value; read it only to heal.
            match &delivery.request_text {
                Some(text) => Some(text.clone()),
                None => {
                    self.runtime
                        .block_on(github.issue(&repo, delivery.issue))?
                        .body
                }
            }
        };
        if let Some(healed) =
            crate::pr_body::healed(body, rules, request.as_deref(), &delivery.paths)
        {
            // Record the intent first, so a crash after the PATCH still reruns.
            let intent = BodyHeal {
                head: head.clone(),
                reran: false,
            };
            self.records.write(&path, &serde_json::to_vec(&intent)?)?;
            self.runtime
                .block_on(github.update_pull_request_body(&repo, pull.number, &healed))?;
            eprintln!(
                "PR-body policy: added {gaps:?} to {repo}#{} (#447)",
                pull.number
            );
            heal = Some(intent);
            edited = true;
        }
        // The edit's completion triggers the reruns, once per head. Closing and
        // reopening the PR would also rerun them, but TL3 would read it as a
        // person closing its PR.
        let Some(heal) = heal.filter(|heal| heal.head == *head && !heal.reran) else {
            return Ok(edited);
        };
        let runs = if policy.reruns.is_empty() {
            Vec::new()
        } else {
            self.runtime
                .block_on(github.workflow_runs(&repo, head))
                .map_err(Error::from)?
        };
        for run in body_edit_reruns(&runs, head, &policy.reruns) {
            if let Err(error) = self.runtime.block_on(github.rerun_failed_jobs(&repo, run)) {
                // The next read of this PR retries; the heal stays pending.
                eprintln!("PR-body policy: rerun of {repo} run {run} failed: {error}");
                return Ok(edited);
            }
            eprintln!("PR-body policy: reran the failed jobs of {repo} run {run} (#447)");
        }
        self.records.write(
            &path,
            &serde_json::to_vec(&BodyHeal {
                head: heal.head,
                reran: true,
            })?,
        )?;
        Ok(edited)
    }
    fn phase(
        delivery: &Delivery,
        snapshot: Option<&Snapshot>,
    ) -> Result<Option<tl3_kernel::Phase>> {
        let Some(snapshot) = snapshot else {
            return Ok(None);
        };
        let world = crate::snapshot::decode(snapshot)?;
        Ok(world
            .nodes
            .get(delivery.node().0 as usize)
            .map(|node| node.phase))
    }

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

async fn read_queue(
    github: &GitHub,
    repo: &str,
    number: u64,
    head: &str,
) -> Result<tl3_mergify::Observation> {
    let mergify = Mergify::new(github.clone());
    let mut retries = 2;
    loop {
        match mergify.read(repo, number, head).await {
            Err(tl3_mergify::Error::ChangedDuringRead) if retries > 0 => retries -= 1,
            result => return result.map_err(adapter_error),
        }
    }
}
async fn queue_facts(
    github: &GitHub,
    repo: &Repository,
    state: &tl3_mergify::Observation,
    expected_head: &str,
    open: &[RequiredCheck],
    snapshot: Option<&Snapshot>,
) -> Result<Vec<QueueFact>> {
    let pull = &state.pull_request;
    let head_checks = match facts::entry(
        is_merge_queue_pr(pull),
        state.state,
        pull.head.sha == expected_head,
        pull.state == "open",
    ) {
        facts::Entry::Ignore => return Ok(vec![]),
        facts::Entry::PushByOther => {
            return Ok(vec![QueueFact {
                repository: repo.clone(),
                number: pull.number,
                evidence: Evidence::PushByOther {
                    head: pull.head.sha.clone(),
                },
            }]);
        }
        facts::Entry::Read { head_checks } => head_checks,
    };
    let mut facts = Vec::new();
    let mut head_infrastructure = None;
    if head_checks {
        let repo_name = full_name(repo);
        let requirements = github
            .required_status_checks(&repo_name, &pull.base.branch)
            .await?;
        let checks =
            head_checks::evaluate(github, &repo_name, pull, requirements, &state.check_runs)
                .await?;
        head_infrastructure = checks.infrastructure();
        if !checks.unavailable.is_empty() {
            facts.push(QueueFact {
                repository: repo.clone(),
                number: pull.number,
                evidence: Evidence::ChecksUnavailable {
                    permissions: checks.unavailable.clone(),
                },
            });
        }
        // Q52: once every suite on the head has completed, no app is still
        // working on it, so a missing required context will not report.
        if !checks.missing.is_empty() {
            let suites = github.check_suites(&repo_name, &pull.head.sha).await?;
            if facts::suites_completed(&suite_statuses(&suites)) {
                facts.extend(checks.missing.iter().map(|required| QueueFact {
                    repository: repo.clone(),
                    number: pull.number,
                    evidence: Evidence::CheckMissing {
                        head: pull.head.sha.clone(),
                        required: required.clone(),
                    },
                }));
            }
        }
        // #447: Mergify waits on a condition nothing still running can meet,
        // such as PrismaBuild's scopes after its pr-gate failed.
        if state.state == QueueState::Waiting
            && checks_settled(state)
            && state
                .check
                .as_ref()
                .is_some_and(|check| !tl3_mergify::unmet_conditions(check).is_empty())
        {
            let suites = github.check_suites(&repo_name, &pull.head.sha).await?;
            let completed = facts::suites_completed(&suite_statuses(&suites));
            let required: Vec<_> = checks
                .missing
                .iter()
                .chain(&checks.reported)
                .cloned()
                .collect();
            if let Some((conditions, failed)) = queue_blocked(state, completed, &required) {
                facts.push(QueueFact {
                    repository: repo.clone(),
                    number: pull.number,
                    evidence: Evidence::QueueBlocked {
                        head: pull.head.sha.clone(),
                        conditions,
                        failed,
                    },
                });
            }
        }
        facts.extend(
            open.iter()
                .filter(|required| checks.reported.contains(required))
                .map(|required| QueueFact {
                    repository: repo.clone(),
                    number: pull.number,
                    evidence: Evidence::CheckReported {
                        head: pull.head.sha.clone(),
                        required: required.clone(),
                    },
                }),
        );
        if let Some(evidence) = checks.evidence(pull) {
            let mut fact = QueueFact {
                repository: repo.clone(),
                number: pull.number,
                evidence,
            };
            if !snapshot
                .map(|snapshot| fact.recorded(snapshot))
                .transpose()?
                .unwrap_or(false)
            {
                if let Evidence::CiFailed { details, .. } = &mut fact.evidence {
                    *details =
                        ci_evidence::read_checks(github, &repo_name, &state.check_runs).await;
                }
                facts.push(fact);
            }
            return Ok(facts);
        }
    }
    let outcome = facts::queue_outcome(
        state.state,
        state.check.as_ref(),
        head_infrastructure,
        tl3_mergify::ci_is_infrastructure,
    );
    let attempt = |check: &CheckRun| (check.id, check.completed_at.map(|time| time.to_rfc3339()));
    let evidence = match outcome {
        Some(facts::Outcome::Merged) => {
            let queue_check = state.check.as_ref().map(|check| facts::QueueCheck {
                completed: check.status == "completed",
                concluded: check.conclusion.is_some(),
                merged_title: check.output.as_ref().is_some_and(|output| {
                    output.title.as_deref() == Some("Merged via merge queue")
                }),
            });
            let checks_passed = match facts::merged_plan(
                pull.head.sha == expected_head,
                queue_check,
                pull.merge_commit_sha.is_some(),
            ) {
                // Mergify publishes its final queue result after GitHub records the merge.
                facts::MergedPlan::Wait => return Ok(vec![]),
                facts::MergedPlan::ViaQueue => tl3_mergify::merged_via_queue(state),
                facts::MergedPlan::MergeCommit => {
                    let sha = pull.merge_commit_sha.as_deref().unwrap_or_default();
                    checks_passed(&github.check_runs(&full_name(repo), sha).await?, sha)
                }
                facts::MergedPlan::NotPassed => false,
            };
            Some(Evidence::Merged {
                head: pull.head.sha.clone(),
                checks_passed,
            })
        }
        Some(facts::Outcome::CiFailed { check, infra }) => {
            let (check, completed_at) = attempt(check);
            Some(Evidence::CiFailed {
                check,
                completed_at,
                head: pull.head.sha.clone(),
                head_check: false,
                details: Default::default(),
                infra,
            })
        }
        Some(facts::Outcome::DequeuedByCommand(check)) => {
            let (check, completed_at) = attempt(check);
            Some(Evidence::DequeuedByCommand {
                check,
                completed_at,
                head: pull.head.sha.clone(),
            })
        }
        Some(facts::Outcome::Conflict(check)) => {
            let (check, completed_at) = attempt(check);
            Some(Evidence::Conflict {
                check,
                completed_at,
                head: pull.head.sha.clone(),
            })
        }
        None => None,
    };
    if let Some(evidence) = evidence {
        let mut fact = QueueFact {
            repository: repo.clone(),
            number: pull.number,
            evidence,
        };
        if !snapshot
            .map(|snapshot| fact.recorded(snapshot))
            .transpose()?
            .unwrap_or(false)
        {
            if let Evidence::CiFailed { details, .. } = &mut fact.evidence {
                *details = ci_evidence::read_queue(github, &full_name(repo), state).await;
            }
            facts.push(fact);
        }
    }
    Ok(facts)
}
pub(crate) fn queue_blocked(
    state: &tl3_mergify::Observation,
    suites_completed: bool,
    required: &[RequiredCheck],
) -> Option<(Vec<String>, Vec<String>)> {
    let conditions = if state.state == QueueState::Waiting && suites_completed {
        tl3_mergify::unmet_conditions(state.check.as_ref()?)
    } else {
        vec![]
    };
    if !facts::queue_blocked(state.state, suites_completed, !conditions.is_empty()) {
        return None;
    }
    let views = tl3_mergify::views(&state.check_runs, &state.pull_request.head.sha, |check| {
        required.iter().any(|rule| rule.matches(check))
    });
    // Each failing name is the newest of its name, so sorting gives the
    // former BTreeMap's order by name.
    let mut failed: Vec<String> = (0..views.len())
        .filter(|&index| facts::blocking_failure(&views, index))
        .map(|index| state.check_runs[index].name.clone())
        .collect();
    failed.sort();
    Some((conditions, failed))
}
pub(crate) fn body_edit_reruns(
    runs: &[tl3_github::WorkflowRun],
    head: &str,
    workflows: &[String],
) -> Vec<u64> {
    let listed = |path: &str| {
        let file = path.rsplit('/').next().unwrap_or(path);
        workflows.iter().any(|listed| listed == file)
    };
    let mut latest: BTreeMap<&str, &tl3_github::WorkflowRun> = BTreeMap::new();
    for run in runs
        .iter()
        .filter(|run| run.head_sha == head && run.event == "pull_request" && listed(&run.path))
    {
        let entry = latest.entry(run.path.as_str()).or_insert(run);
        if run.id > entry.id {
            *entry = run;
        }
    }
    latest
        .into_values()
        .filter(|run| {
            run.status == "completed"
                && matches!(
                    run.conclusion.as_deref(),
                    Some("failure" | "cancelled" | "timed_out" | "startup_failure")
                )
        })
        .map(|run| run.id)
        .collect()
}
pub(crate) fn heal_phase(phase: Option<tl3_kernel::Phase>) -> bool {
    matches!(
        phase,
        Some(tl3_kernel::Phase::Publishing | tl3_kernel::Phase::Queued)
    )
}
fn suite_statuses(suites: &[tl3_github::CheckSuite]) -> Vec<tl3_mergify::decide::Status> {
    suites
        .iter()
        .map(|suite| tl3_mergify::decide::Status::parse(&suite.status))
        .collect()
}
fn checks_passed(checks: &[CheckRun], sha: &str) -> bool {
    facts::checks_passed(&tl3_mergify::views(checks, sha, |_| false))
}

#[derive(Serialize, Deserialize)]
struct BodyHeal { head: String, reran: bool }
