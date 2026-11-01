mod head_checks;

use super::record::Records;
use crate::executor::Output as Out;
use crate::{
    Result,
    executor::{Completion, Delivery, Executor, Observation, Reconciliation},
};
use std::path::Path;
use tl3_kernel::{Ack, EffectKind};

pub struct Fake {
    records: Records,
    hold_merges: bool,
}
impl Fake {
    pub fn new(root: &Path) -> Self {
        Self {
            records: Records::new(root),
            hold_merges: root.join("hold-merges").exists(),
        }
    }
}
impl Executor for Fake {
    fn execute(&mut self, delivery: &Delivery) -> Result<Completion> {
        let node = delivery.node();
        if delivery.pull_request.is_none() {
            return Err(crate::Error::Invalid("enqueue needs a PR number"));
        }
        let observations = match delivery.output {
            Out::Effect {
                kind: EffectKind::Enqueue,
                ..
            } => vec![Observation::Merged {
                node,
                sha: delivery
                    .head_sha
                    .clone()
                    .ok_or(crate::Error::Invalid("queue needs a head"))?,
                checks_passed: true,
            }],
            _ => return Err(crate::Error::Invalid("unsupported Mergify effect")),
        };
        self.records.deliver(
            delivery,
            Completion::confirmed(if self.hold_merges {
                vec![]
            } else {
                observations
            }),
        )
    }
    fn reconcile(&mut self, delivery: &Delivery) -> Result<Reconciliation> {
        self.records.reconcile(delivery)
    }
}

use crate::{
    Error, Repository,
    cli::Input,
    ingress::{GitHubDelivery, GitHubTranslator},
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use tl3_github::{Actor, CheckRun, GitHub, RepoChanges, webhook::Event};
use tl3_kernel::{NodeId, World};
use tl3_mergify::{Mergify, QUEUE_LABEL, QueueState, is_merge_queue_pr};
use tl3_store::{InternKind, Snapshot};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct QueueFact {
    pub repository: Repository,
    pub number: u64,
    pub evidence: Evidence,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Evidence {
    ChecksUnavailable {
        permissions: Vec<String>,
    },
    Merged {
        head: String,
        checks_passed: bool,
    },
    CiFailed {
        check: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        completed_at: Option<String>,
        head: String,
        infra: bool,
        #[serde(default)]
        head_check: bool,
    },
    Conflict {
        check: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        completed_at: Option<String>,
        head: String,
    },
    DequeuedByCommand {
        check: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        completed_at: Option<String>,
        head: String,
    },
    DequeuedByPerson {
        event: u64,
    },
    ClosedByPerson {
        event: u64,
    },
    PushByOther {
        head: String,
    },
}
impl QueueFact {
    pub(crate) fn node(&self, snapshot: &Snapshot) -> Result<Option<NodeId>> {
        for row in snapshot.rows.iter().filter(|row| {
            row.table == "pull_requests" && row.value["number"].as_u64() == Some(self.number)
        }) {
            let node = NodeId(
                row.id
                    .parse()
                    .map_err(|_| Error::Invalid("invalid PR binding"))?,
            );
            if crate::request(snapshot, node)?.repo == self.repository {
                return Ok(Some(node));
            }
        }
        Ok(None)
    }
    pub(crate) fn needs_queue_ack(&self) -> bool {
        matches!(
            self.evidence,
            Evidence::CiFailed {
                head_check: false,
                ..
            } | Evidence::Conflict { .. }
                | Evidence::DequeuedByPerson { .. }
                | Evidence::DequeuedByCommand { .. }
        )
    }
    pub(crate) fn identity(&self) -> String {
        let mut evidence = match &self.evidence {
            Evidence::ChecksUnavailable { .. } => "required-checks-setup".into(),
            Evidence::Merged { head, .. } => format!("merged:{head}"),
            Evidence::CiFailed {
                check, head_check, ..
            } => {
                format!("{}:{check}", if *head_check { "head-ci" } else { "ci" })
            }
            Evidence::Conflict { check, .. } => format!("conflict:{check}"),
            Evidence::DequeuedByCommand { check, .. } => format!("command-dequeued:{check}"),
            Evidence::DequeuedByPerson { event } => format!("dequeued:{event}"),
            Evidence::ClosedByPerson { event } => format!("closed:{event}"),
            Evidence::PushByOther { head } => format!("push:{head}"),
        };
        // Mergify can reuse one check ID across successive queue attempts.
        if let Evidence::CiFailed {
            completed_at: Some(time),
            ..
        }
        | Evidence::Conflict {
            completed_at: Some(time),
            ..
        }
        | Evidence::DequeuedByCommand {
            completed_at: Some(time),
            ..
        } = &self.evidence
        {
            evidence.push(':');
            evidence.push_str(time);
        }
        format!(
            "{}/{}/{}/{}/{evidence}",
            self.repository.owner.to_ascii_lowercase(),
            self.repository.name.to_ascii_lowercase(),
            self.repository.installation,
            self.number
        )
    }
    fn input(self) -> Input {
        Input::Observation {
            key: format!("mergify:{}", self.identity()),
            observation: Observation::Queue(self),
        }
    }
}

/// Authoritative PR and selected queue-check values from one reconciliation read.
/// A missing check is distinct from an unfinished check. This token sets no deadline.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Progress {
    /// GitHub's merge time in RFC 3339 format.
    pub merged_at: Option<String>,
    pub merge_commit_sha: Option<String>,
    pub queue_check: Option<QueueCheckProgress>,
}

/// The newest Mergify queue check on the PR's current head.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct QueueCheckProgress {
    pub id: u64,
    pub status: String,
    pub conclusion: Option<String>,
    pub title: Option<String>,
}

