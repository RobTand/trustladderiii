use super::record::Records;
use crate::{
    Result,
    executor::{Completion, Delivery, Executor, Observation, Reconciliation},
};
use std::path::Path;
use tl3_kernel::{EffectKind, Out};

pub struct Fake(Records);
impl Fake {
    pub fn new(root: &Path) -> Self {
        Self(Records::new(root))
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
        self.0
            .deliver(delivery, Completion::confirmed(observations))
    }
    fn reconcile(&mut self, delivery: &Delivery) -> Result<Reconciliation> {
        self.0.reconcile(delivery)
    }
}

use crate::{Bootstrap, Error, Repository, cli::Input};
use chrono::{DateTime, Utc};
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
    Merged {
        head: String,
        checks_passed: bool,
    },
    CiFailed {
        check: u64,
        head: String,
        infra: bool,
    },
    Conflict {
        check: u64,
        head: String,
    },
    DequeuedByPerson {
        at: DateTime<Utc>,
    },
    ClosedByPerson {
        at: DateTime<Utc>,
    },
    PushByOther {
        head: String,
    },
}
impl QueueFact {
    pub(crate) fn identity(&self) -> String {
        let evidence = match &self.evidence {
            Evidence::Merged { head, .. } => format!("merged:{head}"),
            Evidence::CiFailed { check, .. } => format!("ci:{check}"),
            Evidence::Conflict { check, .. } => format!("conflict:{check}"),
            Evidence::DequeuedByPerson { at } => format!("dequeued:{at}"),
            Evidence::ClosedByPerson { at } => format!("closed:{at}"),
            Evidence::PushByOther { head } => format!("push:{head}"),
        };
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

#[derive(Serialize, Deserialize)]
struct Attempt {
    before: Option<CheckRun>,
}

pub struct Adapter {
    clients: BTreeMap<u64, GitHub>,
    epoch: i64,
    records: Records,
    runtime: tokio::runtime::Runtime,
}
impl Adapter {
    pub fn new(clients: BTreeMap<u64, GitHub>, epoch: i64, records: &Path) -> Result<Self> {
        std::fs::create_dir_all(records)?;
        Ok(Self {
            clients,
            epoch,
            records: Records::new(records),
            runtime: runtime()?,
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
            .block_on(Mergify::new(github.clone()).read(&repo, number, head))
            .map_err(adapter_error)?;
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
    fn completion(&self, delivery: &Delivery) -> Result<Reconciliation> {
        let (state, observations) = self.read(delivery)?;
        let previously_confirmed =
            matches!(self.records.reconcile(delivery)?, Reconciliation::Found(_));
        let baseline = self.baseline(delivery)?;
        let current_head = state.pull_request.head.sha == enqueue(delivery)?.1;
        let active = matches!(state.state, QueueState::Queued | QueueState::ChecksRunning);
        let terminal = matches!(
            state.state,
            QueueState::Dequeued | QueueState::FailedCi | QueueState::Conflict
        );
        if previously_confirmed
            || state.state == QueueState::Merged
            || (current_head
                && (active
                    || (terminal
                        && baseline
                            .as_ref()
                            .is_some_and(|before| before.before != state.check))))
        {
            return Ok(Reconciliation::Found(Completion::confirmed(observations)));
        }
        let github = &self.clients[&delivery.repository.installation];
        let (number, _) = enqueue(delivery)?;
        let issue = self
            .runtime
            .block_on(github.issue(&full_name(&delivery.repository), number))?;
        if current_head && terminal {
            // An unchanged terminal check belongs to the previous enqueue, not this key.
            return Ok(Reconciliation::Absent);
        }
        if baseline.is_none() {
            return Ok(Reconciliation::Absent);
        }
        if issue.labels.iter().any(|label| label.name == QUEUE_LABEL) {
            Ok(Reconciliation::Pending)
        } else {
            Ok(Reconciliation::Absent)
        }
    }
}
impl Executor for Adapter {
    fn execute(&mut self, delivery: &Delivery) -> Result<Completion> {
        self.fence(delivery)?;
        match self.completion(delivery)? {
            Reconciliation::Found(completion) => {
                if !matches!(self.records.reconcile(delivery)?, Reconciliation::Found(_)) {
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
        if self.baseline(delivery)?.is_none() {
            self.records.write(
                &self.records.path(&delivery.key).with_extension("intent"),
                &serde_json::to_vec(&Attempt {
                    before: state.check.clone(),
                })?,
            )?;
        }
        let (number, head) = enqueue(delivery)?;
        let github = self.clients[&delivery.repository.installation].clone();
        let repo = full_name(&delivery.repository);
        let issue = self.runtime.block_on(github.issue(&repo, number))?;
        if !issue.labels.iter().any(|label| label.name == QUEUE_LABEL)
            || matches!(
                state.state,
                QueueState::FailedCi | QueueState::Conflict | QueueState::Dequeued
            )
        {
            self.fence(delivery)?;
            self.runtime
                .block_on(Mergify::new(github).queue(&repo, number, head))
                .map_err(adapter_error)?;
        }
        match self.completion(delivery)? {
            Reconciliation::Found(completion) => {
                self.records
                    .deliver(delivery, Completion::confirmed(vec![]))?;
                Ok(completion)
            }
            _ => Err(Error::Invalid(
                "queue label written; Mergify acceptance is still unknown",
            )),
        }
    }
    fn reconcile(&mut self, delivery: &Delivery) -> Result<Reconciliation> {
        // Reads retain the original effect's epoch, including after a controller upgrade.
        match self.completion(delivery) {
            Ok(result) => Ok(result),
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
fn adapter_error(error: tl3_mergify::Error) -> Error {
    Error::Adapter(Box::new(error))
}
fn runtime() -> Result<tokio::runtime::Runtime> {
    Ok(tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?)
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
    if is_merge_queue_pr(pull)
        || (state.state != QueueState::Merged && pull.head.sha != expected_head)
    {
        return Ok(vec![]);
    }
    let evidence = match state.state {
        QueueState::Merged => {
            // Approval covers the original head. CI must cover the actual merge commit.
            let checks = match &pull.merge_commit_sha {
                Some(sha) => github.check_runs(&full_name(repo), sha).await?,
                None => vec![],
            };
            let checks_passed = pull
                .merge_commit_sha
                .as_ref()
                .is_some_and(|sha| checks_passed(&checks, sha));
            Some(Evidence::Merged {
                head: pull.head.sha.clone(),
                checks_passed,
            })
        }
        QueueState::FailedCi => state.check.as_ref().map(|check| Evidence::CiFailed {
            check: check.id,
            head: pull.head.sha.clone(),
            infra: tl3_mergify::ci_is_infrastructure(check),
        }),
        QueueState::Conflict => state.check.as_ref().map(|check| Evidence::Conflict {
            check: check.id,
            head: pull.head.sha.clone(),
        }),
        _ => None,
    };
    Ok(evidence
        .into_iter()
        .map(|evidence| QueueFact {
            repository: repo.clone(),
            number: pull.number,
            evidence,
        })
        .collect())
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
            check.status == "completed" && check.conclusion.as_deref() == Some("success")
        })
}

struct Tracked {
    number: u64,
    head: String,
    known_heads: Vec<String>,
}
fn tracked(snapshot: &Snapshot, repo: &Repository) -> Result<Vec<Tracked>> {
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
        if crate::request(snapshot, node)?.repo != *repo {
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
            known_heads,
        });
    }
    Ok(result)
}

/// Sources read evidence; only the controller maps PRs to nodes and steps the kernel.
pub struct Translator {
    clients: BTreeMap<u64, GitHub>,
    runtime: tokio::runtime::Runtime,
}
impl Translator {
    pub fn new(clients: BTreeMap<u64, GitHub>) -> Result<Self> {
        Ok(Self {
            clients,
            runtime: runtime()?,
        })
    }
    pub fn webhook(
        &self,
        event: &Event,
        snapshot: &Snapshot,
        bootstrap: &Bootstrap,
    ) -> Result<Vec<Input>> {
        let context = match event {
            Event::PullRequestMerged { context, .. }
            | Event::PullRequestClosed { context, .. }
            | Event::PullRequestSynchronized { context, .. }
            | Event::PullRequestUnlabeled { context, .. }
            | Event::CheckRunCompleted { context, .. }
            | Event::CheckSuiteCompleted { context, .. }
            | Event::MergeGroup { context, .. } => context,
            _ => return Ok(vec![]),
        };
        let Some(repo) = bootstrap.repos.iter().find(|repo| {
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
        let tracked = tracked(snapshot, repo)?;
        let mut facts = Vec::new();
        match event {
            Event::PullRequestMerged { pull_request, .. } => {
                if tracked
                    .iter()
                    .any(|item| item.number == pull_request.number)
                    && !is_merge_queue_pr(pull_request)
                {
                    let state = tl3_mergify::observe(pull_request.clone(), &[], "");
                    facts.extend(
                        self.runtime
                            .block_on(queue_facts(github, repo, &state, ""))?,
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
                if is_merge_queue_pr(pull_request)
                    || !tracked
                        .iter()
                        .any(|item| item.number == pull_request.number)
                {
                    return Ok(vec![]);
                }
                let evidence = match event {
                    Event::PullRequestClosed { .. } if person(actor) => {
                        Some(Evidence::ClosedByPerson {
                            at: pull_request.updated_at,
                        })
                    }
                    Event::PullRequestUnlabeled { label, .. }
                        if label.name == QUEUE_LABEL && person(actor) =>
                    {
                        Some(Evidence::DequeuedByPerson {
                            at: pull_request.updated_at,
                        })
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
            Event::CheckRunCompleted { check_run, .. } => {
                for item in tracked
                    .iter()
                    .filter(|item| item.head == check_run.head_sha)
                {
                    facts.extend(self.look(github, repo, item)?);
                }
            }
            Event::CheckSuiteCompleted { check_suite, .. } => {
                for item in tracked
                    .iter()
                    .filter(|item| item.head == check_suite.head_sha)
                {
                    facts.extend(self.look(github, repo, item)?);
                }
            }
            Event::MergeGroup { merge_group, .. } => {
                if merge_group.head_ref.starts_with("mergify/merge-queue/") {
                    return Ok(vec![]);
                }
                for item in &tracked {
                    facts.extend(self.look(github, repo, item)?);
                }
            }
            _ => {}
        }
        Ok(facts.into_iter().map(QueueFact::input).collect())
    }
    fn look(&self, github: &GitHub, repo: &Repository, item: &Tracked) -> Result<Vec<QueueFact>> {
        let state = self
            .runtime
            .block_on(Mergify::new(github.clone()).read(&full_name(repo), item.number, &item.head))
            .map_err(adapter_error)?;
        self.runtime
            .block_on(queue_facts(github, repo, &state, &item.head))
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
        for item in tracked(snapshot, repo)? {
            // Reread tracked PRs even when their updated_at is outside the catch-up window.
            facts.extend(self.look(github, repo, &item)?);
            let pull = self
                .runtime
                .block_on(github.pull_request(&full_name(repo), item.number))?;
            if is_merge_queue_pr(&pull) {
                continue;
            }
            if pull.merged_at.is_none() && !item.known_heads.contains(&pull.head.sha) {
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
                        evidence: Evidence::DequeuedByPerson {
                            at: event.created_at,
                        },
                    });
                }
            }
            for (number, event) in &changes.closed {
                if *number == item.number && person(&event.actor) {
                    facts.push(QueueFact {
                        repository: repo.clone(),
                        number: item.number,
                        evidence: Evidence::ClosedByPerson {
                            at: event.created_at,
                        },
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
