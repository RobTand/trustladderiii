use super::*;
use crate::mergify_io::io::mock_std as std;
use lifecycle::Requeue;
use tl3_mergify::decide::completion as completion_core;
#[derive(Serialize, Deserialize)]
struct Attempt {
    before: Option<CheckRun>,
    last_event: u64,
    #[serde(default)]
    requeue: Option<Requeue>,
}
impl Attempt {
    fn check_changed(&self, current: &Option<CheckRun>) -> bool {
        completion_core::check_changed(completion_check(&self.before, current))
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
    completion_core::head(current)
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
        if completion_core::reject_queue_pr(is_merge_queue_pr(&state.pull_request)) {
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
        let proof = if proof.is_empty() {
            completion_core::Proof::Empty
        } else if state.pull_request.body.as_deref().unwrap_or_default().contains(&proof) {
            completion_core::Proof::Present
        } else {
            completion_core::Proof::Missing
        };
        if completion_core::proof_missing(proof)
        {
            return Err(Error::Permanent {
                class: "pr_evidence",
                source: "Required recorded proof is missing from the PR body".into(),
            });
        }
        if !completion_core::read_facts(self.heal_body(delivery, &state.pull_request, snapshot)?) {
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
                        return Ok(match completion_core::resolve(result.into(), observed.is_some()) {
                            completion_core::Resolution::Found { ack, conflict, progress } => {
                                if conflict {
                                    observations.push(Observation::Kernel(tl3_kernel::Event::Fact {
                                        node: delivery.node(), fact: tl3_kernel::Fact::Conflict,
                                    }));
                                }
                                let mut completion = match ack {
                                    completion_core::Ack::Confirmed => Completion::confirmed(observations),
                                    completion_core::Ack::Retryable => Completion {
                                        diagnostic: None, ack: Ack::Retryable, failure_class: None,
                                        observations, progress: None,
                                    },
                                    completion_core::Ack::Permanent(failure) => Completion::permanent(failure.class()),
                                };
                                if progress {
                                    completion.progress = observed.as_ref().map(|state|
                                        crate::executor::ProgressToken::Mergify(Progress::from(state)));
                                }
                                Reconciliation::Found(Box::new(completion))
                            }
                            completion_core::Resolution::Pending => Reconciliation::Pending,
                            completion_core::Resolution::Absent => Reconciliation::Absent,
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
                        completion_core::confirmed(completion_receipt(&self.records.reconcile(delivery)?));
                    read = Some(lifecycle::QueueRead {
                        head: lifecycle_head(state.pull_request.head.sha == enqueue(delivery)?.1),
                        queue: state.state,
                        merged: state.pull_request.merged_at.is_some(),
                        conflicts: completion_core::conflicts(state.pull_request.mergeable),
                        held: completion_core::held(state.state, None).map_or_else(
                            || self.unresolved_human_hold(delivery, snapshot), Ok,
                        )?,
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
                    if completion_core::person_events(baseline.is_some(), None).map_or_else(
                        || self.repaired_head(delivery, snapshot), Ok,
                    )? {
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
                            let Some(event) = completion_core::person_event(completion_evidence(&fact.evidence)) else { continue; };
                            let source = if let Some(snapshot) = snapshot {
                                completion_core::Freshness::Snapshot { unresolved: fact.unresolved_hold(snapshot)? }
                            } else {
                                completion_core::Freshness::Intent { last_event: baseline.as_ref().map(|attempt| attempt.last_event) }
                            };
                            let fresh = completion_core::fresh(event, source);
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
                        queue_label: completion_core::has_label(
                            &current.labels.iter().map(|label| completion_label(&label.name)).collect::<Vec<_>>(),
                            completion_core::Label::Queue,
                        ),
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
        let labels: Vec<_> = issue.labels.iter().map(|label| completion_label(&label.name)).collect();
        let retry = completion_core::retry(
            completion_core::has_label(&labels, completion_core::Label::Queue), state.state,
        );
        match completion_core::dequeue_gate(
            retry, state.dequeue_rule_concluded,
            completion_core::has_label(&labels, completion_core::Label::Dequeued),
        ) {
            completion_core::DequeueGate::Done => return Ok(false),
            completion_core::DequeueGate::Pending => return Ok(true),
            completion_core::DequeueGate::Events => {},
        }
        let events = self.runtime.block_on(
            self.clients[&delivery.repository.installation]
                .issue_events(&full_name(&delivery.repository), enqueue(delivery)?.0),
        )?;
        let events: Vec<_> = events.iter().map(|event| completion_core::LabelEvent {
            id: event.id,
            label: event.label.as_ref().map_or(completion_core::Label::Other, |label| completion_label(&label.name)),
            action: if event.event == "labeled" { completion_core::LabelAction::Labeled } else { completion_core::LabelAction::Other },
            at: completion_core::time(event.created_at.timestamp(), event.created_at.timestamp_subsec_nanos()),
        }).collect();
        let label_event = |label| completion_core::newest(&events, label).map(|index| events[index]);
        Ok(lifecycle::dequeue_pending(
            retry,
            state.dequeue_rule_concluded,
            true,
            label_event(completion_core::Label::Dequeued).map(|event| lifecycle::LabelEvent {
                id: event.id,
                labeled: completion_core::labeled(event.action),
                at: event.at,
            }),
            label_event(completion_core::Label::Queued).map(|event| event.id),
            state.check.as_ref().and_then(|check| check.completed_at).map(|time|
                completion_core::time(time.timestamp(), time.timestamp_subsec_nanos())),
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

fn completion_label(name: &str) -> completion_core::Label {
    match name {
        QUEUE_LABEL => completion_core::Label::Queue,
        "queued" => completion_core::Label::Queued,
        "dequeued" => completion_core::Label::Dequeued,
        _ => completion_core::Label::Other,
    }
}

fn completion_receipt(receipt: &Reconciliation) -> completion_core::Receipt {
    match receipt {
        Reconciliation::Found(_) => completion_core::Receipt::Found,
        Reconciliation::Pending => completion_core::Receipt::Pending,
        Reconciliation::Absent => completion_core::Receipt::Absent,
    }
}

fn completion_evidence(evidence: &Evidence) -> completion_core::PersonEvidence {
    match evidence {
        Evidence::DequeuedByPerson { event } => completion_core::PersonEvidence::Dequeued(*event),
        Evidence::ClosedByPerson { event } => completion_core::PersonEvidence::Closed(*event),
        _ => completion_core::PersonEvidence::Other,
    }
}

fn completion_check(before: &Option<CheckRun>, current: &Option<CheckRun>) -> completion_core::CheckComparison {
    match (before, current) {
        (None, None) => completion_core::CheckComparison::BothMissing,
        (Some(before), Some(current)) => completion_core::CheckComparison::BothPresent { fields: [
            before.id == current.id,
            before.pull_requests == current.pull_requests,
            before.name == current.name,
            before.head_sha == current.head_sha,
            before.external_id == current.external_id,
            before.details_url == current.details_url,
            before.status == current.status,
            before.conclusion == current.conclusion,
            before.app == current.app,
            before.output == current.output,
        ] },
        _ => completion_core::CheckComparison::OneMissing,
    }
}

#[cfg(test)]
mod a1_boundary {
    use super::*;

    /// #760: upstream metadata alone never changes acceptance, while every other
    /// field, including unknown payload values, retains the old whole-record equality.
    #[test]
    fn mandatory_a1_full_check_projection() {
        let checks: Vec<CheckRun> = serde_json::from_str(include_str!("../../../mergify/tests/fixtures/waiting/checks.json")).unwrap();
        let original = checks[0].clone();
        let value = serde_json::to_value(&original).unwrap();
        let variants = [
            ("id", json!(u64::MAX)),
            ("pull_requests", json!([{"number":9}])),
            ("name", json!("unknown-check")),
            ("head_sha", json!("other")),
            ("external_id", json!("unknown-external")),
            ("details_url", json!("https://example.invalid/new")),
            ("status", json!("unknown-status")),
            ("conclusion", json!("unknown-conclusion")),
            ("app", json!({"id":999,"slug":"unknown-app"})),
            ("output", json!({"title":"unknown-output","summary":null,"text":null})),
            ("completed_at", json!("2026-11-05T00:00:00Z")),
            ("check_suite", json!({"id":999})),
        ];
        for (field, changed) in variants {
            let mut next = value.clone();
            next[field] = changed;
            let current: CheckRun = serde_json::from_value(next).unwrap();
            let mut normalized = current.clone();
            normalized.completed_at = original.completed_at;
            normalized.check_suite = original.check_suite.clone();
            assert_eq!(completion_core::check_changed(completion_check(&Some(original.clone()), &Some(current))), original != normalized, "{field}");
        }
        assert!(!completion_core::check_changed(completion_check(&None, &None)));
        assert!(completion_core::check_changed(completion_check(&Some(original.clone()), &None)));
        assert!(completion_core::check_changed(completion_check(&None, &Some(original))));
    }

    /// #608/#373: exhaustive lifecycle result conversion, including failure payloads.
    #[test]
    fn mandatory_a1_lifecycle_boundary() {
        for progress in [false, true] {
            for conflict in [false, true] {
                assert_eq!(completion_core::Complete::from(lifecycle::Complete::Confirmed { progress, conflict }),
                    completion_core::Complete::Confirmed { progress, conflict });
            }
        }
        assert_eq!(completion_core::Complete::from(lifecycle::Complete::Retryable), completion_core::Complete::Retryable);
        assert_eq!(completion_core::Complete::from(lifecycle::Complete::Pending), completion_core::Complete::Pending);
        assert_eq!(completion_core::Complete::from(lifecycle::Complete::Absent), completion_core::Complete::Absent);
        for failure in [lifecycle::Failure::Refused, lifecycle::Failure::Permission, lifecycle::Failure::Rejected,
            lifecycle::Failure::DequeuePending, lifecycle::Failure::ChecksPending, lifecycle::Failure::RerunUnknown,
            lifecycle::Failure::RerunFinishing, lifecycle::Failure::PullChanged, lifecycle::Failure::PullClosed, lifecycle::Failure::IntentMissing] {
            assert_eq!(completion_core::Complete::from(lifecycle::Complete::Permanent(failure)), completion_core::Complete::Permanent(failure));
        }
        for (name, label) in [(QUEUE_LABEL, completion_core::Label::Queue), ("queued", completion_core::Label::Queued),
            ("dequeued", completion_core::Label::Dequeued), ("other", completion_core::Label::Other)] {
            assert_eq!(completion_label(name), label);
        }
        assert_eq!(completion_evidence(&Evidence::DequeuedByPerson { event: 9 }), completion_core::PersonEvidence::Dequeued(9));
        assert_eq!(completion_evidence(&Evidence::ClosedByPerson { event: 10 }), completion_core::PersonEvidence::Closed(10));
        assert_eq!(completion_evidence(&Evidence::PushByOther { head: "other".into() }), completion_core::PersonEvidence::Other);
        assert_eq!(completion_receipt(&Reconciliation::Found(Box::new(Completion::permanent("test")))), completion_core::Receipt::Found);
        assert_eq!(completion_receipt(&Reconciliation::Pending), completion_core::Receipt::Pending);
        assert_eq!(completion_receipt(&Reconciliation::Absent), completion_core::Receipt::Absent);
    }
}