impl From<&tl3_mergify::Observation> for Progress {
    fn from(state: &tl3_mergify::Observation) -> Self {
        Self {
            merged_at: state.pull_request.merged_at.map(|time| time.to_rfc3339()),
            merge_commit_sha: state.pull_request.merge_commit_sha.clone(),
            queue_check: state.check.as_ref().map(|check| QueueCheckProgress {
                id: check.id,
                status: check.status.clone(),
                conclusion: check.conclusion.clone(),
                title: check
                    .output
                    .as_ref()
                    .and_then(|output| output.title.clone()),
            }),
        }
    }
}

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

pub struct Adapter {
    clients: BTreeMap<u64, GitHub>,
    epoch: i64,
    records: Records,
    runtime: &'static tokio::runtime::Runtime,
}
impl Adapter {
    pub fn new(clients: BTreeMap<u64, GitHub>, epoch: i64, records: &Path) -> Result<Self> {
        std::fs::create_dir_all(records)?;
        Ok(Self {
            clients,
            epoch,
            records: Records::new(records),
            runtime: crate::executor::runtime()?,
        })
    }
    fn fence(&mut self, delivery: &Delivery) -> Result<()> {
        if delivery.epoch < self.epoch {
            return Err(Error::Invalid("Mergify effect epoch is fenced"));
        }
        self.records.fence(delivery.epoch)?;
        self.epoch = delivery.epoch;
        Ok(())
    }
    fn read(&self, delivery: &Delivery) -> Result<(tl3_mergify::Observation, Vec<Observation>)> {
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
        let facts = self
            .runtime
            .block_on(queue_facts(github, &delivery.repository, &state, head))?
            .into_iter()
            .map(Observation::Queue)
            .collect();
        Ok((state, facts))
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
    fn completion(&self, delivery: &Delivery) -> Result<Reconciliation> {
        let mut baseline = self.baseline(delivery)?;
        if baseline
            .as_ref()
            .is_some_and(|attempt| matches!(attempt.requeue, Some(Requeue::Refused)))
        {
            return Ok(Reconciliation::Found(Completion::permanent(
                "queue_command_refused",
            )));
        }
        let (state, mut observations) = self.read(delivery)?;
        let previously_confirmed =
            matches!(self.records.reconcile(delivery)?, Reconciliation::Found(_));
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
                self.records.write(
                    &self.records.path(&delivery.key).with_extension("intent"),
                    &serde_json::to_vec(attempt)?,
                )?;
                return Ok(Reconciliation::Found(Completion::permanent(
                    "queue_command_refused",
                )));
            }
        }
        if !previously_confirmed && self.reruns_pending(delivery)? {
            return Ok(Reconciliation::Pending);
        }
        let current_head = state.pull_request.head.sha == enqueue(delivery)?.1;
        if !current_head && state.state != QueueState::Merged && !previously_confirmed {
            return Ok(Reconciliation::Found(Completion {
                ack: Ack::Retryable,
                failure_class: None,
                observations,
                progress: None,
            }));
        }
        if current_head
            && state.pull_request.merged_at.is_none()
            && state.pull_request.mergeable == Some(false)
        {
            observations.push(Observation::Kernel(tl3_kernel::Event::Fact {
                node: delivery.node(),
                fact: tl3_kernel::Fact::Conflict,
            }));
            return Ok(Reconciliation::Found(Completion::confirmed(observations)));
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
        if let Some(attempt) = &baseline {
            let github = &self.clients[&delivery.repository.installation];
            let events = self.runtime.block_on(
                github.issue_events(&full_name(&delivery.repository), enqueue(delivery)?.0),
            )?;
            let person_observations: Vec<_> = person_facts(
                &delivery.repository,
                enqueue(delivery)?.0,
                &events,
                state.pull_request.merged_at.is_some(),
            )
            .into_iter()
            .filter(|fact| match fact.evidence {
                Evidence::DequeuedByPerson { event } | Evidence::ClosedByPerson { event } => {
                    event > attempt.last_event
                }
                _ => false,
            })
            .map(Observation::Queue)
            .collect();
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
            return Ok(Reconciliation::Found(completion));
        }
        let github = &self.clients[&delivery.repository.installation];
        let (number, _) = enqueue(delivery)?;
        let issue = self
            .runtime
            .block_on(github.issue(&full_name(&delivery.repository), number))?;
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
                if matches!(requeue, Requeue::Sending)
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
}
impl Executor for Adapter {
    fn app_login(&self, installation: u64) -> Option<String> {
        self.clients
            .get(&installation)
            .map(|github| format!("{}[bot]", github.app_slug()))
    }
    fn probe(&mut self, delivery: &Delivery) -> Result<bool> {
        let (number, _) = enqueue(delivery)?;
        let github = self
            .clients
            .get(&delivery.repository.installation)
            .ok_or(Error::Invalid("missing Mergify installation client"))?;
        self.runtime
            .block_on(github.issue(&full_name(&delivery.repository), number))?;
        Ok(true)
    }
    fn observations(&mut self, delivery: &Delivery) -> Result<Vec<Observation>> {
        let (_, observations) = self.read(delivery)?;
        Ok(observations
            .into_iter()
            .filter(|observation| {
                matches!(
                    observation,
                    Observation::Queue(QueueFact {
                        evidence: Evidence::CiFailed {
                            head_check: true,
                            ..
                        } | Evidence::ChecksUnavailable { .. },
                        ..
                    })
                )
            })
            .collect())
    }
    fn reconcile_cancellation(&mut self, delivery: &Delivery) -> Result<Reconciliation> {
        let (number, _) = enqueue(delivery)?;
        let github = &self.clients[&delivery.repository.installation];
        let repo = full_name(&delivery.repository);
        let issue = self.runtime.block_on(github.issue(&repo, number))?;
        if issue.labels.iter().any(|label| label.name == QUEUE_LABEL) {
            self.runtime
                .block_on(github.remove_label(&repo, number, QUEUE_LABEL))?;
        }
        Ok(Reconciliation::Found(Completion::confirmed(vec![])))
    }
    fn retire_delivery(&mut self, delivery: &Delivery) -> Result<()> {
        for extension in ["intent", "applied", "reruns", "json", "tmp"] {
            match std::fs::remove_file(self.records.path(&delivery.key).with_extension(extension)) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
        }
        std::fs::File::open(self.records.path(&delivery.key).parent().unwrap())?.sync_all()?;
        Ok(())
    }
    fn execute(&mut self, delivery: &Delivery) -> Result<Completion> {
        self.fence(delivery)?;
        match self.completion(delivery)? {
            Reconciliation::Found(completion) => {
                if completion.ack == Ack::Confirmed
                    && !matches!(self.records.reconcile(delivery)?, Reconciliation::Found(_))
                {
                    self.records
                        .deliver(delivery, Completion::confirmed(vec![]))?;
                }
                return Ok(completion);
            }
            Reconciliation::Pending => {
                return Err(Error::Invalid("Mergify acceptance is still unknown"));
            }
            Reconciliation::Absent => {}
        }
        let (state, _) = self.read(delivery)?;
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
            return Err(Error::Invalid("required head checks are still pending"));
        }
        let mut attempt = match self.baseline(delivery)? {
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
        let mergify = Mergify::new(github);
        let result = if attempt.requeue.is_some() {
            // A crash or lost response cannot prove the command was not accepted.
            attempt.requeue = Some(Requeue::Sending);
            self.records.write(&path, &serde_json::to_vec(&attempt)?)?;
            let result = self
                .runtime
                .block_on(mergify.requeue(&repo, number, head, &delivery.key));
            if matches!(
                &result,
                Err(tl3_mergify::Error::GitHub(
                    tl3_github::Error::RateLimited { .. }
                    | tl3_github::Error::Http {
                        status: 400..=407 | 409..=499,
                    },
                ))
            ) {
                attempt.requeue = Some(Requeue::Ready);
                self.records.write(&path, &serde_json::to_vec(&attempt)?)?;
            }
            result
        } else {
            self.runtime.block_on(mergify.queue(&repo, number, head))
        };
        match result {
            Ok(()) => {}
            Err(error @ tl3_mergify::Error::GitHub(tl3_github::Error::Http { status: 403 }))
                if attempt.requeue.is_some() =>
            {
                return Err(Error::Permanent {
                    class: "queue_command_permission",
                    source: Box::new(error),
                });
            }
            Err(tl3_mergify::Error::HeadChanged) => {
                return match self.completion(delivery)? {
                    Reconciliation::Found(completion) => Ok(completion),
                    _ => Err(Error::Invalid("PR changed during enqueue")),
                };
            }
            Err(error) => return Err(adapter_error(error)),
        }
        self.records.write(
            &self.records.path(&delivery.key).with_extension("applied"),
            b"true",
        )?;
        match self.completion(delivery)? {
            Reconciliation::Found(completion) => {
                self.records
                    .deliver(delivery, Completion::confirmed(vec![]))?;
                Ok(completion)
            }
            _ => Err(Error::Invalid(
                "enqueue request sent; Mergify acceptance is still unknown",
            )),
        }
    }
    fn reconcile(&mut self, delivery: &Delivery) -> Result<Reconciliation> {
        // Reads retain the original effect's epoch, including after a controller upgrade.
        match self.completion(delivery) {
            Ok(Reconciliation::Found(completion)) => {
                if completion.ack == Ack::Confirmed
                    && !matches!(self.records.reconcile(delivery)?, Reconciliation::Found(_))
                {
                    // This receipt writes no upstream state and may name an older epoch.
                    self.records
                        .remember(delivery, Completion::confirmed(vec![]))?;
                }
                Ok(Reconciliation::Found(completion))
            }
            Ok(result) => Ok(result),
            Err(error) if error.is_github_not_found() => Err(error),
            Err(_) => Ok(Reconciliation::Pending),
        }
    }
}
fn enqueue(delivery: &Delivery) -> Result<(u64, &str)> {
    if !matches!(
        delivery.output,
        Out::Effect {
            kind: EffectKind::Enqueue,
            ..
        }
    ) {
        return Err(Error::Invalid("unsupported Mergify effect"));
    }
    Ok((
        delivery
            .pull_request
            .ok_or(Error::Invalid("enqueue needs a PR number"))?,
        delivery
            .head_sha
            .as_deref()
            .ok_or(Error::Invalid("enqueue needs an approved head"))?,
    ))
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

fn adapter_error(error: tl3_mergify::Error) -> Error {
    match error {
        tl3_mergify::Error::GitHub(error) => error.into(),
        error => Error::Adapter(Box::new(error)),
    }
}
fn full_name(repo: &Repository) -> String {
    format!("{}/{}", repo.owner, repo.name)
}

async fn queue_facts(
    github: &GitHub,
    repo: &Repository,
    state: &tl3_mergify::Observation,
    expected_head: &str,
) -> Result<Vec<QueueFact>> {
    let pull = &state.pull_request;
    if is_merge_queue_pr(pull) {
        return Ok(vec![]);
    }
    if state.state != QueueState::Merged && pull.head.sha != expected_head {
        return Ok(vec![QueueFact {
            repository: repo.clone(),
            number: pull.number,
            evidence: Evidence::PushByOther {
                head: pull.head.sha.clone(),
            },
        }]);
    }
    let mut facts = Vec::new();
    let mut head_infrastructure = None;
    if pull.state == "open"
        && matches!(
            state.state,
            QueueState::Waiting | QueueState::Unknown | QueueState::FailedCi | QueueState::Dequeued
        )
    {
        let checks = head_checks::read(github, &full_name(repo), pull).await?;
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
        if let Some(evidence) = checks.evidence(pull) {
            facts.push(QueueFact {
                repository: repo.clone(),
                number: pull.number,
                evidence,
            });
            return Ok(facts);
        }
    }
    let evidence = match state.state {
        QueueState::Merged => {
            // Mergify publishes its final queue result after GitHub records the merge.
            if pull.head.sha == expected_head
                && state
                    .check
                    .as_ref()
                    .is_some_and(|check| check.status != "completed" || check.conclusion.is_none())
            {
                return Ok(vec![]);
            }
            let checks_passed = if state.check.as_ref().is_some_and(|check| {
                check
                    .output
                    .as_ref()
                    .is_some_and(|output| output.title.as_deref() == Some("Merged via merge queue"))
            }) {
                tl3_mergify::merged_via_queue(state)
            } else if let Some(sha) = &pull.merge_commit_sha {
                checks_passed(&github.check_runs(&full_name(repo), sha).await?, sha)
            } else {
                false
            };
            Some(Evidence::Merged {
                head: pull.head.sha.clone(),
                checks_passed,
            })
        }
        QueueState::FailedCi => state.check.as_ref().map(|check| Evidence::CiFailed {
            check: check.id,
            completed_at: check.completed_at.map(|time| time.to_rfc3339()),
            head: pull.head.sha.clone(),
            head_check: false,
            infra: head_infrastructure.unwrap_or_else(|| tl3_mergify::ci_is_infrastructure(check)),
        }),
        QueueState::DequeuedByPerson => {
            state
                .check
                .as_ref()
                .map(|check| Evidence::DequeuedByCommand {
                    check: check.id,
                    completed_at: check.completed_at.map(|time| time.to_rfc3339()),
                    head: pull.head.sha.clone(),
                })
        }
        QueueState::Dequeued => state.check.as_ref().map(|check| Evidence::CiFailed {
            check: check.id,
            completed_at: check.completed_at.map(|time| time.to_rfc3339()),
            head: pull.head.sha.clone(),
            infra: head_infrastructure.unwrap_or(true),
            head_check: false,
        }),
        QueueState::Conflict => state.check.as_ref().map(|check| Evidence::Conflict {
            check: check.id,
            completed_at: check.completed_at.map(|time| time.to_rfc3339()),
            head: pull.head.sha.clone(),
        }),
        _ => None,
    };
    facts.extend(evidence.into_iter().map(|evidence| QueueFact {
        repository: repo.clone(),
        number: pull.number,
        evidence,
    }));
    Ok(facts)
}
fn checks_passed(checks: &[CheckRun], sha: &str) -> bool {
    let mut latest = BTreeMap::new();
    for check in checks.iter().filter(|check| check.head_sha == sha) {
        let key = (check.app.as_ref().map(|app| app.id), &check.name);
        let entry = latest.entry(key).or_insert(check);
        if check.id > entry.id {
            *entry = check;
        }
    }
    !latest.is_empty()
        && latest.values().all(|check| {
            check.status == "completed"
                && matches!(
                    check.conclusion.as_deref(),
                    Some("success" | "skipped" | "neutral")
                )
        })
}

fn approved_head(snapshot: &Snapshot, approved: Option<tl3_kernel::HeadId>) -> Result<String> {
    let Some(head) = approved else {
        return Ok(String::new());
    };
    snapshot
        .interned
        .iter()
        .find(|entry| entry.kind == InternKind::Commit && entry.id == head.0 as i64)
        .map(|entry| entry.value.clone())
        .ok_or(Error::Invalid("approved head is not interned"))
}

struct Tracked {
    number: u64,
    head: String,
    approved: String,
    known_heads: Vec<String>,
}
fn tracked(snapshot: &Snapshot, repo: &Repository, include_closed: bool) -> Result<Vec<Tracked>> {
    let world: World = crate::snapshot::decode(snapshot)?;
    let mut result = Vec::new();
    for row in snapshot
        .rows
        .iter()
        .filter(|row| row.table == "pull_requests")
    {
        let node = NodeId(
            row.id
                .parse()
                .map_err(|_| Error::Invalid("invalid PR node"))?,
        );
        let n = &world.nodes[node.0 as usize];
        if (matches!(n.phase, tl3_kernel::Phase::Closed(_))
            && (!include_closed || n.merged.is_some()))
            || crate::request(snapshot, node)?.repo != *repo
        {
            continue;
        }
        let Some(head) = world.nodes[node.0 as usize].head else {
            continue;
        };
        let head = snapshot
            .interned
            .iter()
            .find(|entry| entry.kind == InternKind::Commit && entry.id == head.0 as i64)
            .ok_or(Error::Invalid("tracked head is not interned"))?
            .value
            .clone();
        let known_heads = snapshot
            .interned
            .iter()
            .filter(|entry| entry.kind == InternKind::Commit && entry.scope == row.id)
            .map(|entry| entry.value.clone())
            .collect();
        result.push(Tracked {
            number: row.value["number"]
                .as_u64()
                .ok_or(Error::Invalid("invalid PR number"))?,
            head,
            approved: approved_head(snapshot, n.approved)?,
            known_heads,
        });
    }
    Ok(result)
}

/// Sources read evidence; only the controller maps PRs to nodes and steps the kernel.
pub struct Translator {
    clients: BTreeMap<u64, GitHub>,
    runtime: &'static tokio::runtime::Runtime,
}
impl Translator {
    pub fn new(clients: BTreeMap<u64, GitHub>) -> Result<Self> {
        Ok(Self {
            clients,
            runtime: crate::executor::runtime()?,
        })
    }
    pub fn webhook(
        &self,
        event: &Event,
        snapshot: &Snapshot,
        repos: &[Repository],
    ) -> Result<Vec<Input>> {
        let context = match event {
            Event::PullRequestMerged { context, .. }
            | Event::PullRequestClosed { context, .. }
            | Event::PullRequestSynchronized { context, .. }
            | Event::PullRequestUnlabeled { context, .. }
            | Event::CheckRunCompleted { context, .. }
            | Event::CheckRunUpdated { context, .. }
            | Event::MergeGroup { context, .. } => context,
            _ => return Ok(vec![]),
        };
        let Some(repo) = repos.iter().find(|repo| {
            repo.installation == context.installation_id
                && repo
                    .owner
                    .eq_ignore_ascii_case(&context.repository.owner.login)
                && repo.name.eq_ignore_ascii_case(&context.repository.name)
        }) else {
            return Ok(vec![]);
        };
        let github = self
            .clients
            .get(&repo.installation)
            .ok_or(Error::Invalid("missing queue fact client"))?;
        let open = tracked(snapshot, repo, false)?;
        let world: World = crate::snapshot::decode(snapshot)?;
        let mut facts = Vec::new();
        let mut wake = Vec::new();
        match event {
            Event::PullRequestMerged { pull_request, .. } => {
                let binding = QueueFact {
                    repository: repo.clone(),
                    number: pull_request.number,
                    evidence: Evidence::Merged {
                        head: pull_request.head.sha.clone(),
                        checks_passed: false,
                    },
                };
                if let Some(node) = binding.node(snapshot)?
                    && world.nodes[node.0 as usize].merged.is_none()
                    && !is_merge_queue_pr(pull_request)
                {
                    let checks = self
                        .runtime
                        .block_on(github.check_runs(&full_name(repo), &pull_request.head.sha))?;
                    let head = approved_head(snapshot, world.nodes[node.0 as usize].approved)?;
                    let state = tl3_mergify::observe(pull_request.clone(), &checks, &head);
                    facts.extend(
                        self.runtime
                            .block_on(queue_facts(github, repo, &state, &head))?,
                    );
                }
            }
            Event::PullRequestClosed {
                pull_request,
                actor,
                ..
            }
            | Event::PullRequestUnlabeled {
                pull_request,
                actor,
                ..
            }
            | Event::PullRequestSynchronized {
                pull_request,
                actor,
                ..
            } => {
                if is_merge_queue_pr(pull_request) {
                    return Ok(vec![]);
                }
                let Some(item) = open.iter().find(|item| item.number == pull_request.number) else {
                    return Ok(vec![]);
                };
                let evidence = match event {
                    Event::PullRequestClosed { .. } | Event::PullRequestUnlabeled { .. } => {
                        let events = self
                            .runtime
                            .block_on(github.issue_events(&full_name(repo), pull_request.number))?;
                        // A delayed webhook can predate the close in these fresh events.
                        let current = self
                            .runtime
                            .block_on(github.pull_request(&full_name(repo), pull_request.number))?;
                        if current.merged_at.is_some() {
                            facts.extend(self.look(github, repo, item)?);
                        } else {
                            facts.extend(person_facts(repo, pull_request.number, &events, false));
                        }
                        None
                    }
                    Event::PullRequestSynchronized { after, .. }
                        if actor.login != format!("{}[bot]", github.app_slug())
                            && after == &pull_request.head.sha =>
                    {
                        Some(Evidence::PushByOther {
                            head: after.clone(),
                        })
                    }
                    _ => None,
                };
                facts.extend(evidence.map(|evidence| QueueFact {
                    repository: repo.clone(),
                    number: pull_request.number,
                    evidence,
                }));
            }
            Event::CheckRunCompleted { check_run, .. }
            | Event::CheckRunUpdated { check_run, .. } => {
                let queue_event = check_run.name == tl3_mergify::QUEUE_CHECK
                    && check_run.app.as_ref().is_some_and(|app| {
                        app.id == tl3_mergify::MERGIFY_APP_ID && app.slug == "mergify"
                    });
                if check_run.name == tl3_mergify::QUEUE_CHECK && !queue_event {
                    return Ok(vec![]);
                }
                for item in tracked(snapshot, repo, true)?.iter().filter(|item| {
                    item.head == check_run.head_sha
                        && (check_run.pull_requests.is_empty()
                            || check_run
                                .pull_requests
                                .iter()
                                .any(|pull| pull.number == item.number))
                }) {
                    let mut pending = Vec::new();
                    for effect in &snapshot.pending_effects {
                        let delivery = crate::hydrate(effect, snapshot)?;
                        if delivery.repository == *repo
                            && delivery.pull_request == Some(item.number)
                            && matches!(
                                delivery.output,
                                Out::Effect {
                                    kind: EffectKind::Enqueue,
                                    ..
                                }
                            )
                        {
                            pending.push(Input::Look {
                                key: effect.key.clone(),
                            });
                        }
                    }
                    if queue_event || !pending.is_empty() && item.approved == check_run.head_sha {
                        facts.extend(self.look(github, repo, item)?);
                        if !queue_event {
                            wake.extend(pending);
                        }
                    }
                }
            }
            Event::MergeGroup { .. } => {
                for item in &open {
                    facts.extend(self.look(github, repo, item)?);
                }
            }
            _ => {}
        }
        wake.extend(facts.into_iter().map(QueueFact::input));
        Ok(wake)
    }
    fn look(&self, github: &GitHub, repo: &Repository, item: &Tracked) -> Result<Vec<QueueFact>> {
        let state = self.runtime.block_on(read_queue(
            github,
            &full_name(repo),
            item.number,
            &item.head,
        ))?;
        self.runtime
            .block_on(queue_facts(github, repo, &state, &item.approved))
    }
    pub fn catch_up(
        &self,
        repo: &Repository,
        changes: &RepoChanges,
        snapshot: &Snapshot,
    ) -> Result<Vec<Input>> {
        let github = self
            .clients
            .get(&repo.installation)
            .ok_or(Error::Invalid("missing queue fact client"))?;
        let mut facts = Vec::new();
        let world: World = crate::snapshot::decode(snapshot)?;
        for pull in changes
            .pull_requests
            .iter()
            .filter(|pull| pull.merged_at.is_some() && !is_merge_queue_pr(pull))
        {
            let binding = QueueFact {
                repository: repo.clone(),
                number: pull.number,
                evidence: Evidence::Merged {
                    head: pull.head.sha.clone(),
                    checks_passed: false,
                },
            };
            if let Some(node) = binding.node(snapshot)?
                && matches!(
                    world.nodes[node.0 as usize].phase,
                    tl3_kernel::Phase::Closed(_)
                )
                && world.nodes[node.0 as usize].merged.is_none()
            {
                let head = approved_head(snapshot, world.nodes[node.0 as usize].approved)?;
                let state = tl3_mergify::observe(pull.clone(), &changes.check_runs, &head);
                facts.extend(
                    self.runtime
                        .block_on(queue_facts(github, repo, &state, &head))?,
                );
            }
        }
        for item in tracked(snapshot, repo, false)? {
            // Reread tracked PRs even when their updated_at is outside the catch-up window.
            facts.extend(self.look(github, repo, &item)?);
            let pull = self
                .runtime
                .block_on(github.pull_request(&full_name(repo), item.number))?;
            if is_merge_queue_pr(&pull) || pull.merged_at.is_some() {
                continue;
            }
            if !item.known_heads.contains(&pull.head.sha) {
                facts.push(QueueFact {
                    repository: repo.clone(),
                    number: item.number,
                    evidence: Evidence::PushByOther {
                        head: pull.head.sha.clone(),
                    },
                });
            }
            for (number, event) in &changes.labels {
                if *number == item.number
                    && event.event == "unlabeled"
                    && event.label.name == QUEUE_LABEL
                    && person(&event.actor)
                {
                    facts.push(QueueFact {
                        repository: repo.clone(),
                        number: item.number,
                        evidence: Evidence::DequeuedByPerson { event: event.id },
                    });
                }
            }
            for (number, event) in &changes.closed {
                if *number == item.number && event.actor.as_ref().is_some_and(person) {
                    facts.push(QueueFact {
                        repository: repo.clone(),
                        number: item.number,
                        evidence: Evidence::ClosedByPerson { event: event.id },
                    });
                }
            }
        }
        Ok(facts.into_iter().map(QueueFact::input).collect())
    }
}
fn person(actor: &Actor) -> bool {
    !actor.login.ends_with("[bot]")
}

fn person_facts(
    repo: &Repository,
    number: u64,
    events: &[tl3_github::IssueActivity],
    merged: bool,
) -> Vec<QueueFact> {
    if merged || events.iter().any(|event| event.event == "merged") {
        return vec![];
    }
    events
        .iter()
        .filter(|event| event.actor.as_ref().is_some_and(person))
        .filter_map(|event| {
            let evidence = match event.event.as_str() {
                "unlabeled"
                    if event
                        .label
                        .as_ref()
                        .is_some_and(|label| label.name == QUEUE_LABEL) =>
                {
                    Evidence::DequeuedByPerson { event: event.id }
                }
                "closed" => Evidence::ClosedByPerson { event: event.id },
                _ => return None,
            };
            Some(QueueFact {
                repository: repo.clone(),
                number,
                evidence,
            })
        })
        .collect()
}

impl GitHubTranslator for Translator {
    fn translate(&mut self, delivery: &GitHubDelivery, snapshot: &Snapshot) -> Result<Vec<Input>> {
        let repos = snapshot
            .rows
            .iter()
            .filter(|row| row.table == "repos")
            .map(|row| crate::decode_stored(row.value.clone()))
            .collect::<std::result::Result<Vec<Repository>, _>>()?;
        match delivery {
            GitHubDelivery::Webhook(delivery) => match &delivery.event {
                Some(event) => self.webhook(event, snapshot, &repos),
                None => Ok(vec![]),
            },
            GitHubDelivery::CatchUp {
                repository,
                changes,
            } if repos.contains(repository) => self.catch_up(repository, changes, snapshot),
            _ => Ok(vec![]),
        }
    }
}

#[cfg(test)]
mod error_tests {
    use super::*;

    #[test]
    fn github_failure_class_survives_mergify() {
        for (status, expected) in [
            (401, "permission"),
            (403, "permission"),
            (404, "not_found"),
            (410, "not_found"),
            (422, "invalid_request"),
            (500, "upstream_server"),
            (503, "upstream_server"),
        ] {
            let direct = Error::from(tl3_github::Error::Http { status });
            let wrapped = adapter_error(tl3_mergify::Error::GitHub(tl3_github::Error::Http {
                status,
            }));
            assert_eq!(
                crate::looks::failure_class(&direct),
                expected,
                "direct HTTP {status}"
            );
            assert_eq!(
                crate::looks::failure_class(&wrapped),
                expected,
                "wrapped HTTP {status}"
            );
        }
        for error in [tl3_github::Error::Transport, tl3_github::Error::Response] {
            let expected = if matches!(error, tl3_github::Error::Transport) {
                "transport"
            } else {
                "invalid_response"
            };
            assert_eq!(
                crate::looks::failure_class(&adapter_error(tl3_mergify::Error::GitHub(error))),
                expected
            );
        }
    }
}
