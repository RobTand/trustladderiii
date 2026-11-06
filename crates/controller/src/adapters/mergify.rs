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
    fn contract_evidence(
        &mut self,
        delivery: &Delivery,
    ) -> Result<crate::dispatch_liveness::decision::ContractEvidence> {
        self.records.contract_evidence(delivery)
    }

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
use tl3_mergify::decide::{facts, translate::Kind};
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

impl RequiredCheck {
    /// The check run reports this context, from the expected app if one is named.
    pub(crate) fn matches(&self, check: &CheckRun) -> bool {
        self.context == check.name
            && self
                .app
                .is_none_or(|id| check.app.as_ref().is_some_and(|app| app.id == id))
    }
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
impl Evidence {
    /// The evidence without its payload, as the core decides on it.
    pub(crate) fn kind(&self) -> Kind {
        match self {
            Self::ChecksUnavailable { .. } => Kind::ChecksUnavailable,
            Self::CheckMissing { .. } => Kind::CheckMissing,
            Self::QueueBlocked { .. } => Kind::QueueBlocked,
            Self::CheckReported { .. } => Kind::CheckReported,
            Self::Merged { checks_passed, .. } => Kind::Merged {
                checks_passed: *checks_passed,
            },
            Self::CiFailed {
                head_check, infra, ..
            } => Kind::CiFailed {
                head_check: *head_check,
                infra: *infra,
            },
            Self::Conflict { .. } => Kind::Conflict,
            Self::DequeuedByCommand { .. } => Kind::DequeuedByCommand,
            Self::DequeuedByPerson { .. } => Kind::DequeuedByPerson,
            Self::ClosedByPerson { .. } => Kind::ClosedByPerson,
            Self::PushByOther { .. } => Kind::PushByOther,
        }
    }
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
        self.evidence.kind().setup()
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
        if !completion_core::hold_candidate(completion_hold(&self.evidence)) {
            return Ok(false);
        }
        let Some(node) = self.node(snapshot)? else {
            return Ok(completion_core::unresolved_hold(None));
        };
        let observed = snapshot
            .rows
            .iter()
            .find(|row| row.table == "queue_facts" && row.id == self.identity());
        let resumed = snapshot
            .rows
            .iter()
            .find(|row| row.table == "queue_resumes" && row.id == node.0.to_string());
        Ok(completion_core::unresolved_hold(observed.zip(resumed).map(
            |(observed, resumed)| (observed.version.get(), resumed.version.get()),
        )))
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
    completion_core::head(current).into()
}

use tl3_mergify::decide::completion as completion_core;
use tl3_mergify::decide::enqueue::{self as lifecycle, Requeue};
use tl3_mergify::decide::{heal as heal_core, plan as execute_core};

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

pub struct Adapter {
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
    pub fn new(clients: BTreeMap<u64, GitHub>, epoch: i64, records: &Path) -> Result<Self> {
        std::fs::create_dir_all(records)?;
        Ok(Self {
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
        let matches: Vec<_> = self
            .policies
            .iter()
            .map(|(repo, _)| *repo == delivery.repository)
            .collect();
        let Some(policy) = heal_core::policy(&matches).map(|index| &self.policies[index].1) else {
            return Ok(false);
        };
        let rules = &policy.rules;
        if !heal_core::available(pull.state == "open", pull.merged_at.is_some()) {
            return Ok(false);
        }
        let ours = match &self.branches {
            Some(branches) => Some(branches.matches(delivery, &pull.head.branch)?),
            None => None,
        };
        if !heal_core::owned(ours) {
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
        let pending = heal_core::pending(heal.as_ref().map(|saved| heal_record(saved, head)));
        // Read the world only when there is something to do.
        if !heal_core::needs_phase(gaps.is_empty(), pending)
            || !heal_phase(Self::phase(delivery, snapshot)?)
        {
            return Ok(false);
        }
        let mut edited = false;
        let request = match heal_core::request(gaps.is_empty(), delivery.request_text.is_some()) {
            heal_core::Request::None => None,
            heal_core::Request::Saved => delivery.request_text.clone(),
            heal_core::Request::Issue => {
                self.runtime
                    .block_on(github.issue(&repo, delivery.issue))?
                    .body
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
        let Some(heal) = heal.filter(|saved| heal_core::pending(Some(heal_record(saved, head))))
        else {
            return Ok(edited);
        };
        let runs = if !heal_core::read_runs(policy.reruns.is_empty()) {
            Vec::new()
        } else {
            self.runtime
                .block_on(github.workflow_runs(&repo, head))
                .map_err(Error::from)?
        };
        for run in body_edit_reruns(&runs, head, &policy.reruns) {
            if let heal_core::Rerun::KeepPending(error) =
                heal_core::rerun_result(self.runtime.block_on(github.rerun_failed_jobs(&repo, run)))
            {
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
        if execute_core::fenced(delivery.epoch, self.epoch) {
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
        } else if state
            .pull_request
            .body
            .as_deref()
            .unwrap_or_default()
            .contains(&proof)
        {
            completion_core::Proof::Present
        } else {
            completion_core::Proof::Missing
        };
        if completion_core::proof_missing(proof) {
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
        let labels: Vec<_> = issue
            .labels
            .iter()
            .map(|label| completion_label(&label.name))
            .collect();
        let retry = completion_core::retry(
            completion_core::has_label(&labels, completion_core::Label::Queue),
            state.state.into(),
        );
        match completion_core::dequeue_gate(
            retry,
            state.dequeue_rule_concluded,
            completion_core::has_label(&labels, completion_core::Label::Dequeued),
        ) {
            completion_core::DequeueGate::Done => return Ok(false),
            completion_core::DequeueGate::Pending => return Ok(true),
            completion_core::DequeueGate::Events => {}
        }
        let events = self.runtime.block_on(
            self.clients[&delivery.repository.installation]
                .issue_events(&full_name(&delivery.repository), enqueue(delivery)?.0),
        )?;
        let events: Vec<_> = events
            .iter()
            .map(|event| completion_core::LabelEvent {
                id: event.id,
                label: event
                    .label
                    .as_ref()
                    .map_or(completion_core::Label::Other, |label| {
                        completion_label(&label.name)
                    }),
                action: if event.event == "labeled" {
                    completion_core::LabelAction::Labeled
                } else {
                    completion_core::LabelAction::Other
                },
                at: completion_core::time(
                    event.created_at.timestamp(),
                    event.created_at.timestamp_subsec_nanos(),
                ),
            })
            .collect();
        let label_event =
            |label| completion_core::newest(&events, label).map(|index| events[index]);
        Ok(lifecycle::dequeue_pending(
            retry,
            state.dequeue_rule_concluded,
            true,
            label_event(completion_core::Label::Dequeued).map(|event| lifecycle::LabelEvent {
                id: event.id,
                labeled: completion_core::labeled(event.action),
                at: event.at.into(),
            }),
            label_event(completion_core::Label::Queued).map(|event| event.id),
            state
                .check
                .as_ref()
                .and_then(|check| check.completed_at)
                .map(|time| {
                    completion_core::time(time.timestamp(), time.timestamp_subsec_nanos()).into()
                }),
        ))
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
                        return Ok(
                            match completion_core::resolve(result.into(), observed.is_some()) {
                                completion_core::Resolution::Found {
                                    ack,
                                    conflict,
                                    progress,
                                } => {
                                    if conflict {
                                        observations.push(Observation::Kernel(
                                            tl3_kernel::Event::Fact {
                                                node: delivery.node(),
                                                fact: tl3_kernel::Fact::Conflict,
                                            },
                                        ));
                                    }
                                    let mut completion = match ack {
                                        completion_core::Ack::Confirmed => {
                                            Completion::confirmed(observations)
                                        }
                                        completion_core::Ack::Retryable => Completion {
                                            diagnostic: None,
                                            ack: Ack::Retryable,
                                            failure_class: None,
                                            observations,
                                            progress: None,
                                        },
                                        completion_core::Ack::Permanent(failure) => {
                                            Completion::permanent(failure.class())
                                        }
                                    };
                                    if progress {
                                        completion.progress = observed.as_ref().map(|state| {
                                            crate::executor::ProgressToken::Mergify(Progress::from(
                                                state,
                                            ))
                                        });
                                    }
                                    Reconciliation::Found(Box::new(completion))
                                }
                                completion_core::Resolution::Pending => Reconciliation::Pending,
                                completion_core::Resolution::Absent => Reconciliation::Absent,
                            },
                        );
                    }
                    _ => return Err(Error::Invalid("unexpected enqueue reconciliation effect")),
                }
            }
            match requested.ok_or(Error::Invalid("enqueue reconciliation needs a read"))? {
                lifecycle::Need::Queue => {
                    let (state, facts) = self.read(delivery, snapshot)?;
                    observations = facts;
                    core.confirmed = completion_core::confirmed(completion_receipt(
                        &self.records.reconcile(delivery)?,
                    ));
                    read = Some(lifecycle::QueueRead {
                        head: lifecycle_head(state.pull_request.head.sha == enqueue(delivery)?.1),
                        queue: state.state,
                        merged: state.pull_request.merged_at.is_some(),
                        conflicts: completion_core::conflicts(state.pull_request.mergeable),
                        held: completion_core::held(state.state.into(), None)
                            .map_or_else(|| self.unresolved_human_hold(delivery, snapshot), Ok)?,
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
                    if completion_core::person_events(baseline.is_some(), None)
                        .map_or_else(|| self.repaired_head(delivery, snapshot), Ok)?
                    {
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
                            let Some(event) =
                                completion_core::person_event(completion_evidence(&fact.evidence))
                            else {
                                continue;
                            };
                            let source = if let Some(snapshot) = snapshot {
                                completion_core::Freshness::Snapshot {
                                    unresolved: fact.unresolved_hold(snapshot)?,
                                }
                            } else {
                                completion_core::Freshness::Intent {
                                    last_event: baseline.as_ref().map(|attempt| attempt.last_event),
                                }
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
                            &current
                                .labels
                                .iter()
                                .map(|label| completion_label(&label.name))
                                .collect::<Vec<_>>(),
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
}
impl Executor for Adapter {
    fn contract_evidence(
        &mut self,
        delivery: &Delivery,
    ) -> Result<crate::dispatch_liveness::decision::ContractEvidence> {
        use crate::dispatch_liveness::decision::ContractEvidence;
        if self.records.contract_evidence(delivery)? == ContractEvidence::Ending {
            return Ok(ContractEvidence::Ending);
        }
        Ok(if self.baseline(delivery)?.is_some() {
            ContractEvidence::RecordedIntent
        } else {
            ContractEvidence::Absent
        })
    }

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
        match execute_core::start(execution_receipt(self.read_enqueue(delivery)?)) {
            execute_core::Start::Found {
                value: completion,
                read_receipt,
            } => {
                if read_receipt
                    && execute_core::needs_delivery(matches!(
                        self.records.reconcile(delivery)?,
                        Reconciliation::Found(_)
                    ))
                {
                    self.records
                        .deliver(delivery, Completion::confirmed(vec![]))?;
                }
                return Ok(*completion);
            }
            execute_core::Start::Pending => {
                return Err(Error::Invalid("Mergify acceptance is still unknown"));
            }
            execute_core::Start::Proceed => {}
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
        let attempt = match execute_core::attempt(self.baseline(delivery)?) {
            execute_core::Attempt::Saved(attempt) => attempt,
            execute_core::Attempt::New => Attempt {
                before: state.check.clone(),
                last_event: execute_core::last_event(
                    &self
                        .runtime
                        .block_on(github.issue_events(&repo, number))?
                        .iter()
                        .map(|event| event.id)
                        .collect::<Vec<_>>(),
                ),
                requeue: execute_core::ready(
                    issue.labels.iter().any(|label| label.name == QUEUE_LABEL),
                    Requeue::Ready,
                ),
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
        loop {
            let effect = match execute_core::next(effects.pop_front()) {
                execute_core::Next::Effect(effect) => effect,
                execute_core::Next::Applied => {
                    self.records.write(
                        &self.records.path(&delivery.key).with_extension("applied"),
                        b"true",
                    )?;
                    break;
                }
            };
            match effect {
                lifecycle::Effect::SaveIntent => {
                    let plan = execute_core::intent();
                    for step in [plan.0, plan.1] {
                        match step {
                            execute_core::IntentEffect::Save => self.records.write(
                                &self.records.path(&delivery.key).with_extension("intent"),
                                &serde_json::to_vec(&attempt)?,
                            )?,
                            execute_core::IntentEffect::Fence => self.fence(delivery)?,
                        }
                    }
                }
                lifecycle::Effect::Read(lifecycle::Need::Command) => {
                    if let Some(completion) =
                        execute_core::found(execution_receipt(self.queue_command(delivery, None)?))
                    {
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
                    return match execute_core::found(execution_receipt(
                        self.completion(delivery, self.snapshot()?.as_ref())?,
                    )) {
                        Some(completion) => Ok(*completion),
                        None => Err(Error::Invalid("PR changed during enqueue")),
                    };
                }
                lifecycle::Effect::Error(failure) => return Err(lifecycle_error(failure)),
                _ => return Err(Error::Invalid("unexpected enqueue execution effect")),
            }
        }
        match execute_core::finish(execution_receipt(self.read_enqueue(delivery)?)) {
            execute_core::Final::Found {
                value: completion,
                deliver,
            } => {
                if deliver {
                    self.records
                        .deliver(delivery, Completion::confirmed(vec![]))?;
                }
                Ok(*completion)
            }
            execute_core::Final::Unknown => Err(Error::Invalid(
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
    // Exact paths are grouped at the upstream boundary. BTreeMap retains the
    // original path order, and each group retains input order for equal ids.
    let mut groups: BTreeMap<&str, Vec<heal_core::Run>> = BTreeMap::new();
    for run in runs {
        let file = run.path.rsplit('/').next().unwrap_or(&run.path);
        groups.entry(&run.path).or_default().push(heal_core::Run {
            id: run.id,
            current_head: run.head_sha == head,
            pull_request: run.event == "pull_request",
            listed: workflows.iter().any(|listed| listed == file),
            completed: run.status == "completed",
            failed: matches!(
                run.conclusion.as_deref(),
                Some("failure" | "cancelled" | "timed_out" | "startup_failure")
            ),
        });
    }
    groups
        .into_values()
        .filter_map(|runs| {
            heal_core::newest(&runs)
                .filter(|&index| heal_core::failed(&runs[index]))
                .map(|index| runs[index].id)
        })
        .collect()
}

/// #447 review: a heal touches only a node in Publishing or Queued.
pub(crate) fn heal_phase(phase: Option<tl3_kernel::Phase>) -> bool {
    heal_core::phase(match phase {
        Some(tl3_kernel::Phase::Publishing) => heal_core::Phase::Publishing,
        Some(tl3_kernel::Phase::Queued) => heal_core::Phase::Queued,
        _ => heal_core::Phase::Other,
    })
}

/// #447 review: whether every check on the head, other than Mergify's own,
/// has completed. Until then a check suite read can't show a blocked queue.
pub(crate) fn checks_settled(state: &tl3_mergify::Observation) -> bool {
    facts::checks_settled(&tl3_mergify::views(
        &state.check_runs,
        &state.pull_request.head.sha,
        |_| false,
    ))
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

/// `open` names the open missing-check incidents on the expected head.
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
fn checks_passed(checks: &[CheckRun], sha: &str) -> bool {
    facts::checks_passed(&tl3_mergify::views(checks, sha, |_| false))
}

fn suite_statuses(suites: &[tl3_github::CheckSuite]) -> Vec<tl3_mergify::decide::Status> {
    suites
        .iter()
        .map(|suite| tl3_mergify::decide::Status::parse(&suite.status))
        .collect()
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
                    && let Some(facts::PersonFact::Dequeued { event }) =
                        facts::person_fact(facts::IssueEvent {
                            id: event.id,
                            actor: actor(Some(&event.actor)),
                            kind: event_kind(&event.event, Some(event.label.name.as_str())),
                        })
                {
                    facts.push(QueueFact {
                        repository: repo.clone(),
                        number: item.number,
                        evidence: Evidence::DequeuedByPerson { event },
                    });
                }
            }
            for (number, event) in &changes.closed {
                if *number == item.number && actor(event.actor.as_ref()) == facts::Actor::Person {
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
fn actor(actor: Option<&Actor>) -> facts::Actor {
    facts::Actor::parse(actor.map(|actor| actor.login.as_str()))
}

fn event_kind(event: &str, label: Option<&str>) -> facts::EventKind {
    match event {
        "unlabeled" => facts::EventKind::Unlabeled {
            queue_label: label == Some(QUEUE_LABEL),
        },
        "closed" => facts::EventKind::Closed,
        "merged" => facts::EventKind::Merged,
        _ => facts::EventKind::Other,
    }
}

fn person_facts(
    repo: &Repository,
    number: u64,
    events: &[tl3_github::IssueActivity],
    merged: bool,
) -> Vec<QueueFact> {
    let events: Vec<_> = events
        .iter()
        .map(|event| facts::IssueEvent {
            id: event.id,
            actor: actor(event.actor.as_ref()),
            kind: event_kind(
                &event.event,
                event.label.as_ref().map(|label| label.name.as_str()),
            ),
        })
        .collect();
    facts::person_facts(&events, merged)
        .map(|fact| QueueFact {
            repository: repo.clone(),
            number,
            evidence: match fact {
                facts::PersonFact::Dequeued { event } => Evidence::DequeuedByPerson { event },
                facts::PersonFact::Closed { event } => Evidence::ClosedByPerson { event },
            },
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
mod kind_tests {
    use super::*;

    /// `Evidence::kind` matches every variant with no wildcard, so the compiler
    /// keeps it total. This pins each arm and the flags it copies (#735).
    /// Kani can't take it: the controller crate pulls in sqlx and Postgres.
    #[test]
    fn every_evidence_maps_to_its_kind() {
        let head = || "abc".to_string();
        let required = || RequiredCheck {
            context: "Rust".into(),
            app: Some(15368),
        };
        let mut cases = vec![
            (
                Evidence::ChecksUnavailable {
                    permissions: vec!["checks".into()],
                },
                Kind::ChecksUnavailable,
            ),
            (
                Evidence::CheckMissing {
                    head: head(),
                    required: required(),
                },
                Kind::CheckMissing,
            ),
            (
                Evidence::QueueBlocked {
                    head: head(),
                    conditions: vec!["scopes".into()],
                    failed: vec![],
                },
                Kind::QueueBlocked,
            ),
            (
                Evidence::CheckReported {
                    head: head(),
                    required: required(),
                },
                Kind::CheckReported,
            ),
            (
                Evidence::Conflict {
                    check: 1,
                    completed_at: None,
                    head: head(),
                },
                Kind::Conflict,
            ),
            (
                Evidence::DequeuedByCommand {
                    check: 2,
                    completed_at: None,
                    head: head(),
                },
                Kind::DequeuedByCommand,
            ),
            (
                Evidence::DequeuedByPerson { event: 3 },
                Kind::DequeuedByPerson,
            ),
            (Evidence::ClosedByPerson { event: 4 }, Kind::ClosedByPerson),
            (Evidence::PushByOther { head: head() }, Kind::PushByOther),
        ];
        for checks_passed in [false, true] {
            cases.push((
                Evidence::Merged {
                    head: head(),
                    checks_passed,
                },
                Kind::Merged { checks_passed },
            ));
        }
        for head_check in [false, true] {
            for infra in [false, true] {
                cases.push((
                    Evidence::CiFailed {
                        check: 5,
                        completed_at: None,
                        head: head(),
                        infra,
                        head_check,
                        details: Default::default(),
                    },
                    Kind::CiFailed { head_check, infra },
                ));
            }
        }
        assert_eq!(cases.len(), 15);
        for (evidence, kind) in cases {
            assert_eq!(evidence.kind(), kind, "{evidence:?}");
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

fn completion_check(
    before: &Option<CheckRun>,
    current: &Option<CheckRun>,
) -> completion_core::CheckComparison {
    match (before, current) {
        (None, None) => completion_core::CheckComparison::BothMissing,
        (Some(before), Some(current)) => completion_core::CheckComparison::BothPresent {
            fields: completion_core::CheckFields {
                id: before.id == current.id,
                pull_requests: before.pull_requests == current.pull_requests,
                name: before.name == current.name,
                head_sha: before.head_sha == current.head_sha,
                external_id: before.external_id == current.external_id,
                details_url: before.details_url == current.details_url,
                status: before.status == current.status,
                conclusion: before.conclusion == current.conclusion,
                app: before.app == current.app,
                output: before.output == current.output,
            },
        },
        _ => completion_core::CheckComparison::OneMissing,
    }
}

fn completion_hold(evidence: &Evidence) -> completion_core::HoldEvidence {
    match evidence {
        Evidence::DequeuedByPerson { .. } | Evidence::ClosedByPerson { .. } => {
            completion_core::HoldEvidence::Person
        }
        Evidence::DequeuedByCommand { .. } => completion_core::HoldEvidence::Command,
        _ => completion_core::HoldEvidence::Other,
    }
}

fn execution_receipt(receipt: Reconciliation) -> execute_core::Receipt<Box<Completion>> {
    match receipt {
        Reconciliation::Found(value) => {
            let ack = if value.ack == Ack::Confirmed {
                execute_core::Ack::Confirmed
            } else {
                execute_core::Ack::Other
            };
            execute_core::Receipt::Found { value, ack }
        }
        Reconciliation::Pending => execute_core::Receipt::Pending,
        Reconciliation::Absent => execute_core::Receipt::Absent,
    }
}

fn heal_record(saved: &BodyHeal, head: &str) -> heal_core::Record {
    heal_core::Record {
        same_head: saved.head == head,
        reran: saved.reran,
    }
}
