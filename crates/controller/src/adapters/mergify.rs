pub(crate) mod ci_evidence;
mod head_checks;
mod waiting_requeue;
pub(crate) use waiting_requeue::PROGRESS_PREFIX as REQUEUE_PROGRESS_PREFIX;

use super::record::Records;
use crate::executor::Output as Out;
use crate::{
    Result,
    executor::{Completion, Delivery, Executor, Look, Observation, Reconciliation},
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

/// A required context and the app expected to report it; `None` accepts any source.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequiredCheck {
    pub context: String,
    pub app: Option<u64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Evidence {
    ChecksUnavailable {
        permissions: Vec<String>,
    },
    /// Every check suite on the head completed without this required context (Q52).
    CheckMissing {
        head: String,
        required: RequiredCheck,
    },
    /// #447: Mergify waits on queue conditions that nothing still running can meet.
    QueueBlocked {
        head: String,
        conditions: Vec<String>,
        failed: Vec<String>,
    },
    /// The context of an open missing-check incident has reported on that head (Q52).
    CheckReported {
        head: String,
        required: RequiredCheck,
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
        #[serde(default)]
        details: crate::findings::QueueCiEvidence,
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
    /// Setup evidence is deduplicated against incidents, not observation receipts.
    pub(crate) fn setup(&self) -> bool {
        matches!(
            self.evidence,
            Evidence::ChecksUnavailable { .. }
                | Evidence::CheckMissing { .. }
                | Evidence::CheckReported { .. }
                | Evidence::QueueBlocked { .. }
        )
    }
    pub(crate) fn recorded(&self, snapshot: &Snapshot) -> Result<bool> {
        if let Evidence::CheckMissing { head, required }
        | Evidence::CheckReported { head, required } = &self.evidence
        {
            let Some(node) = self.node(snapshot)? else {
                return Ok(false);
            };
            let id = crate::looks::missing_check_incident_id(node, head, required);
            let incident = snapshot
                .rows
                .iter()
                .find(|row| row.table == "incidents" && row.id == id);
            // A raised incident stays deduplicated after a clear; a report matters only while open.
            return Ok(match self.evidence {
                Evidence::CheckMissing { .. } => incident.is_some(),
                _ => !incident.is_some_and(|row| row.value["open"] == true),
            });
        }
        if let Evidence::QueueBlocked {
            head, conditions, ..
        } = &self.evidence
        {
            let Some(node) = self.node(snapshot)? else {
                return Ok(false);
            };
            let id = crate::looks::queue_blocked_incident_id(node, head, conditions);
            return Ok(snapshot
                .rows
                .iter()
                .any(|row| row.table == "incidents" && row.id == id));
        }
        if let Evidence::ChecksUnavailable { permissions } = &self.evidence {
            let Some(node) = self.node(snapshot)? else {
                return Ok(false);
            };
            return Ok(permissions.iter().all(|permission| {
                let id = crate::looks::check_setup_incident_id(node, permission);
                snapshot.rows.iter().any(|row| {
                    row.table == "incidents" && row.id == id && row.value["open"] == true
                })
            }));
        }
        let identity = self.identity();
        Ok(snapshot
            .rows
            .iter()
            .any(|row| row.table == "queue_facts" && row.id == identity))
    }
    fn unresolved_hold(&self, snapshot: &Snapshot) -> Result<bool> {
        if !matches!(
            self.evidence,
            Evidence::DequeuedByPerson { .. }
                | Evidence::ClosedByPerson { .. }
                | Evidence::DequeuedByCommand { .. }
        ) {
            return Ok(false);
        }
        let Some(node) = self.node(snapshot)? else {
            return Ok(true);
        };
        let observed = snapshot
            .rows
            .iter()
            .find(|row| row.table == "queue_facts" && row.id == self.identity());
        let resumed = snapshot
            .rows
            .iter()
            .find(|row| row.table == "queue_resumes" && row.id == node.0.to_string());
        Ok(!observed
            .zip(resumed)
            .is_some_and(|(observed, resumed)| resumed.version > observed.version))
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
            Evidence::ChecksUnavailable { permissions } => {
                let mut gaps = permissions.clone();
                gaps.sort();
                gaps.dedup();
                format!(
                    "required-checks-setup:{}",
                    serde_json::to_string(&gaps).unwrap()
                )
            }
            Evidence::CheckMissing { head, required } => {
                format!(
                    "missing-check:{head}:{}",
                    serde_json::to_string(required).unwrap()
                )
            }
            Evidence::CheckReported { head, required } => {
                format!(
                    "check-reported:{head}:{}",
                    serde_json::to_string(required).unwrap()
                )
            }
            Evidence::QueueBlocked {
                head, conditions, ..
            } => format!(
                "queue-blocked:{head}:{}",
                serde_json::to_string(conditions).unwrap()
            ),
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
    ci_access: tl3_config::BuildBuddy,
    clients: BTreeMap<u64, GitHub>,
    epoch: i64,
    records: Records,
    runtime: &'static tokio::runtime::Runtime,
    /// Each served repository's PR-body policy and heal reruns (#447).
    policies: Vec<(Repository, BodyPolicy)>,
    /// TL3's branch names, so a heal touches only TL3's own PRs.
    branches: Option<crate::branches::Branches>,
    /// The world, so a heal touches only a node in Publishing or Queued.
    store: Option<tl3_config::Store>,
    clock: Box<dyn crate::clock::Clock>,
}
/// A served repository's PR-body lines and the workflows a heal reruns (#447).
struct BodyPolicy {
    rules: Vec<tl3_config::BodyLine>,
    reruns: Vec<String>,
}
impl Adapter {
    pub fn with_ci_evidence(mut self, access: tl3_config::BuildBuddy) -> Self {
        self.ci_access = access;
        self
    }

    pub fn new(clients: BTreeMap<u64, GitHub>, epoch: i64, records: &Path) -> Result<Self> {
        std::fs::create_dir_all(records)?;
        Ok(Self {
            ci_access: Default::default(),
            clients,
            epoch,
            records: Records::new(records),
            runtime: crate::executor::runtime()?,
            policies: Vec::new(),
            branches: None,
            store: None,
            clock: Box::new(crate::clock::SystemClock),
        })
    }
    pub fn with_clock(mut self, clock: impl crate::clock::Clock + 'static) -> Self {
        self.clock = Box::new(clock);
        self
    }
    pub fn with_repositories(mut self, repos: &[tl3_config::Repository]) -> Result<Self> {
        self.policies = repos
            .iter()
            .map(|repo| {
                (
                    Repository {
                        owner: repo.owner.clone(),
                        name: repo.name.clone(),
                        installation: repo.installation_id,
                    },
                    BodyPolicy {
                        rules: repo.pr_body_lines.clone(),
                        reruns: repo.heal_rerun_workflows.clone(),
                    },
                )
            })
            .collect();
        self.branches = Some(crate::branches::Branches::new(repos)?);
        Ok(self)
    }
    /// Reads the world, so a heal touches only a node in Publishing or Queued.
    pub fn with_store(mut self, store: tl3_config::Store) -> Self {
        self.store = Some(store);
        self
    }
    /// #447: one idempotent update brings an open TL3 PR's body into its
    /// repository's policy. A body that complies is left alone, and so is a PR
    /// on another branch or a node outside Publishing and Queued. Returns
    /// whether it edited the body.
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
    fn snapshot(&self) -> Result<Option<Snapshot>> {
        Ok(self
            .store
            .as_ref()
            .map(|store| tl3_store::Store::open(store)?.load())
            .transpose()?)
    }
    /// The delivery's node phase, from the evaluation's snapshot.
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
    fn fence(&mut self, delivery: &Delivery) -> Result<()> {
        if delivery.epoch < self.epoch {
            return Err(Error::Invalid("Mergify effect epoch is fenced"));
        }
        self.records.fence(delivery.epoch)?;
        self.epoch = delivery.epoch;
        Ok(())
    }
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
                &self.ci_access,
            ))?
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
        let snapshot = self.snapshot()?;
        let (_, observations) = self.read(delivery, snapshot.as_ref())?;
        Ok(observations
            .into_iter()
            .filter(|observation| {
                matches!(
                    observation,
                    Observation::Queue(QueueFact {
                        evidence: Evidence::CiFailed {
                            head_check: true,
                            ..
                        } | Evidence::ChecksUnavailable { .. }
                            | Evidence::CheckMissing { .. }
                            | Evidence::QueueBlocked { .. },
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
        Ok(Reconciliation::Found(Box::new(Completion::confirmed(
            vec![],
        ))))
    }
    fn retire_delivery(&mut self, delivery: &Delivery) -> Result<()> {
        self.retain_legacy_command(delivery)?;
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
    /// #356: an enqueue Mergify hasn't acknowledged yet is a wait, and the
    /// no-progress allowance catches one that never comes. Only a read that
    /// errors is a failed look.
    fn look(&mut self, delivery: &Delivery) -> Result<Look> {
        Ok(match self.read_enqueue(delivery)? {
            Reconciliation::Found(completion) => Look::Found(completion),
            Reconciliation::Absent => Look::Absent,
            Reconciliation::Pending => Look::Alive {
                progress: self
                    .requeue_progress(delivery)?
                    .unwrap_or_else(|| "mergify:awaiting-acknowledgement".into()),
            },
        })
    }
}
impl Adapter {
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

/// #447: the failed runs a PR-body edit leaves stale. A `pull_request` workflow
/// reruns on `opened`, `synchronize` and `reopened`; an edit doesn't rerun it, so
/// its failed run stays failed. `pull_request_target` workflows, like
/// PrismaBuild's regression-test, can listen for `edited` and rerun themselves.
/// Only the repository's listed workflow files are rerun, since a rerun can
/// have side effects; an empty list reruns nothing.
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

/// #447 review: a heal touches only a node in Publishing or Queued.
pub(crate) fn heal_phase(phase: Option<tl3_kernel::Phase>) -> bool {
    matches!(
        phase,
        Some(tl3_kernel::Phase::Publishing | tl3_kernel::Phase::Queued)
    )
}

/// #447 review: whether every check on the head, other than Mergify's own,
/// has completed. Until then a check suite read can't show a blocked queue.
pub(crate) fn checks_settled(state: &tl3_mergify::Observation) -> bool {
    state
        .check_runs
        .iter()
        .filter(|check| check.head_sha == state.pull_request.head.sha)
        .filter(|check| !check.app.as_ref().is_some_and(|app| app.slug == "mergify"))
        .all(|check| check.status == "completed")
}

/// A PR-body heal on one head: whether its stale runs have been rerun (#447).
#[derive(Serialize, Deserialize)]
struct BodyHeal {
    head: String,
    reran: bool,
}

/// #447: Mergify waits on queue conditions while every check suite on the head
/// has completed, so nothing still running can meet them. Returns the unmet
/// conditions and the failed checks outside TL3's required set.
pub(crate) fn queue_blocked(
    state: &tl3_mergify::Observation,
    suites_completed: bool,
    required: &[RequiredCheck],
) -> Option<(Vec<String>, Vec<String>)> {
    if state.state != QueueState::Waiting || !suites_completed {
        return None;
    }
    let conditions = tl3_mergify::unmet_conditions(state.check.as_ref()?);
    if conditions.is_empty() {
        return None;
    }
    let head = &state.pull_request.head.sha;
    let mut latest: BTreeMap<&str, &CheckRun> = BTreeMap::new();
    for check in state
        .check_runs
        .iter()
        .filter(|check| &check.head_sha == head)
    {
        let entry = latest.entry(check.name.as_str()).or_insert(check);
        if check.id > entry.id {
            *entry = check;
        }
    }
    let failed = latest
        .into_values()
        .filter(|check| {
            check.status == "completed"
                && matches!(
                    check.conclusion.as_deref(),
                    Some(
                        "failure"
                            | "cancelled"
                            | "timed_out"
                            | "action_required"
                            | "startup_failure"
                    )
                )
                && !check.app.as_ref().is_some_and(|app| app.slug == "mergify")
                && !required.iter().any(|rule| {
                    rule.context == check.name
                        && rule
                            .app
                            .is_none_or(|id| check.app.as_ref().is_some_and(|app| app.id == id))
                })
        })
        .map(|check| check.name.clone())
        .collect();
    Some((conditions, failed))
}

/// `open` names the open missing-check incidents on the expected head.
async fn queue_facts(
    github: &GitHub,
    repo: &Repository,
    state: &tl3_mergify::Observation,
    expected_head: &str,
    open: &[RequiredCheck],
    snapshot: Option<&Snapshot>,
    ci_access: &tl3_config::BuildBuddy,
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
            if !suites.is_empty() && suites.iter().all(|suite| suite.status == "completed") {
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
            let completed =
                !suites.is_empty() && suites.iter().all(|suite| suite.status == "completed");
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
                    *details = ci_evidence::read_checks(
                        github,
                        &repo_name,
                        &pull.head.sha,
                        &state.check_runs,
                        ci_access,
                    )
                    .await;
                }
                facts.push(fact);
            }
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
            details: Default::default(),
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
            details: Default::default(),
        }),
        QueueState::Conflict => state.check.as_ref().map(|check| Evidence::Conflict {
            check: check.id,
            completed_at: check.completed_at.map(|time| time.to_rfc3339()),
            head: pull.head.sha.clone(),
        }),
        _ => None,
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
                *details =
                    ci_evidence::read_queue(github, &full_name(repo), state, ci_access).await;
            }
            facts.push(fact);
        }
    }
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
    missing: Vec<RequiredCheck>,
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
        let approved = approved_head(snapshot, n.approved)?;
        result.push(Tracked {
            number: row.value["number"]
                .as_u64()
                .ok_or(Error::Invalid("invalid PR number"))?,
            missing: open_missing_checks(snapshot, node, &approved)?,
            head,
            approved,
            known_heads,
        });
    }
    Ok(result)
}

fn open_missing_checks(
    snapshot: &Snapshot,
    node: NodeId,
    head: &str,
) -> Result<Vec<RequiredCheck>> {
    let mut open = Vec::new();
    for row in snapshot
        .rows
        .iter()
        .filter(|row| row.table == "queue_facts")
    {
        let Ok(fact) = serde_json::from_value::<QueueFact>(row.value.clone()) else {
            continue;
        };
        if let Evidence::CheckMissing {
            head: missing,
            required,
        } = &fact.evidence
            && missing == head
            && fact.node(snapshot)? == Some(node)
        {
            let id = crate::looks::missing_check_incident_id(node, head, required);
            if snapshot
                .rows
                .iter()
                .any(|row| row.table == "incidents" && row.id == id && row.value["open"] == true)
            {
                open.push(required.clone());
            }
        }
    }
    Ok(open)
}

fn pending_enqueues(snapshot: &Snapshot, repo: &Repository, number: u64) -> Result<Vec<Input>> {
    let mut pending = Vec::new();
    for effect in &snapshot.pending_effects {
        let delivery = crate::hydrate(effect, snapshot)?;
        if delivery.repository == *repo
            && delivery.pull_request == Some(number)
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
    Ok(pending)
}

/// Sources read evidence; only the controller maps PRs to nodes and steps the kernel.
pub struct Translator {
    ci_access: tl3_config::BuildBuddy,
    clients: BTreeMap<u64, GitHub>,
    runtime: &'static tokio::runtime::Runtime,
}
impl Translator {
    pub fn with_ci_evidence(mut self, access: tl3_config::BuildBuddy) -> Self {
        self.ci_access = access;
        self
    }

    pub fn new(clients: BTreeMap<u64, GitHub>) -> Result<Self> {
        Ok(Self {
            ci_access: Default::default(),
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
            | Event::CheckSuiteCompleted { context, .. }
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
                    facts.extend(self.runtime.block_on(queue_facts(
                        github,
                        repo,
                        &state,
                        &head,
                        &[],
                        Some(snapshot),
                        &self.ci_access,
                    ))?);
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
                            facts.extend(self.look(github, repo, item, snapshot)?);
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
                    let pending = pending_enqueues(snapshot, repo, item.number)?;
                    if queue_event || !pending.is_empty() && item.approved == check_run.head_sha {
                        facts.extend(self.look(github, repo, item, snapshot)?);
                        if !queue_event {
                            wake.extend(pending);
                        }
                    }
                }
            }
            // A completed suite can be the last evidence that a required check won't report (Q52).
            Event::CheckSuiteCompleted { check_suite, .. } => {
                for item in open.iter().filter(|item| {
                    item.approved == check_suite.head_sha
                        && (check_suite.pull_requests.is_empty()
                            || check_suite
                                .pull_requests
                                .iter()
                                .any(|pull| pull.number == item.number))
                }) {
                    let pending = pending_enqueues(snapshot, repo, item.number)?;
                    if !pending.is_empty() {
                        facts.extend(self.look(github, repo, item, snapshot)?);
                        wake.extend(pending);
                    }
                }
            }
            Event::MergeGroup { .. } => {
                for item in &open {
                    facts.extend(self.look(github, repo, item, snapshot)?);
                }
            }
            _ => {}
        }
        wake.extend(facts.into_iter().map(QueueFact::input));
        Ok(wake)
    }
    fn look(
        &self,
        github: &GitHub,
        repo: &Repository,
        item: &Tracked,
        snapshot: &Snapshot,
    ) -> Result<Vec<QueueFact>> {
        let state = self.runtime.block_on(read_queue(
            github,
            &full_name(repo),
            item.number,
            &item.head,
        ))?;
        self.runtime.block_on(queue_facts(
            github,
            repo,
            &state,
            &item.approved,
            &item.missing,
            Some(snapshot),
            &self.ci_access,
        ))
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
                facts.extend(self.runtime.block_on(queue_facts(
                    github,
                    repo,
                    &state,
                    &head,
                    &[],
                    Some(snapshot),
                    &self.ci_access,
                ))?);
            }
        }
        for item in tracked(snapshot, repo, false)? {
            // Reread tracked PRs even when their updated_at is outside the catch-up window.
            facts.extend(self.look(github, repo, &item, snapshot)?);
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

#[cfg(test)]
mod queue_blocked_tests {
    use super::*;
    use tl3_mergify::pb2632_fixture::{PB2632_CHECK_RUNS, PB2632_PULL_REQUEST};

    fn pb2632() -> tl3_mergify::Observation {
        let pull: tl3_github::PullRequest = serde_json::from_str(PB2632_PULL_REQUEST).unwrap();
        let checks: Vec<tl3_github::CheckRun> = serde_json::from_str(PB2632_CHECK_RUNS).unwrap();
        let head = pull.head.sha.clone();
        tl3_mergify::observe(pull, &checks, &head)
    }

    // PrismaBuild's branch protection requires only issue-link (app 15368).
    fn required() -> Vec<RequiredCheck> {
        vec![RequiredCheck {
            context: "issue-link".into(),
            app: Some(15368),
        }]
    }

    #[test]
    fn prismabuild_2632_is_blocked_on_scopes_by_pr_gate() {
        let (conditions, failed) = queue_blocked(&pb2632(), true, &required())
            .expect("TL3 must not wait silently on #2632");
        assert_eq!(
            conditions,
            ["scopes-available: waiting for pull request scopes to be reported"]
        );
        assert_eq!(
            failed,
            ["Bazel", "Hosted lane", "pr-gate", "regression-test"]
        );
    }

    #[test]
    fn a_body_edit_reruns_only_prismabuilds_gate() {
        // gate.yml (pull_request) has no `edited` trigger; regression-test.yml
        // (pull_request_target) has one and reruns itself; issue-link passed.
        let runs: Vec<tl3_github::WorkflowRun> =
            serde_json::from_str(tl3_mergify::pb2632_fixture::PB2632_WORKFLOW_RUNS).unwrap();
        let head = "4309359678fc08ddfb44f12439e3a3744ee3eb11";
        let gate = ["gate.yml".to_owned()];
        assert_eq!(body_edit_reruns(&runs, head, &gate), [41476603888]);
        assert!(body_edit_reruns(&runs, "another-head", &gate).is_empty());
        // A rerun that already succeeded needs nothing more.
        let mut passed = runs.clone();
        for run in &mut passed {
            run.conclusion = Some("success".into());
        }
        assert!(body_edit_reruns(&passed, head, &gate).is_empty());
    }

    #[test]
    fn a_body_edit_reruns_only_listed_workflows() {
        // #447 review: a rerun can have side effects, so only listed files rerun.
        let mut runs: Vec<tl3_github::WorkflowRun> =
            serde_json::from_str(tl3_mergify::pb2632_fixture::PB2632_WORKFLOW_RUNS).unwrap();
        let head = "4309359678fc08ddfb44f12439e3a3744ee3eb11";
        let mut deploy = runs[0].clone();
        deploy.id = 41476609999;
        deploy.path = ".github/workflows/deploy.yml".into();
        deploy.conclusion = Some("failure".into());
        runs.push(deploy);
        assert!(body_edit_reruns(&runs, head, &[]).is_empty());
        assert_eq!(
            body_edit_reruns(&runs, head, &["gate.yml".to_owned()]),
            [41476603888]
        );
        // A name that only ends like a listed file doesn't count.
        assert!(body_edit_reruns(&runs, head, &["ate.yml".to_owned()]).is_empty());
    }

    #[test]
    fn a_heal_touches_only_publishing_or_queued_nodes() {
        use tl3_kernel::{Outcome, Phase};
        assert!(heal_phase(Some(Phase::Publishing)));
        assert!(heal_phase(Some(Phase::Queued)));
        for phase in [
            Phase::Making,
            Phase::Rebasing,
            Phase::Closed(Outcome::Merged),
        ] {
            assert!(!heal_phase(Some(phase)), "{phase:?}");
        }
        assert!(!heal_phase(None), "an unknown node is never healed");
    }

    #[test]
    fn a_running_check_skips_the_suite_read() {
        assert!(checks_settled(&pb2632()));
        let mut running = pb2632();
        let check = running
            .check_runs
            .iter_mut()
            .find(|check| check.name == "pr-gate")
            .unwrap();
        check.status = "in_progress".into();
        check.conclusion = None;
        assert!(!checks_settled(&running));
        // Mergify's own queue check stays in progress while it waits.
        let mut waiting = pb2632();
        for check in &mut waiting.check_runs {
            if check.app.as_ref().is_some_and(|app| app.slug == "mergify") {
                check.status = "in_progress".into();
                check.conclusion = None;
            }
        }
        assert!(checks_settled(&waiting));
    }

    #[test]
    fn a_suite_still_running_or_a_queue_not_waiting_is_not_blocked() {
        assert_eq!(queue_blocked(&pb2632(), false, &required()), None);
        let mut queued = pb2632();
        queued.state = tl3_mergify::QueueState::Queued;
        assert_eq!(queue_blocked(&queued, true, &required()), None);
    }
}
