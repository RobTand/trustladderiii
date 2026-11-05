#![forbid(unsafe_code)]

#[cfg(all(loom, not(test)))]
compile_error!("the loom feature is only supported for library unit tests");

#[cfg(all(test, loom))]
extern crate self as tl3_controller;

pub mod adoption;
mod approval_delivery;
pub mod findings;

pub mod adapters;
mod blocked;
pub mod board;
mod branches;
pub mod budget;
pub mod classifier;
pub mod cli;
pub mod clock;
pub mod console;
pub use tl3_contracts as contracts;
mod decisions;
mod dispatch_liveness;
pub mod evidence;
pub mod executor;
mod github_messages;
mod goals;
mod http;
pub mod ingress;
pub mod input_errors;
mod kernel_boundary;
pub mod looks;
#[cfg(all(test, loom))]
#[path = "../tests/loom/mod.rs"]
mod loom_tests;
pub mod memory;
pub mod merge_dependencies;
mod narrowing;
mod notices;
pub mod postmortem_reply;
mod postmortems;
mod quota_wake;
#[cfg(all(test, loom))]
#[path = "../tests/support/ownership.rs"]
mod startup_ownership;
pub use postmortems::PostmortemPace;
pub mod pr_body;
pub mod priority;
pub mod production;
pub mod prompt_render;
pub mod prompts;
mod protected_requests;
pub mod publication_failure;
mod question_findings;
pub mod redaction;
pub mod release;
pub mod replay;
mod review_stops;
pub mod reviewer;
mod scope;
mod snapshot;
pub mod status;
pub mod status_web;
pub mod steering;
pub mod store_stats;
mod sync;
pub mod worker_inputs;

use executor::{
    Completion, Delivery, Observation, Output as DeliveryOutput, Reconciliation, Registry,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use tl3_kernel::{Ack, Event, Fact, HeadId, NodeId, Out, Reply, World};
use tl3_store::{Commit, Effect, EffectState, EffectTransition, RowChange, Snapshot, Store};

pub type Result<T> = std::result::Result<T, Error>;
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("replay-check: {0}")]
    Replay(String),
    #[error("{0}")]
    Store(#[from] tl3_store::Error),
    #[error("{0}")]
    Config(#[from] tl3_config::Error),
    #[error("invalid controller data: {0}")]
    Json(#[from] serde_json::Error),
    #[error("invalid stored controller data: {0}")]
    StoredJson(#[source] serde_json::Error),
    #[error("controller I/O failed")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    WorkerSource(#[from] adapters::workers::SourceError),
    #[error("upstream adapter failed: {0}")]
    Adapter(#[source] Box<dyn std::error::Error + Send + Sync>),
    #[error("{0}")]
    WorkerInputs(#[from] worker_inputs::Failure),
    #[error("worker reply has no matching durable dispatch")]
    WorkerDispatchMissing,
    #[error("{0}")]
    Invalid(&'static str),
    #[error("merge prerequisite update is missing its issue or pull request number")]
    PrerequisiteNumber,
    #[error("{0}")]
    CounterExhausted(String),
    #[error("Hindsight returned HTTP {0}")]
    HindsightStatus(u16),
    #[error("writer drafts failed the message checker after two reruns")]
    MessageRejected,
    #[error(
        "repository {owner}/{name} (installation {installation}) has open requests; keep it in github.repos until drained"
    )]
    RepositoryInFlight {
        owner: String,
        name: String,
        installation: u64,
    },
    #[error("upstream permanently failed: {class}")]
    Permanent {
        class: &'static str,
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },
    #[error("redaction helper infrastructure failed")]
    RedactionInfra(#[source] std::io::Error),
    #[error("upstream quota exhausted")]
    Quota { retry_after: std::time::Duration },
    /// The enqueue guard is waiting for required checks to report. This is
    /// evidence of a legitimate wait, not a failure (Q52).
    #[error("required head checks are still pending")]
    ChecksPending,
    #[error("approval check dispatch outcome unknown")]
    ApprovalAmbiguous,
}

fn decode_stored<T: serde::de::DeserializeOwned>(value: Value) -> Result<T> {
    serde_json::from_value(value).map_err(Error::StoredJson)
}

impl Error {
    fn is_github_not_found(&self) -> bool {
        publication_failure::http_status(self) == Some(404)
    }
}

impl From<tl3_github::Error> for Error {
    fn from(error: tl3_github::Error) -> Self {
        match error {
            tl3_github::Error::RateLimited { retry_after } => Self::Quota { retry_after },
            error => match error.failure_class() {
                tl3_github::FailureClass::Permanent(class) => Self::Permanent {
                    class,
                    source: Box::new(error),
                },
                _ => Self::Adapter(Box::new(error)),
            },
        }
    }
}

impl From<tl3_worktrees::Error> for Error {
    fn from(error: tl3_worktrees::Error) -> Self {
        match error.permanent_class() {
            Some(class) => Self::Permanent {
                class,
                source: Box::new(error),
            },
            None => Self::Adapter(Box::new(error)),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Repository {
    pub owner: String,
    pub name: String,
    pub installation: u64,
}
impl PartialEq for Repository {
    fn eq(&self, other: &Self) -> bool {
        self.installation == other.installation
            && self.owner.eq_ignore_ascii_case(&other.owner)
            && self.name.eq_ignore_ascii_case(&other.name)
    }
}
impl Eq for Repository {}
#[derive(Clone, Debug)]
pub struct Bootstrap {
    pub repos: Vec<Repository>,
    pub rob: u64,
    pub operations: Option<tl3_config::OperationsIssue>,
    pub slots: u8,
    pub pool: u8,
    /// Q54: `limits.fail_fast`, set on the stored world at every open.
    pub fail_fast: bool,
}
impl Bootstrap {
    pub fn github_repositories(&self) -> Vec<Repository> {
        let mut repos = self.repos.clone();
        if let Some(o) = &self.operations {
            let repo = Repository {
                owner: o.owner.clone(),
                name: o.name.clone(),
                installation: o.installation_id,
            };
            if !repos.contains(&repo) {
                repos.push(repo);
            }
        }
        repos
    }
    pub fn from_config(config: &tl3_config::Config) -> Result<Self> {
        Ok(Self {
            repos: config
                .github
                .repos
                .iter()
                .map(|r| Repository {
                    owner: r.owner.clone(),
                    name: r.name.clone(),
                    installation: r.installation_id,
                })
                .collect(),
            rob: config.github.rob.id,
            operations: config.github.operations.clone(),
            slots: config
                .limits
                .w
                .try_into()
                .map_err(|_| Error::Invalid("slot count exceeds kernel range"))?,
            pool: config.limits.agent_pool,
            fail_fast: config.limits.fail_fast,
        })
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LabelledIssue {
    pub delivery: String,
    pub owner: String,
    pub repo: String,
    pub installation: u64,
    pub number: u64,
    #[serde(default)]
    pub title: String,
    pub actor: u64,
    pub label: String,
    /// The issue's labels when it was labelled, for its priority (#416, #417).
    /// Omitted when empty, so stored inputs keep their older shape (#375).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub labels: Vec<String>,
}

/// An issue's title or labels changed; its request's priority may follow (Q39).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct IssueChange {
    pub delivery: String,
    pub owner: String,
    pub repo: String,
    pub installation: u64,
    pub number: u64,
    pub title: String,
    pub labels: Vec<String>,
    /// When GitHub last changed the issue, which orders its edits.
    pub updated_at: chrono::DateTime<chrono::Utc>,
}
/// The newest issue edit seen for a request (Q39), by request node.
const ISSUE_EDITS: &str = "issue_edits";

/// Requests whose issue's same-second edits disagreed (Q39), by request node.
/// Neither edit is applied; a timer rereads the issue until it settles.
const PRIORITY_RECONCILE: &str = "priority_reconcile";

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct Reconcile {
    pub(crate) repository: Repository,
    pub(crate) number: u64,
    /// The updated_at the disagreeing edits share.
    pub(crate) updated_at: chrono::DateTime<chrono::Utc>,
    /// Which reconciliation of this request's issue, so two never share keys.
    pub(crate) generation: u32,
    /// Rereads so far.
    pub(crate) attempts: u32,
    /// When the next reread is due, in Unix milliseconds.
    pub(crate) next_due_ms: u64,
}

#[derive(Serialize, Deserialize)]
struct IssueEdit {
    updated_at: chrono::DateTime<chrono::Utc>,
    /// Same-second reconciliations started for this request (Q39).
    #[serde(default, skip_serializing_if = "is_zero")]
    reconciliations: u32,
}

fn is_zero(n: &u32) -> bool {
    *n == 0
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Request {
    repo: Repository,
    issue: u64,
    #[serde(default)]
    title: String,
    issue_id: i64,
    #[serde(default)]
    source: Option<String>,
    #[serde(default)]
    proposed: Option<adoption::ProposedPullRequest>,
    /// P0–P3 from the issue, read once at intake (#416); `None` for unranked
    /// and older rows, and omitted then so the row keeps its older shape (#375).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    priority: Option<u8>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Boundary {
    WorkerWait,
    BeforeDispatch,
    AfterDispatch,
    BeforeReconcile,
    AfterReconcile,
}

pub use tl3_store::Ownership;

pub struct Controller {
    store: Store,
    adapters: Registry,
    bootstrap: Bootstrap,
    github_translators: Vec<ingress::Translation>,
    clock: Box<dyn clock::Clock>,
    held_effects: BTreeMap<executor::Upstream, BTreeMap<String, String>>,
    /// Persisted inputs are treated as already reported on startup. Diagnostics
    /// stay bounded by the pending queue and cannot repeat after a restart.
    contention_notices: BTreeSet<(String, String)>,
    #[cfg(test)]
    refusal_injection: std::cell::RefCell<input_errors::RefusalInjection>,
    allowances: tl3_config::Allowances,
    quota_models: Option<tl3_config::Models>,
    quota_horizon_ms: u64,
    /// tl3d judges owed postmortems at this pace (#403); see `run_postmortems`.
    postmortems: Option<PostmortemPace>,
    /// The revision tl3d started at: failed postmortem runs before it never
    /// pause the runner (#486).
    postmortem_since: i64,
    /// Starting credit reserve by priority, P0 first (#416). Empty keeps the kernel's.
    priority_reserve: BTreeMap<u8, u8>,
    request_protection: Vec<(Repository, Vec<String>)>,
    delegated_attributions: Vec<String>,
}
impl Controller {
    pub fn open(store: Store, adapters: Registry, bootstrap: Bootstrap) -> Result<Self> {
        Self::open_with_clock(store, adapters, bootstrap, clock::SystemClock)
    }

    pub fn open_with_clock(
        store: Store,
        adapters: Registry,
        bootstrap: Bootstrap,
        clock: impl clock::Clock + 'static,
    ) -> Result<Self> {
        Self::open_owned_with_clock(Ownership::claim(store)?, adapters, bootstrap, clock)
    }

    pub fn open_owned(
        ownership: Ownership,
        adapters: Registry,
        bootstrap: Bootstrap,
    ) -> Result<Self> {
        Self::open_owned_with_clock(ownership, adapters, bootstrap, clock::SystemClock)
    }

    fn open_owned_with_clock(
        ownership: Ownership,
        adapters: Registry,
        bootstrap: Bootstrap,
        clock: impl clock::Clock + 'static,
    ) -> Result<Self> {
        let mut store = ownership.into_store();
        let snapshot = store.load()?;
        if snapshot.rows.is_empty() {
            let world = World::new(
                &vec![bootstrap.slots; bootstrap.repos.len()],
                bootstrap.pool,
            );
            let mut commit = Commit::new("controller:init", json!({"initialize": 1}), &snapshot);
            commit.rows = snapshot::changes(&snapshot::Rows::new(), snapshot::encode(&world)?);
            for (index, repo) in bootstrap.repos.iter().enumerate() {
                store.intern_repo(&repo.owner, &repo.name)?;
                commit
                    .rows
                    .push(put("repos", index.to_string(), serde_json::to_value(repo)?));
            }
            store.commit(&commit)?;
        } else {
            snapshot::decode(&snapshot)?;
        }
        let snapshot = store.load()?;
        let mut backfill = Commit::new(
            format!("steering:backfill:{}", snapshot.revision.get()),
            json!({"comment_backfill": true}),
            &snapshot,
        );
        for row in snapshot.rows.iter().filter(|r| {
            r.table == "github_comments"
                && r.value.get("classification").is_none()
                && r.value["ignored"] != true
        }) {
            if !snapshot
                .rows
                .iter()
                .any(|r| r.table == "unclassified_comments" && r.id == row.id)
            {
                backfill.rows.push(put(
                    "unclassified_comments",
                    row.id.clone(),
                    row.value.clone(),
                ));
            }
        }
        if !backfill.rows.is_empty() {
            store.commit(&backfill)?;
        }
        let mut controller = Self {
            store,
            adapters,
            bootstrap,
            github_translators: Vec::new(),
            clock: Box::new(clock),
            held_effects: BTreeMap::new(),
            contention_notices: snapshot
                .rows
                .iter()
                .filter(|r| r.table == "inbox" || r.table == input_errors::PENDING_KERNEL)
                .map(|r| (r.table.clone(), r.id.clone()))
                .collect(),
            #[cfg(test)]
            refusal_injection: Default::default(),
            allowances: tl3_config::Allowances::default(),
            quota_models: None,
            quota_horizon_ms: 6 * 3_600_000,
            postmortems: None,
            postmortem_since: 0,
            priority_reserve: BTreeMap::new(),
            request_protection: Vec::new(),
            delegated_attributions: Vec::new(),
        };
        controller.reconcile_capacity()?;
        controller.cleanup_ended_looks()?;
        controller.recover_postmortems()?;
        controller.reconcile_worker_waits()?;
        controller.migrate_model_quota_deadlines()?;
        Ok(controller)
    }

    fn reconcile_capacity(&mut self) -> Result<()> {
        let snapshot = self.store.load()?;
        let mut world = snapshot::decode(&snapshot)?;
        let mut repositories = stored_repositories(&snapshot)?;
        if repositories.len() != world.gates.len() {
            return Err(Error::Invalid("stored repositories and gates differ"));
        }
        for (index, repository) in repositories.iter().enumerate() {
            if !self.bootstrap.repos.contains(repository)
                && world.nodes.iter().any(|node| {
                    node.repo == tl3_kernel::RepoId(index as u64) && !node.phase.is_closed()
                })
            {
                return Err(Error::RepositoryInFlight {
                    owner: repository.owner.clone(),
                    name: repository.name.clone(),
                    installation: repository.installation,
                });
            }
        }
        let configured = self.bootstrap.slots;
        let configured_pool = self.bootstrap.pool;
        if configured_pool < world.pool_capacity
            && world.nodes.iter().any(|node| !node.phase.is_closed())
        {
            return Err(Error::Invalid(
                "limits.agent_pool cannot decrease while requests are in flight; drain all repositories first",
            ));
        }
        let pool_change = (configured_pool != world.pool_capacity)
            .then(|| json!({"stored": world.pool_capacity, "configured": configured_pool}));
        world.pool_capacity = configured_pool;
        // Q54: the kernel reads the flag at each retry, so open requests follow it too.
        let fail_fast_change = (self.bootstrap.fail_fast != world.fail_fast)
            .then(|| json!({"stored": world.fail_fast, "configured": self.bootstrap.fail_fast}));
        world.fail_fast = self.bootstrap.fail_fast;
        let mut changes = Vec::new();
        for (repo, gate) in world.gates.iter_mut().enumerate() {
            if configured == gate.w {
                continue;
            }
            if configured < gate.w
                && world.nodes.iter().any(|node| {
                    node.repo == tl3_kernel::RepoId(repo as u64)
                        && !matches!(node.phase, tl3_kernel::Phase::Closed(_))
                })
            {
                return Err(Error::Invalid(
                    "limits.w cannot decrease while requests are in flight; drain the repository first",
                ));
            }
            changes.push(json!({"repo": repo, "stored": gate.w, "configured": configured}));
            gate.w = configured;
        }
        let mut additions = Vec::new();
        for repository in &self.bootstrap.repos {
            if !repositories.contains(repository) {
                let index = repositories.len();
                repositories.push(repository.clone());
                world.gates.push(tl3_kernel::Gate { w: configured });
                additions.push((index, repository));
            }
        }
        if changes.is_empty()
            && pool_change.is_none()
            && fail_fast_change.is_none()
            && additions.is_empty()
        {
            return Ok(());
        }
        let mut commit = Commit::new(
            format!("controller:gates:{}", snapshot.revision.get()),
            json!({"gate_capacity": changes, "agent_capacity": pool_change,
                "fail_fast": fail_fast_change, "repositories": additions}),
            &snapshot,
        );
        if !self.kernel_headroom(&snapshot, &world, &mut commit)? {
            self.store.commit(&commit)?;
            return Ok(());
        }
        for (index, repository) in additions {
            self.store
                .intern_repo(&repository.owner, &repository.name)?;
            commit.rows.push(put(
                "repos",
                index.to_string(),
                serde_json::to_value(repository)?,
            ));
        }
        let (world, outputs) = tl3_kernel::settle_world(world);
        self.record_kernel_result(&snapshot, world, outputs, &mut commit)?;
        self.store.commit(&commit)?;
        Ok(())
    }

    pub fn register_github_translator(
        &mut self,
        name: &str,
        translator: impl ingress::GitHubTranslator + 'static,
    ) {
        assert!(
            !name.is_empty()
                && self
                    .github_translators
                    .iter()
                    .all(|entry| entry.name != name),
            "unique GitHub translator name required"
        );
        self.github_translators.push(ingress::Translation {
            name: name.into(),
            translator: Box::new(translator),
        });
    }

    pub fn now(&self) -> std::time::SystemTime {
        self.clock.now()
    }

    pub fn effect(&mut self, key: &str) -> Result<Option<Effect>> {
        Ok(self.store.effect(key)?)
    }

    pub fn snapshot(&mut self) -> Result<Snapshot> {
        Ok(self.store.load()?)
    }
    pub fn world(&mut self) -> Result<World> {
        snapshot::decode(&self.store.load()?)
    }

    /// The starting credit reserve by priority (#416). Off by default.
    pub fn set_priority_credit_reserve(&mut self, limits: &tl3_config::Limits) {
        self.priority_reserve = (0..=3)
            .filter_map(|priority| Some((priority, limits.credit_reserve_for(Some(priority))?)))
            .collect();
    }

    pub fn intake(&mut self, issue: LabelledIssue) -> Result<bool> {
        self.intake_proposed(issue, None)
    }

    fn intake_proposed(
        &mut self,
        issue: LabelledIssue,
        proposed: Option<adoption::ProposedPullRequest>,
    ) -> Result<bool> {
        if self.intake_repository(&issue).is_none() {
            return Ok(false);
        }
        let accepted = self.submit_kernel_input(
            format!("intake:{}", issue.delivery),
            input_errors::PendingKernelInput::Intake { issue, proposed },
        )?;
        self.retry_held_effects(executor::Upstream::GitHub);
        Ok(accepted)
    }

    fn queue_adoption_question(&mut self, question: adoption::Clarification) -> Result<()> {
        let snapshot = self.store.load()?;
        let key = question.key();
        if self
            .store
            .event_matches(&[format!("{key}:sent")], &json!({"question_sent":key}))?
        {
            return Ok(());
        }
        if snapshot
            .rows
            .iter()
            .any(|r| r.table == "adoption_questions" && r.id == key)
        {
            return Ok(());
        }
        let mut commit = Commit::new(key.clone(), json!(&question), &snapshot);
        commit
            .rows
            .push(put("adoption_questions", key, json!({"question":question})));
        self.store.commit(&commit)?;
        Ok(())
    }

    fn intake_repository(&self, issue: &LabelledIssue) -> Option<Repository> {
        if issue.actor != self.bootstrap.rob || issue.label != "tl3" {
            return None;
        }
        self.bootstrap
            .repos
            .iter()
            .find(|r| {
                r.owner.eq_ignore_ascii_case(&issue.owner)
                    && r.name.eq_ignore_ascii_case(&issue.repo)
                    && r.installation == issue.installation
            })
            .cloned()
    }

    /// Refresh an open request's pool priority (Q39) from its issue's current
    /// title and labels. The stored request keeps the priority read at intake,
    /// which sets its credit reserve (#416).
    pub fn issue_changed(&mut self, change: &IssueChange) -> Result<()> {
        let snapshot = self.store.load()?;
        // A retained intake has no request row yet. Only the path without a
        // backlog or halt may discard an edit to an issue TL3 does not track.
        if !input_errors::kernel_backlog(&snapshot)
            && !snapshot.rows.iter().any(|r| {
                r.table == "incidents"
                    && r.value["open"] == true
                    && r.value["signature"]["class"] == input_errors::COUNTER_HEADROOM
            })
            && snapshot::decode(&snapshot)?.headroom_ok()
            && !stored_repositories(&snapshot)?.iter().any(|repo| {
                repo.owner.eq_ignore_ascii_case(&change.owner)
                    && repo.name.eq_ignore_ascii_case(&change.repo)
                    && repo.installation == change.installation
                    && latest_request(&snapshot, repo, change.number).is_some()
            })
        {
            return Ok(());
        }
        self.submit_kernel_input(
            format!("github-issue:{}", change.delivery),
            input_errors::PendingKernelInput::IssueChange(change.clone()),
        )
        .map(|_| ())
    }

    /// State-table "Counter headroom": retain complete edits before comparing
    /// their priority or timestamp with a world that the halt has frozen.
    pub(crate) fn prepare_issue_change(
        &mut self,
        snapshot: &Snapshot,
        change: &IssueChange,
        commit: &mut Commit,
    ) -> Result<()> {
        let world = snapshot::decode(snapshot)?;
        if !self.kernel_headroom(snapshot, &world, commit)? {
            return Ok(());
        }
        let Some(repo) = stored_repositories(snapshot)?.into_iter().find(|r| {
            r.owner.eq_ignore_ascii_case(&change.owner)
                && r.name.eq_ignore_ascii_case(&change.repo)
                && r.installation == change.installation
        }) else {
            return Ok(());
        };
        let Some(row) = latest_request(snapshot, &repo, change.number) else {
            return Ok(());
        };
        let node = NodeId(
            row.id
                .parse()
                .map_err(|_| Error::Invalid("invalid request node"))?,
        );
        let Some(n) = world
            .nodes
            .get(node.0 as usize)
            .filter(|n| !n.phase.is_closed())
        else {
            return Ok(());
        };
        // Edits are ordered by GitHub's updated_at, not by arrival: a late or
        // replayed older edit is stale. A newer edit is recorded even when it
        // keeps the priority, so it makes the older ones stale too.
        let edit = snapshot
            .rows
            .iter()
            .find(|r| r.table == ISSUE_EDITS && r.id == row.id)
            .map(|r| decode_stored::<IssueEdit>(r.value.clone()))
            .transpose()?;
        let seen = edit.as_ref().map(|edit| edit.updated_at);
        let pending = snapshot
            .rows
            .iter()
            .any(|r| r.table == PRIORITY_RECONCILE && r.id == row.id);
        let (updated_at, priority) = (
            change.updated_at,
            priority::pool_priority(&change.title, &change.labels),
        );
        if seen.is_some_and(|seen| updated_at < seen) {
            return Ok(());
        }
        if seen == Some(updated_at) && priority != n.priority {
            // GitHub's updated_at has second precision, so two edits in one
            // second can arrive in either order. Apply neither: a timer
            // rereads the issue (priority::Controller::reconcile_due).
            if pending {
                return Ok(());
            }
            let generation = edit.map_or(0, |edit| edit.reconciliations) + 1;
            commit.rows.push(put(
                ISSUE_EDITS,
                row.id.clone(),
                json!(IssueEdit {
                    updated_at,
                    reconciliations: generation,
                }),
            ));
            commit.rows.push(put(
                PRIORITY_RECONCILE,
                row.id.clone(),
                json!(Reconcile {
                    repository: repo.clone(),
                    number: change.number,
                    updated_at,
                    generation,
                    attempts: 0,
                    next_due_ms: self.now_ms(),
                }),
            ));
            return Ok(());
        }
        if seen.is_some_and(|seen| updated_at <= seen) && priority == n.priority {
            return Ok(());
        }
        if pending && seen.is_some_and(|seen| updated_at > seen) {
            // A newer edit settles a pending same-second disagreement.
            commit.rows.push(RowChange::Delete {
                table: PRIORITY_RECONCILE.into(),
                id: row.id.clone(),
            });
        }
        if seen.is_none_or(|seen| updated_at > seen) {
            commit.rows.push(put(
                ISSUE_EDITS,
                row.id.clone(),
                json!(IssueEdit {
                    updated_at,
                    reconciliations: edit.map_or(0, |edit| edit.reconciliations),
                }),
            ));
        }
        if priority != n.priority {
            self.apply(
                snapshot,
                world,
                Event::Fact {
                    node,
                    fact: Fact::Priority(priority),
                },
                commit,
            )?;
        }
        Ok(())
    }

    fn prepare_intake(
        &mut self,
        snapshot: &Snapshot,
        issue: &LabelledIssue,
        repo: Repository,
        proposed: Option<adoption::ProposedPullRequest>,
        commit: &mut Commit,
    ) -> Result<Option<Event>> {
        let world = snapshot::decode(snapshot)?;
        if issue.number == 0 {
            return Err(Error::Invalid("issue number must be positive"));
        }
        let (index, repo) = stored_repositories(snapshot)?
            .into_iter()
            .enumerate()
            .find(|(_, stored)| *stored == repo)
            .ok_or(Error::Invalid(
                "served repository requires a snapshot migration",
            ))?;
        let repo_id = self.store.intern_repo(&repo.owner, &repo.name)?;
        let issue_id = self.store.intern_issue(repo_id, issue.number)?.get();
        let existing = latest_request(snapshot, &repo, issue.number);
        let pending = existing.and_then(|request| {
            snapshot
                .rows
                .iter()
                .find(|row| row.table == "replacement_pending" && row.id == request.id)
        });
        if existing.is_none() || pending.is_some() {
            let node = world.nodes.len().to_string();
            let source = if let Some(pending) = pending {
                let original = NodeId(
                    pending
                        .id
                        .parse()
                        .map_err(|_| Error::Invalid("invalid request node"))?,
                );
                let replacement = steering::replacement_issue(snapshot, original, &pending.value)?;
                decisions::inherit(snapshot, commit, original, NodeId(world.nodes.len() as u64))?;
                let mut link = json!({
                    "original": original, "authorized_by": self.bootstrap.rob, "issue": replacement
                });
                if let Some(delegate) = steering::replacement_delegate(snapshot, original) {
                    link["delegated_by"] = delegate.clone();
                }
                // #449: Rob's answer is the scope; ingest reads the original issue as context.
                if let steering::Task::FileRequest { question, .. } =
                    decode_stored(pending.value.clone())?
                {
                    link["narrowing"] = json!({"question": question});
                }
                commit.rows.push(put("request_links", node.clone(), link));
                for table in ["replacement_pending", "replacement"] {
                    commit.rows.push(RowChange::Delete {
                        table: table.into(),
                        id: pending.id.clone(),
                    });
                }
                replacement.body
            } else {
                None
            };
            if pending.is_none() {
                decisions::Context::new(NodeId(world.nodes.len() as u64))
                    .store(snapshot, commit)?;
            }
            let event = if let Some(proposed) = &proposed {
                Event::NewAdoption {
                    repo: tl3_kernel::RepoId(index as u64),
                    head: self.head(NodeId(world.nodes.len() as u64), &proposed.head)?,
                    priority: priority::pool_priority(&issue.title, &issue.labels),
                }
            } else {
                Event::NewRequest {
                    repo: tl3_kernel::RepoId(index as u64),
                    priority: priority::pool_priority(&issue.title, &issue.labels),
                }
            };
            commit.rows.push(put(
                "requests",
                node,
                json!(Request {
                    repo,
                    issue: issue.number,
                    title: issue.title.clone(),
                    issue_id,
                    source,
                    proposed,
                    priority: priority::issue_priority(&issue.title, &issue.labels),
                }),
            ));
            return Ok(Some(event));
        }
        Ok(None)
    }

    /// Upstream inbox keys must be stable across webhook redelivery and looks.
    pub fn observe(&mut self, key: &str, observation: Observation) -> Result<()> {
        let snapshot = self.store.load()?;
        if self.queue_message_reconciliation(&snapshot, key, &observation)? {
            return Ok(());
        }
        if let Observation::TraceLogsCapped { evicted } = observation {
            // #430: an operations note (#81), once while it stays open.
            let mut commit = Commit::new(
                format!("trace-logs-capped:{key}:{}", snapshot.revision.get()),
                json!({"trace_logs_capped": evicted}),
                &snapshot,
            );
            self.input_failed(
                &snapshot,
                "worker-trace-logs",
                None,
                "Held worker span logs",
                None,
                executor::Upstream::Workers,
                Error::Invalid(
                    "held span logs reached their disk cap; the oldest unshipped ones were dropped",
                ),
                &mut commit,
            )?;
            if !commit.rows.is_empty() {
                self.store.commit(&commit)?;
            }
            return Ok(());
        }
        let news = observation_upstream(&snapshot, &observation)?;
        if let Observation::Queue(fact) = &observation {
            let identity = fact.identity();
            if fact.recorded(&snapshot)?
                || snapshot.rows.iter().any(|row| {
                    row.table == "inbox"
                        && serde_json::from_value::<Observation>(row.value.clone())
                            .ok()
                            .is_some_and(|pending| matches!(pending, Observation::Queue(pending) if pending.identity() == identity))
                })
            {
                return Ok(());
            }
        }
        if let Observation::GitHubComment {
            repository,
            comment,
            ..
        } = &observation
        {
            let id = format!(
                "{}/{}/{}/{}",
                repository.owner.to_ascii_lowercase(),
                repository.name.to_ascii_lowercase(),
                repository.installation,
                comment.id
            );
            if self.store.comment_retired(&id)?
                || snapshot.rows.iter().any(|row| {
                    (row.table == "github_comments" && row.id == id)
                        || (row.table == "inbox" && row.id == format!("external:{key}"))
                })
            {
                self.retry_held_effects(executor::Upstream::GitHub);
                return Ok(());
            }
        }
        // Setup gaps describe current health. A fresh read after a clear must
        // not be suppressed by the receipt for an earlier observation.
        let key = if matches!(&observation, Observation::Queue(fact) if fact.setup()) {
            format!("{key}:{}", snapshot.revision.get())
        } else {
            key.to_owned()
        };
        let mut commit = Commit::new(format!("observe:{key}"), json!(&observation), &snapshot);
        self.model_success_evidence(&snapshot, &observation, &mut commit)?;
        commit
            .rows
            .push(put("inbox", format!("external:{key}"), json!(&observation)));
        self.store.commit(&commit)?;
        if let Some(source) = news {
            self.retry_held_effects(source);
        }
        Ok(())
    }

    pub(crate) fn queue_observation(
        &mut self,
        snapshot: &Snapshot,
        commit: &mut Commit,
        key: &str,
        index: usize,
        observation: &Observation,
    ) -> Result<()> {
        self.model_success_evidence(snapshot, observation, commit)?;
        // Q53 / #828: two real quota attempts can announce exactly the same
        // reset. Their durable attempt numbers distinguish endings from replays.
        let quota_key = if let Observation::Quota {
            key: held, model, ..
        } = observation
        {
            self.store
                .latest_worker_quota(held, model.as_deref())?
                .map(|ending| format!("{key}:quota-attempt:{}", ending.attempt))
        } else {
            None
        };
        let key = quota_key.as_deref().unwrap_or(key);
        if let Observation::Queue(fact) = observation
            && fact.setup()
        {
            let identity = fact.identity();
            if fact.recorded(snapshot)? || snapshot.rows.iter().any(|row| {
                row.table == "inbox" && serde_json::from_value::<Observation>(row.value.clone())
                    .ok().is_some_and(|pending| matches!(pending, Observation::Queue(pending) if pending.identity() == identity))
            }) {
                return Ok(());
            }
            let id = inbox_key(
                &format!("{key}:{}", snapshot.revision.get()),
                index,
                observation,
            )?;
            commit.rows.push(put("inbox", id, json!(observation)));
            return Ok(());
        }
        let id = inbox_key(key, index, observation)?;
        let aliases = [
            id.clone(),
            format!("{key}:{index:08}"),
            format!("external:look:{key}:{index}:{}", evidence_key(observation)?),
        ];
        let value = json!(observation);
        if snapshot
            .rows
            .iter()
            .any(|row| row.table == "inbox" && aliases.contains(&row.id) && row.value == value)
            || self.store.event_matches(
                &aliases
                    .iter()
                    .map(|id| format!("consume:{id}"))
                    .collect::<Vec<_>>(),
                &value,
            )?
        {
            return Ok(());
        }
        commit.rows.push(put("inbox", id, value));
        Ok(())
    }

    /// Only news from an effect's own upstream releases its holds.
    pub fn retry_held_effects(&mut self, source: executor::Upstream) {
        if let Some(effects) = self.held_effects.get_mut(&source) {
            // A 404 must spend its retry allowance through a look first.
            effects.retain(|_, class| class == "not_found");
        }
    }

    fn is_held(&self, delivery: &Delivery) -> bool {
        self.held_effects
            .get(&executor::upstream(delivery.output))
            .is_some_and(|effects| effects.contains_key(&delivery.key))
    }

    fn hold_class(&self, delivery: &Delivery) -> Option<&str> {
        self.held_effects
            .get(&executor::upstream(delivery.output))
            .and_then(|effects| effects.get(&delivery.key))
            .map(String::as_str)
    }

    fn remove_hold(&mut self, delivery: &Delivery) {
        if let Some(effects) = self
            .held_effects
            .get_mut(&executor::upstream(delivery.output))
        {
            effects.remove(&delivery.key);
        }
    }

    pub fn drive_once(&mut self) -> Result<bool> {
        self.drive_once_with(|_, _| {})
    }

    /// The hook exposes durable boundaries for process-level fault injection.
    pub fn drive_once_with(&mut self, mut hook: impl FnMut(Boundary, &str)) -> Result<bool> {
        if self.store.migrate_dispatch_evidence()? {
            return Ok(true);
        }
        let snapshot = self.store.load()?;
        if self.retire_committed_waits(&snapshot)? {
            return Ok(true);
        }
        if self.reconcile_approvals(&snapshot)? {
            return Ok(true);
        }
        if self.reconcile_messages(&snapshot)? {
            return Ok(true);
        }
        if self.reconcile_memory(&snapshot)? || self.post_input_incident(&snapshot)? {
            return Ok(true);
        }
        if let Some(progressed) = self.pause_kernel_inputs(&snapshot)? {
            return Ok(progressed);
        }
        for row in snapshot
            .rows
            .iter()
            .filter(|r| r.table == "adoption_questions")
        {
            let question = decode_stored(row.value["question"].clone())?;
            hook(Boundary::BeforeDispatch, &row.id);
            let result = self
                .adapters
                .get(executor::Upstream::GitHub)?
                .intake_question(&question, snapshot.epoch.get());
            hook(Boundary::AfterDispatch, &row.id);
            match result {
                Ok(()) => {
                    let mut commit = Commit::new(
                        format!("{}:sent", row.id),
                        json!({"question_sent":row.id}),
                        &snapshot,
                    );
                    commit.rows.push(RowChange::Delete {
                        table: "adoption_questions".into(),
                        id: row.id.clone(),
                    });
                    self.store.commit(&commit)?;
                    return Ok(true);
                }
                Err(error) => eprintln!("adoption clarification pending for {}: {error}", row.id),
            }
        }
        let mut retired_approval = false;
        for row in snapshot
            .rows
            .iter()
            .filter(|r| r.table == "retiring_effects")
        {
            let delivery: Delivery = decode_stored(row.value.clone())?;
            let current = self.store.load()?;
            match self.retire_effect(&current, row) {
                Ok(()) if approval_delivery::is_approval(&delivery) => {
                    retired_approval = true;
                }
                Ok(()) => return Ok(true),
                Err(error) if !input_errors::recoverable(&error) => return Err(error),
                Err(error) => eprintln!("adapter retirement failed for {}: {error}", row.id),
            }
        }
        let snapshot = if retired_approval {
            self.store.load()?
        } else {
            snapshot
        };
        if let Some(row) = snapshot
            .rows
            .iter()
            .filter(|r| r.table == "inbox" || r.table == input_errors::PENDING_KERNEL)
            .min_by_key(|r| r.arrival)
        {
            if row.table == input_errors::PENDING_KERNEL {
                self.replay_kernel_input(&snapshot, row)?;
                return Ok(true);
            }
            let observation: Observation = decode_stored(row.value.clone())?;
            let mut waiting_for_queue = false;
            if let Observation::Queue(fact) = &observation
                && fact.needs_queue_ack()
                && let Some(node) = fact.node(&snapshot)?
            {
                for effect in &snapshot.pending_effects {
                    let delivery = hydrate(effect, &snapshot)?;
                    if delivery.node() == node
                        && matches!(
                            delivery.output,
                            DeliveryOutput::Effect {
                                kind: tl3_kernel::EffectKind::Enqueue,
                                ..
                            }
                        )
                        && matches!(effect.state, EffectState::Sent | EffectState::Uncertain)
                    {
                        // Queue facts can beat a lost enqueue response. Confirm that key first.
                        hook(Boundary::BeforeReconcile, &delivery.key);
                        let reconciled = self.adapters.adapter(&delivery)?.reconcile(&delivery);
                        hook(Boundary::AfterReconcile, &delivery.key);
                        if let Ok(Reconciliation::Found(completion)) = reconciled {
                            self.complete_queue_prerequisite(row, &effect.key, *completion)?;
                            return Ok(true);
                        }
                        waiting_for_queue = true;
                    }
                }
            }
            if !waiting_for_queue {
                if let Err(error) = self.consume_kernel_input(&snapshot, row) {
                    let mut commit =
                        Commit::new(format!("consume:{}", row.id), row.value.clone(), &snapshot);
                    let (target, title, node) =
                        input_errors::observation_context(&snapshot, &observation)?;
                    self.input_failed(
                        &snapshot,
                        &format!("observation:{}", crate::evidence_key(&observation)?),
                        target,
                        &title,
                        node,
                        observation_upstream(&snapshot, &observation)?
                            .unwrap_or(executor::Upstream::GitHub),
                        error,
                        &mut commit,
                    )?;
                    commit.rows.push(RowChange::Delete {
                        table: "inbox".into(),
                        id: row.id.clone(),
                    });
                    self.store.commit(&commit)?;
                }
                return Ok(true);
            }
        }
        if self.start_replacement(&snapshot)?
            || self.retire_comment(&snapshot)?
            || self.schedule_comment(&snapshot)?
            || self.start_postmortem(&snapshot)?
        {
            return Ok(true);
        }
        let mut progressed = false;
        let world = snapshot::decode(&snapshot)?;
        if snapshot
            .rows
            .iter()
            .any(|r| r.table == dispatch_liveness::QUEUED)
            && self.reconcile_queued_worker_waits(&snapshot, &world, &mut hook)?
        {
            return Ok(true);
        }
        for effect in &snapshot.pending_effects {
            let delivery = hydrate(effect, &snapshot)?;
            // ClosePr removes the queue label without waiting for Mergify admission reads.
            let closing_queue = executor::upstream(delivery.output) == executor::Upstream::Mergify
                && matches!(
                    world.nodes[delivery
                        .node()
                        .index()
                        .ok_or(Error::Invalid("node index overflow"))?]
                    .phase,
                    tl3_kernel::Phase::Stopping(_) | tl3_kernel::Phase::Closed(_)
                );
            let cancelling = effect.epoch < snapshot.epoch
                || match delivery.output {
                    DeliveryOutput::Effect { node, seq, .. } => {
                        let node = &world.nodes
                            [node.index().ok_or(Error::Invalid("node index overflow"))?];
                        node.effect_cancelling(seq)
                            || matches!(node.phase, tl3_kernel::Phase::Closed(_))
                    }
                    DeliveryOutput::Notice { .. } | DeliveryOutput::Comment { .. } => false,
                };
            if cancelling {
                match effect.state {
                    EffectState::Intended => {
                        if executor::upstream(delivery.output) == executor::Upstream::Mergify
                            && !closing_queue
                            && !matches!(
                                self.adapters
                                    .adapter(&delivery)?
                                    .reconcile_cancellation(&delivery),
                                Ok(Reconciliation::Found(_))
                            )
                        {
                            continue;
                        }
                        self.cancel_absent(effect, &delivery)?;
                        return Ok(true);
                    }
                    EffectState::Sent | EffectState::Uncertain => {
                        self.transition(effect, EffectState::Cancelling)?;
                        return Ok(true);
                    }
                    _ => {}
                }
            }
            if looks::outage_waiting(&snapshot, &delivery.key) && !cancelling {
                continue;
            }
            match effect.state {
                EffectState::Intended => {
                    let (prior_dispatch, predecessor) = if approval_delivery::is_approval(&delivery)
                        && !snapshot.rows.iter().any(|r| {
                            r.table == approval_delivery::AUTHORIZATIONS && r.id == effect.key
                        }) {
                        self.approval_prior_dispatch(effect, &snapshot)?
                    } else {
                        (false, false)
                    };
                    if prior_dispatch {
                        match self.adapters.adapter(&delivery)?.reconcile(&delivery) {
                            Ok(Reconciliation::Absent) if !predecessor => {}
                            Ok(Reconciliation::Found(completion)) if !predecessor => {
                                self.transition(effect, EffectState::Sent)?;
                                self.complete(&effect.key, *completion)?;
                                return Ok(true);
                            }
                            _ => {
                                self.transition(effect, EffectState::Sent)?;
                                self.approval_ambiguous(&delivery)?;
                                return Ok(true);
                            }
                        }
                    }
                    self.transition(effect, EffectState::Sent)?;
                    hook(Boundary::BeforeDispatch, &effect.key);
                    let current = self.store.load()?;
                    let delivery = hydrate(effect, &current)?;
                    if self.observe_pending_effect(&delivery)? {
                        return Ok(true);
                    }
                    let result = self.adapters.adapter(&delivery)?.execute(&delivery);
                    hook(Boundary::AfterDispatch, &effect.key);
                    match result {
                        Ok(completion) => {
                            self.complete(&effect.key, completion)?;
                            return Ok(true);
                        }
                        Err(Error::ApprovalAmbiguous) => {
                            self.approval_ambiguous(&delivery)?;
                            return Ok(true);
                        }
                        Err(Error::WorkerInputs(ref failure)) => {
                            let snapshot = self.store.load()?;
                            let mut commit = Commit::new(
                                format!(
                                    "worker-input-failure:{}:{}",
                                    effect.key,
                                    snapshot.revision.get()
                                ),
                                json!({"worker_input_failure":effect.key}),
                                &snapshot,
                            );
                            let context =
                                worker_inputs::Context::capture(failure, &snapshot, &delivery);
                            commit.rows.push(put(
                                "worker_input_failures",
                                effect.key.clone(),
                                json!(context),
                            ));
                            commit.rows.push(put(
                                "worker_input_contexts",
                                delivery.node().0.to_string(),
                                json!(context),
                            ));
                            self.store.commit(&commit)?;
                            self.complete(
                                &effect.key,
                                Completion::permanent(worker_inputs::CLASS),
                            )?;
                            return Ok(true);
                        }
                        // GitHub can briefly return 404 for a newly created resource.
                        // A look proving absence spends the existing retry allowance.
                        Err(ref error @ Error::Permanent { class, .. })
                            if !(error.is_github_not_found()
                                && matches!(delivery.output, DeliveryOutput::Effect { node, .. }
                                    if world.nodes[node.index().ok_or(Error::Invalid("node index overflow"))?].fx_failures == 0)) =>
                        {
                            let mut completion = Completion::permanent(class);
                            if publication_failure::handles(&delivery) {
                                let diagnostic = publication_failure::Diagnostic::capture(
                                    &delivery,
                                    Some(error),
                                    class,
                                )?;
                                completion.diagnostic = Some(diagnostic);
                            }
                            self.complete(&effect.key, completion)?;
                            return Ok(true);
                        }
                        Err(error) => {
                            // A transport error is not an upstream acknowledgement.
                            self.record_publication_refusal(&delivery, &error)?;
                            eprintln!("adapter execute failed for {}: {error}", effect.key);
                            if let Error::Quota { retry_after } = &error {
                                self.defer_quota(&effect.key, *retry_after)?;
                            }
                            self.held_effects
                                .entry(executor::upstream(delivery.output))
                                .or_default()
                                .insert(effect.key.clone(), looks::failure_class(&error).into());
                            progressed = true;
                            continue;
                        }
                    }
                }
                EffectState::Sent => {
                    self.transition(effect, EffectState::Uncertain)?;
                    return Ok(true);
                }
                EffectState::Uncertain | EffectState::Cancelling => {
                    if !cancelling && self.observe_pending_effect(&delivery)? {
                        return Ok(true);
                    }
                    let local_cancellation = effect.state == EffectState::Cancelling
                        && executor::upstream(delivery.output) == executor::Upstream::Mergify;
                    if !local_cancellation
                        && looks::quota_waiting(&snapshot, &effect.key, self.now_ms())?
                    {
                        continue;
                    }
                    hook(Boundary::BeforeReconcile, &effect.key);
                    let reconciled = if effect.state == EffectState::Cancelling {
                        if closing_queue {
                            Ok(Reconciliation::Found(Box::new(Completion::confirmed(
                                vec![],
                            ))))
                        } else {
                            self.adapters
                                .adapter(&delivery)?
                                .reconcile_cancellation(&delivery)
                        }
                    } else {
                        self.adapters.adapter(&delivery)?.reconcile(&delivery)
                    };
                    hook(Boundary::AfterReconcile, &effect.key);
                    let result = match reconciled {
                        Ok(result) => result,
                        Err(error) => {
                            eprintln!("adapter reconciliation failed for {}: {error}", effect.key);
                            self.record_publication_refusal(&delivery, &error)?;
                            continue;
                        }
                    };
                    match result {
                        Reconciliation::Found(completion) => {
                            let completion = *completion;
                            if completion.releases_holds() {
                                self.retry_held_effects(executor::upstream(delivery.output));
                            }
                            self.complete(&effect.key, completion)?;
                            return Ok(true);
                        }
                        Reconciliation::Absent => {
                            if effect.state == EffectState::Cancelling {
                                self.cancel_absent(effect, &delivery)?;
                                return Ok(true);
                            }
                            if !self.is_held(&delivery) {
                                self.transition(effect, EffectState::Intended)?;
                                progressed = true;
                            }
                        }
                        Reconciliation::Pending => {
                            if approval_delivery::is_approval(&delivery)
                                && !cancelling
                                && !self.is_held(&delivery)
                            {
                                progressed |= self.approval_ambiguous(&delivery)?;
                            }
                        }
                    }
                }
                EffectState::Confirmed | EffectState::Failed | EffectState::Cancelled => {
                    unreachable!()
                }
            }
        }
        // An acknowledged start can outlive its dispatch. Its authoritative
        // record must also be read after a restart or a missed notification.
        let snapshot = self.store.load()?;
        let watch_keys: Vec<_> = snapshot
            .rows
            .iter()
            .filter(|row| row.table == "watches")
            .map(|row| row.id.clone())
            .collect();
        for key in watch_keys {
            let snapshot = self.store.load()?;
            let Some(row) = snapshot
                .rows
                .iter()
                .find(|row| row.table == "watches" && row.id == key)
            else {
                continue;
            };
            let mut delivery: Delivery = decode_stored(row.value.clone())?;
            let world = snapshot::decode(&snapshot)?;
            let node = &world.nodes[delivery
                .node()
                .index()
                .ok_or(Error::Invalid("node index overflow"))?];
            let effect = self
                .store
                .effect(&delivery.key)?
                .ok_or(Error::Invalid("missing watched effect"))?;
            if effect.state.is_pending()
                || looks::quota_waiting(&snapshot, &delivery.key, self.now_ms())?
            {
                continue;
            }
            let active = effect.state == EffectState::Confirmed
                && if let Some(jobs) = delivery.job_table() {
                    snapshot
                        .rows
                        .iter()
                        .any(|r| r.table == jobs && r.id == delivery.key)
                } else {
                    match delivery.output {
                        DeliveryOutput::Effect { kind, .. } => {
                            node.operation_pending(kind, delivery.generation)
                        }
                        DeliveryOutput::Notice { .. } | DeliveryOutput::Comment { .. } => false,
                    }
                };
            let mut commit = Commit::new(
                format!("look:{}:{}", row.id, snapshot.revision.get()),
                json!({"look": row.id}),
                &snapshot,
            );
            if active && looks::outage_waiting(&snapshot, &delivery.key) {
                continue;
            }
            let mut quota = false;
            if active {
                delivery = hydrate(&effect, &snapshot)?;
                hook(Boundary::BeforeReconcile, &delivery.key);
                let reconciled = self.adapters.adapter(&delivery)?.reconcile(&delivery);
                hook(Boundary::AfterReconcile, &delivery.key);
                let completion = match reconciled {
                    Ok(Reconciliation::Found(completion)) => *completion,
                    Ok(Reconciliation::Absent)
                        if executor::upstream(delivery.output) == executor::Upstream::Workers =>
                    {
                        adapters::workers::failed(&delivery)
                    }
                    Ok(Reconciliation::Absent | Reconciliation::Pending) => continue,
                    Err(error) => {
                        eprintln!(
                            "adapter reconciliation failed for {}: {error}",
                            delivery.key
                        );
                        continue;
                    }
                };
                if completion.ack != Ack::Confirmed {
                    continue;
                }
                if completion.observations.is_empty() {
                    let changed = self.record_progress(
                        &snapshot,
                        &mut commit,
                        &row.id,
                        completion.progress.as_ref(),
                    )?;
                    if !commit.rows.is_empty() {
                        self.store.commit(&commit)?;
                    }
                    if changed == Some(true) {
                        self.retry_held_effects(executor::upstream(delivery.output));
                    }
                    continue;
                }
                if completion.releases_holds() {
                    self.retry_held_effects(executor::upstream(delivery.output));
                }
                quota = completion
                    .observations
                    .iter()
                    .any(|observation| matches!(observation, Observation::Quota { .. }));
                for (index, observation) in completion.observations.iter().enumerate() {
                    self.queue_observation(
                        &snapshot,
                        &mut commit,
                        &delivery.key,
                        index,
                        observation,
                    )?;
                }
            }
            if !quota {
                commit.rows.push(RowChange::Delete {
                    table: "watches".into(),
                    id: row.id.clone(),
                });
                queue_retirement(&mut commit, &delivery);
            }
            self.store.commit(&commit)?;
            return Ok(true);
        }
        self.adapters.retire(&mut self.store);
        let snapshot = self.store.load()?;
        Ok(self.prune_allowances(&snapshot)? || progressed)
    }

    fn retire_effect(&mut self, snapshot: &Snapshot, row: &tl3_store::WorldRow) -> Result<()> {
        let delivery: Delivery = decode_stored(row.value.clone())?;
        if executor::upstream(delivery.output) == executor::Upstream::Mergify
            || approval_delivery::is_approval(&delivery)
        {
            self.adapters
                .adapter(&delivery)?
                .retire_delivery(&delivery)?;
        }
        let mut commit = Commit::new(
            format!("retire:{}:{}", row.id, snapshot.revision.get()),
            json!({"retired": row.id}),
            snapshot,
        );
        commit.rows.push(RowChange::Delete {
            table: row.table.clone(),
            id: row.id.clone(),
        });
        self.store.commit(&commit)?;
        Ok(())
    }

    fn observe_pending_effect(&mut self, delivery: &Delivery) -> Result<bool> {
        let before = self.store.load()?.revision;
        // Dispatch and reconciliation retain their existing error/allowance handling.
        if let Ok(observations) = self.adapters.adapter(delivery)?.observations(delivery) {
            for (index, observation) in observations.into_iter().enumerate() {
                let key = format!(
                    "pending:{}:{index}:{}",
                    delivery.key,
                    evidence_key(&observation)?
                );
                self.observe(&key, observation)?;
            }
        }
        Ok(self.store.load()?.revision != before)
    }

    fn transition(&mut self, effect: &Effect, to: EffectState) -> Result<()> {
        let snapshot = self.store.load()?;
        let delivery = hydrate(effect, &snapshot)?;
        let mut commit = Commit::new(
            format!("delivery:{}:{}", effect.key, snapshot.revision.get()),
            json!({"effect": effect.key, "from": effect.state, "to": to}),
            &snapshot,
        );
        commit.transitions.push(EffectTransition {
            key: effect.key.clone(),
            from: effect.state,
            to,
        });
        if to == EffectState::Sent && approval_delivery::is_approval(&delivery) {
            commit.rows.push(RowChange::Delete {
                table: approval_delivery::AUTHORIZATIONS.into(),
                id: effect.key.clone(),
            });
        }
        // A pending-checks hold waits on check webhooks; it arms no silence timer.
        if to == EffectState::Uncertain
            && let Some(class) = self
                .held_effects
                .get(&executor::upstream(delivery.output))
                .and_then(|effects| effects.get(&effect.key))
            && class != looks::CHECKS_PENDING
        {
            self.failure_allowance(&snapshot, &effect.key, class, &mut commit)?;
        }
        self.store.commit(&commit)?;
        Ok(())
    }

    fn cancel_absent(&mut self, effect: &Effect, _delivery: &Delivery) -> Result<()> {
        self.submit_kernel_input(
            format!("cancel:{}:{}", effect.key, effect.updated_revision.get()),
            input_errors::PendingKernelInput::CancelAbsent {
                key: effect.key.clone(),
            },
        )
        .map(|_| ())
    }

    fn prepare_cancel_absent(
        &mut self,
        snapshot: &Snapshot,
        effect: &Effect,
        delivery: &Delivery,
    ) -> Result<Commit> {
        let mut commit = Commit::new(
            format!("cancel:{}:{}", effect.key, snapshot.revision.get()),
            json!({"cancelled_absent": effect.key}),
            snapshot,
        );
        commit.transitions.push(EffectTransition {
            key: effect.key.clone(),
            from: effect.state,
            to: EffectState::Cancelled,
        });
        if let DeliveryOutput::Effect { node, seq, .. } = delivery.output {
            self.apply(
                snapshot,
                snapshot::decode(snapshot)?,
                Event::Ack {
                    node,
                    seq,
                    ack: Ack::Retryable,
                },
                &mut commit,
            )?;
        }
        if approval_delivery::is_approval(delivery) {
            queue_retirement(&mut commit, delivery);
        }
        looks::resolve_effect_incidents(snapshot, &mut commit, |_, key| Ok(key == delivery.key))?;
        Ok(commit)
    }

    fn complete(&mut self, key: &str, completion: Completion) -> Result<()> {
        let snapshot = self.store.load()?;
        let effect = snapshot
            .pending_effects
            .iter()
            .find(|e| e.key == key)
            .ok_or(Error::Invalid("completion has no pending effect"))?;
        self.submit_kernel_input(
            format!("completion:{key}:{}", effect.updated_revision.get()),
            input_errors::PendingKernelInput::Completion {
                key: key.into(),
                completion,
            },
        )
        .map(|_| ())
    }

    fn prepare_completion(
        &mut self,
        snapshot: &Snapshot,
        key: &str,
        mut completion: Completion,
    ) -> Result<Commit> {
        let effect = snapshot
            .pending_effects
            .iter()
            .find(|e| e.key == key)
            .ok_or(Error::Invalid("completion has no pending effect"))?;
        let delivery: Delivery = decode_stored(effect.payload.clone())?;
        if completion.ack == Ack::Permanent
            && publication_failure::handles(&delivery)
            && completion.diagnostic.is_none()
        {
            completion.diagnostic = Some(publication_failure::Diagnostic::capture(
                &delivery,
                None,
                completion.failure_class.as_deref().unwrap_or("unknown"),
            )?);
        }
        let mut commit = Commit::new(
            format!("completion:{key}:{}", effect.updated_revision.get()),
            json!(&completion),
            snapshot,
        );
        let stopped_node = match delivery.output {
            executor::Output::Effect {
                node,
                kind: tl3_kernel::EffectKind::StopRun,
                ..
            } if completion.ack == Ack::Confirmed => Some(node),
            _ => None,
        };
        self.release_terminal_model_probes(snapshot, &mut commit, stopped_node)?;
        self.retire_completed_model_attempts(snapshot, &mut commit)?;
        if let Some(diagnostic) = &completion.diagnostic {
            eprintln!("{}", diagnostic.render());
            commit
                .rows
                .push(put("publication_failures", key.into(), json!(diagnostic)));
        }
        // A confirmed completion may still name its failure, such as a check
        // action whose trusted harness rejected TL3's own input.
        if let Some(class) = completion.failure_class.as_deref() {
            self.failure_allowance(snapshot, key, class, &mut commit)?;
        }
        if effect.state == EffectState::Uncertain && completion.ack != Ack::Confirmed {
            // A terminal failed attempt was found. Return to the sending state
            // through the effect machine before recording its failure.
            commit.transitions.push(EffectTransition {
                key: key.into(),
                from: EffectState::Uncertain,
                to: EffectState::Intended,
            });
            commit.transitions.push(EffectTransition {
                key: key.into(),
                from: EffectState::Intended,
                to: EffectState::Sent,
            });
        }
        let from = if effect.state == EffectState::Uncertain && completion.ack != Ack::Confirmed {
            EffectState::Sent
        } else {
            effect.state
        };
        let state = if effect.state == EffectState::Cancelling {
            EffectState::Confirmed
        } else {
            match completion.ack {
                Ack::Confirmed => EffectState::Confirmed,
                Ack::Retryable => EffectState::Intended,
                Ack::Permanent | Ack::BodyPolicyRefused => EffectState::Failed,
            }
        };
        commit.transitions.push(EffectTransition {
            key: key.into(),
            from,
            to: state,
        });
        // Preserve teardown evidence in the same transaction that can emit the summary.
        for observation in &completion.observations {
            match observation {
                Observation::ScratchSaved { node, reference } => {
                    let value = json!({"node":node,"reference":reference});
                    commit
                        .rows
                        .push(put("saved_scratch", reference.clone(), value));
                    commit.rows.push(put(
                        "scratch_evidence",
                        reference.clone(),
                        json!({"node":node,"reference":reference,"status":"saved"}),
                    ));
                }
                Observation::ScratchPruned { reference } => {
                    if let Some(saved) = snapshot
                        .rows
                        .iter()
                        .find(|r| r.table == "saved_scratch" && r.id == *reference)
                    {
                        let mut value = saved.value.clone();
                        value["status"] = json!("pruned");
                        commit
                            .rows
                            .push(put("scratch_evidence", reference.clone(), value));
                    }
                    commit.rows.push(RowChange::Delete {
                        table: "saved_scratch".into(),
                        id: reference.clone(),
                    });
                }
                _ => {}
            }
        }
        if let DeliveryOutput::Effect {
            node, seq, kind, ..
        } = delivery.output
        {
            let ack = if kind == tl3_kernel::EffectKind::OpenPr
                && completion.ack == Ack::Permanent
                && completion
                    .diagnostic
                    .as_ref()
                    .is_some_and(publication_failure::Diagnostic::is_body_refusal)
            {
                Ack::BodyPolicyRefused
            } else {
                completion.ack
            };
            self.apply(
                snapshot,
                snapshot::decode(snapshot)?,
                Event::Ack { node, seq, ack },
                &mut commit,
            )?;
        }
        if completion.ack == Ack::Permanent && delivery.is_postmortem() {
            // #403: a postmortem never posts or raises an incident; it fails.
            self.postmortem_failed(snapshot, &delivery, "launch_failed", &mut commit)?;
        } else if completion.ack == Ack::Permanent
            && !matches!(delivery.output, DeliveryOutput::Effect { .. })
        {
            self.failed_notice(
                snapshot,
                &delivery,
                completion
                    .failure_class
                    .as_deref()
                    .unwrap_or("permanent_failure"),
                &mut commit,
            )?;
        }
        if completion.ack == Ack::Retryable
            && state == EffectState::Intended
            && matches!(delivery.output, DeliveryOutput::Effect { .. })
        {
            // The kernel assigns a fresh sequence to its retry. Retire the old
            // delivery and its watch together, so absence of the cancelled key
            // cannot fail the replacement operation in the same generation.
            commit.rows.push(RowChange::Delete {
                table: "watches".into(),
                id: key.into(),
            });
            if approval_delivery::is_approval(&delivery) {
                for replacement in &commit.effects {
                    let next: Delivery = decode_stored(replacement.payload.clone())?;
                    if approval_delivery::is_approval(&next) {
                        commit.rows.push(put(
                            approval_delivery::AUTHORIZATIONS,
                            next.key.clone(),
                            json!(true),
                        ));
                        commit.rows.push(put(
                            approval_delivery::LINEAGE_SAFE,
                            next.key,
                            json!(true),
                        ));
                    }
                }
            }
            queue_retirement(&mut commit, &delivery);
            commit.transitions.push(EffectTransition {
                key: key.into(),
                from: EffectState::Intended,
                to: EffectState::Cancelled,
            });
        }
        if completion.ack == Ack::Confirmed {
            if matches!(
                delivery.output,
                DeliveryOutput::Effect {
                    kind: tl3_kernel::EffectKind::StartRun(_),
                    ..
                }
            ) {
                commit.rows.push(RowChange::Delete {
                    table: "worker_input_contexts".into(),
                    id: delivery.node().0.to_string(),
                });
            }
            looks::resolve_effect_incidents(snapshot, &mut commit, |_, effect| Ok(effect == key))?;
            if let Some(marker) = &delivery.marker_key {
                commit.rows.push(RowChange::Delete {
                    table: "notice_incidents".into(),
                    id: marker.clone(),
                });
            }
            self.record_progress(snapshot, &mut commit, key, completion.progress.as_ref())?;
        }
        // Upstream facts remain evidence even when the attempted write was refused.
        for (index, observation) in completion.observations.iter().enumerate() {
            if matches!(
                observation,
                Observation::ScratchSaved { .. } | Observation::ScratchPruned { .. }
            ) {
                continue;
            }
            self.queue_observation(snapshot, &mut commit, key, index, observation)?;
        }
        if approval_delivery::is_approval(&delivery)
            && matches!(
                state,
                EffectState::Confirmed | EffectState::Failed | EffectState::Cancelled
            )
        {
            queue_retirement(&mut commit, &delivery);
        }
        evidence::retire_delivered(snapshot, &mut commit, &delivery, completion.ack);
        Ok(commit)
    }

    fn apply(
        &mut self,
        snapshot: &Snapshot,
        mut world: World,
        event: Event,
        commit: &mut Commit,
    ) -> Result<()> {
        if !self.kernel_headroom(snapshot, &world, commit)? {
            return Ok(());
        }
        // A dequeue receipt can precede queue admission, where the kernel
        // intentionally ignores it. Only an accepted Resume releases that hold.
        if let Event::Command {
            node,
            id,
            command: tl3_kernel::Command::Resume,
        } = event
            && let Some(current) = world.nodes.get(node.0 as usize)
            && id > current.last_cmd
            && !current.phase.is_closed()
        {
            commit.rows.push(put(
                "queue_resumes",
                node.0.to_string(),
                json!({"command": id}),
            ));
        }
        let closing_cause = match event {
            Event::Command {
                node,
                command:
                    tl3_kernel::Command::Stop
                    | tl3_kernel::Command::Answer {
                        answer: tl3_kernel::Answer::Stop,
                        ..
                    },
                ..
            } => Some((node, "user_stop")),
            Event::Command {
                node,
                command:
                    tl3_kernel::Command::ChangeRequest
                    | tl3_kernel::Command::Answer {
                        answer: tl3_kernel::Answer::ChangeRequest,
                        ..
                    },
                ..
            } => Some((node, "replaced")),
            Event::Fact {
                node,
                fact: tl3_kernel::Fact::PrClosedByPerson,
            } => Some((node, "pr_closed")),
            Event::Fact {
                node,
                fact: tl3_kernel::Fact::PrDequeuedByPerson,
            } => Some((node, "pr_dequeued")),
            _ => None,
        }
        .filter(|(node, _)| {
            world.nodes.get(node.0 as usize).is_some_and(|n| {
                !matches!(
                    n.phase,
                    tl3_kernel::Phase::Stopping(_) | tl3_kernel::Phase::Closed(_)
                )
            })
        });
        let mut dependency_outputs = Vec::new();
        self.declare_merge_dependencies(
            snapshot,
            &mut world,
            event,
            &mut dependency_outputs,
            commit,
        )?;
        if !self.kernel_headroom(snapshot, &world, commit)? {
            return Ok(());
        }
        let dependency_roots = merge_dependencies::changed_roots(&world, event);
        if !self.input_checkpoint(snapshot, commit)? {
            return Ok(());
        }
        let (mut world, mut outputs) = tl3_kernel::step(world, event);
        outputs.splice(0..0, dependency_outputs);
        self.announce_dependency_endings(
            snapshot,
            &mut world,
            dependency_roots,
            &mut outputs,
            commit,
        )?;
        if input_errors::deferred(commit) {
            return Ok(());
        }
        if let Some((node, reason)) = closing_cause
            && world.nodes.get(node.0 as usize).is_some_and(|n| {
                matches!(
                    n.phase,
                    tl3_kernel::Phase::Stopping(_) | tl3_kernel::Phase::Closed(_)
                )
            })
        {
            commit.rows.push(put(
                "close_reasons",
                node.0.to_string(),
                json!({"reason":reason}),
            ));
        }
        self.record_kernel_result(snapshot, world, outputs, commit)?;
        if !commit.result.is_object() {
            commit.result = json!({});
        }
        commit.result["kernel_event"] = json!(event);
        Ok(())
    }

    fn record_kernel_result(
        &mut self,
        snapshot: &Snapshot,
        world: World,
        outputs: Vec<Out>,
        commit: &mut Commit,
    ) -> Result<()> {
        let previous = snapshot::decode(snapshot)?;
        let before = snapshot::encode(&previous)?;
        for (index, node) in world.nodes.iter().enumerate() {
            if !matches!(node.phase, tl3_kernel::Phase::Planning { .. })
                || previous
                    .nodes
                    .get(index)
                    .is_some_and(|node| matches!(node.phase, tl3_kernel::Phase::Planning { .. }))
            {
                continue;
            }
            let id = index.to_string();
            let maker_findings = commit
                .rows
                .iter()
                .rev()
                .find_map(|row| match row {
                    RowChange::Put {
                        table,
                        id: row_id,
                        value,
                    } if table == "maker_findings" && row_id == &id => Some(value),
                    _ => None,
                })
                .or_else(|| {
                    snapshot
                        .rows
                        .iter()
                        .find(|row| row.table == "maker_findings" && row.id == id)
                        .map(|row| &row.value)
                });
            if let Some(maker_findings) = maker_findings {
                let mut findings = self
                    .store
                    .worker_document(&id, "findings")?
                    .unwrap_or(json!([]));
                let reports = findings
                    .as_array_mut()
                    .ok_or(Error::Invalid("findings must be text reports"))?;
                for finding in maker_findings
                    .as_array()
                    .ok_or(Error::Invalid("invalid maker findings"))?
                {
                    if !reports.contains(&finding["report"]) {
                        reports.push(finding["report"].clone());
                    }
                }
                commit.rows.push(put("findings", id, findings));
            }
        }
        commit
            .rows
            .extend(snapshot::changes(&before, snapshot::encode(&world)?));
        self.close_replacement(snapshot, &previous, &world, commit)?;
        self.record_review_stops(snapshot, &previous, &world, commit)?;
        self.resolve_closed_effect_incidents(snapshot, &world, commit)?;
        self.record_postmortems(snapshot, &previous, &world, commit)?;
        self.resolve_stale_missing_checks(snapshot, &world, commit)?;
        for (index, node) in world.nodes.iter().enumerate() {
            if previous
                .nodes
                .get(index)
                .is_some_and(|old| old.waiting_incident)
                && !node.waiting_incident
            {
                for row in snapshot.rows.iter().filter(|r| {
                    r.table == "incidents"
                        && r.value["node"] == json!(NodeId(index as u64))
                        && r.value["open"] == true
                        && r.value["blocking"] == true
                }) {
                    let mut value = row.value.clone();
                    value["open"] = json!(false);
                    commit.rows.push(put("incidents", row.id.clone(), value));
                }
            }
        }
        let dictionary = self.store.load()?.interned;
        for (index, output) in outputs.into_iter().enumerate() {
            if let Out::Notice {
                node,
                notice: tl3_kernel::Notice::FileReplacement,
            } = output
            {
                self.pending_replacement(snapshot, &world, commit, node)?;
                continue;
            }
            let (node, key) = match output {
                Out::Effect { node, seq, .. } => (node, format!("effect:{}:{seq}", node.0)),
                Out::Notice { node, .. } => (
                    node,
                    format!("notice:{}:{index}", snapshot.revision.get() + 1),
                ),
            };
            let n = &world.nodes[node.index().ok_or(Error::Invalid("node index overflow"))?];
            let head_sha = n
                .head
                .map(|head| {
                    dictionary
                        .iter()
                        .find(|i| i.kind == tl3_store::InternKind::Commit && i.id == head.0 as i64)
                        .map(|i| i.value.clone())
                        .ok_or(Error::Invalid("head missing from intern dictionary"))
                })
                .transpose()?;
            let binding = commit.rows.iter().find_map(|row| match row {
                RowChange::Put { table, id, value }
                    if table == "requests" && id == &node.0.to_string() =>
                {
                    Some(value.clone())
                }
                _ => None,
            });
            let request: Request = match binding {
                Some(value) => decode_stored(value)?,
                None => request(snapshot, node)?,
            };
            let pull_request = snapshot
                .rows
                .iter()
                .find(|r| r.table == "pull_requests" && r.id == node.0.to_string())
                .and_then(|r| r.value["number"].as_u64());
            let paths = world
                .files(n.footprint)
                .ok_or(Error::Invalid("unknown footprint"))?
                .map(|file| {
                    dictionary
                        .iter()
                        .find(|i| i.kind == tl3_store::InternKind::Path && i.id == file.0 as i64)
                        .map(|i| i.value.clone())
                        .ok_or(Error::Invalid("path missing from intern dictionary"))
                })
                .collect::<Result<Vec<_>>>()?;
            let mut checks = check_run(snapshot, node, head_sha.as_deref())?;
            if checks.is_none()
                && matches!(
                    output,
                    Out::Effect {
                        kind: tl3_kernel::EffectKind::SubmitChecks,
                        ..
                    }
                )
                && let Some(sha) = &head_sha
                && let Some(materials) = self.store.worker_materials(&node.0.to_string())?
                && let Some(run) = materials.get("check_run")
            {
                let run: adapters::prismabuild::CheckRun = decode_stored(run.clone())?;
                commit
                    .rows
                    .push(put("check_plans", format!("{}:{sha}", node.0), json!(&run)));
                checks = Some(run);
            }
            let question_context = if let Out::Notice {
                notice: tl3_kernel::Notice::Ask { question },
                ..
            } = output
            {
                let plan = commit
                    .rows
                    .iter()
                    .rev()
                    .find_map(|row| match row {
                        RowChange::Put { table, id, value }
                            if table == "plans" && id == &node.0.to_string() =>
                        {
                            Some(value.clone())
                        }
                        _ => None,
                    })
                    .or_else(|| {
                        snapshot
                            .rows
                            .iter()
                            .find(|r| r.table == "plans" && r.id == node.0.to_string())
                            .map(|r| r.value.clone())
                    });
                let mut context = json!({"node": node, "question": question, "kind": n.question.map(|q| q.kind), "generation": n.generation, "pieces": plan.as_ref().and_then(|p| p.get("pieces")).cloned().unwrap_or(json!([])), "plan": plan});
                // #403: every question to Rob owes a postmortem.
                commit.postmortems.push(postmortems::owed(
                    snapshot,
                    node,
                    format!("question:{question}"),
                    postmortems::Reason::Escalation,
                    None,
                )?);
                // #322: Rob sees the deciding worker's findings, not only the question kind.
                let written = |table: &str| {
                    commit.rows.iter().rev().find_map(|row| match row {
                        RowChange::Put {
                            table: t,
                            id,
                            value,
                        } if t == table && id == &node.0.to_string() => Some(value.clone()),
                        _ => None,
                    })
                };
                let before = |table: &str| {
                    snapshot
                        .rows
                        .iter()
                        .find(|r| r.table == table && r.id == node.0.to_string())
                        .map(|r| r.value.clone())
                };
                let incidents: Vec<_> = commit
                    .rows
                    .iter()
                    .filter_map(|row| match row {
                        RowChange::Put { table, value, .. }
                            if table == "incidents" && value["node"] == json!(node) =>
                        {
                            Some(value.clone())
                        }
                        _ => None,
                    })
                    .collect();
                let prior = snapshot::decode(snapshot)?
                    .nodes
                    .get(node.0 as usize)
                    .map_or(tl3_kernel::Phase::Ingesting, |previous| previous.phase);
                if let Some(binding) = n.question.as_ref().filter(|binding| binding.id == question)
                    && let Some(findings) = question_findings::findings(
                        binding,
                        &question_findings::Sources {
                            fresh: &written,
                            previous: &before,
                            incidents: &incidents,
                            prior,
                        },
                    )
                {
                    context["findings"] = findings;
                }
                if let Some(failure) = worker_inputs::current(snapshot, commit, node)? {
                    context["worker_input_failure"] = json!(failure);
                }
                commit.rows.push(put(
                    "question_context",
                    question.to_string(),
                    context.clone(),
                ));
                Some(context)
            } else {
                None
            };
            let mut delivery = Delivery {
                key: key.clone(),
                epoch: snapshot.epoch.get(),
                output: output.into(),
                generation: n.generation,
                head_sha,
                repository: request.repo,
                issue: request.issue,
                pull_request,
                proposed: request.proposed,
                paths,
                attempt: n.attempt,
                round: n.round,
                replanned: n.replanned,
                checks,
                job_generation: n.job,
                question: n.question,
                marker_key: None,
                note: match output {
                    Out::Notice { notice, .. } => merge_dependencies::dependency_note(notice),
                    _ => None,
                },
                scope_removals: None,
                question_context,
                steering: None,
                request_text: request.source,
                evidence: None,
            };
            if matches!(
                output,
                Out::Notice {
                    notice: tl3_kernel::Notice::Summary(_),
                    ..
                } | Out::Effect {
                    kind: tl3_kernel::EffectKind::OpenPr | tl3_kernel::EffectKind::Enqueue,
                    ..
                }
            ) {
                delivery.evidence = Some(evidence::capture(snapshot, commit, &delivery)?);
            }
            if matches!(
                output,
                Out::Notice {
                    notice: tl3_kernel::Notice::Summary(_),
                    ..
                }
            ) {
                delivery.scope_removals = scope::summary(snapshot, node)?;
                delivery.question_context = review_stops::context(snapshot, commit, node);
                let references = &delivery
                    .evidence
                    .as_ref()
                    .expect("captured summary evidence")
                    .scratch;
                if !references.is_empty() {
                    delivery.note = Some(format!(
                        "Saved unfinished work, kept for 30 days:\n{}",
                        references.join("\n")
                    ));
                }
            }
            if matches!(
                output,
                Out::Notice {
                    notice: tl3_kernel::Notice::Summary(tl3_kernel::Outcome::NotActionable),
                    ..
                }
            ) {
                protected_requests::closing_summary(self, snapshot, commit, &mut delivery)?;
            }
            if matches!(output, Out::Notice { .. })
                && let Some(failure) = worker_inputs::current(snapshot, commit, node)?
            {
                delivery.question_context.get_or_insert_with(|| json!({}))["worker_input_failure"] =
                    json!(failure);
            }
            if let Out::Notice {
                notice: tl3_kernel::Notice::Incident { needs_rob },
                ..
            } = output
            {
                let id = looks::incident_id(snapshot, index);
                let looks::AttachedIncident {
                    upstream: mut source,
                    mut class,
                    diagnostic,
                    input_failure,
                } = self.attach_incident(snapshot, node, &id, commit)?;
                let blocked = blocked::evidence(commit, node)?;
                if blocked.is_some() {
                    source = executor::Upstream::Workers;
                    class = blocked::CLASS.into();
                }
                let signature = looks::IncidentSignature {
                    upstream: source,
                    class: class.clone(),
                };
                let needs_rob =
                    looks::incident_needs_rob(looks::IncidentCause::Kernel { needs_rob });
                let app_login = self
                    .adapters
                    .get(source)?
                    .app_login(delivery.repository.installation);
                let mut summary = looks::incident_summary(
                    &request.title,
                    &signature,
                    needs_rob,
                    app_login.as_deref(),
                );
                worker_inputs::append(&mut summary, input_failure.as_ref());
                let occurrences = 1 + snapshot
                    .rows
                    .iter()
                    .filter(|row| {
                        row.table == "incidents"
                            && row.value["node"] == json!(node)
                            && row.value["signature"] == json!(signature)
                    })
                    .count();
                if let Some(diagnostic) = &diagnostic {
                    summary.push_str(&format!(
                        " Recorded occurrence {occurrences}. {}",
                        diagnostic.render()
                    ));
                }
                if let Some(detail) = looks::queue_command_detail(&delivery, &class) {
                    summary.push(' ');
                    summary.push_str(&detail);
                }
                if class == "queue_unfinished"
                    && let Some(detail) = looks::queue_incident_detail(snapshot, node)?
                {
                    summary.push(' ');
                    summary.push_str(&detail);
                }
                if let Some(evidence) = blocked {
                    summary.push(' ');
                    summary.push_str(&evidence);
                }
                if source == executor::Upstream::Workers {
                    let failure = commit
                        .rows
                        .iter()
                        .rev()
                        .find_map(|row| match row {
                            RowChange::Put { table, id, value }
                                if table == "worker_failures" && id == &node.0.to_string() =>
                            {
                                Some(value)
                            }
                            _ => None,
                        })
                        .or_else(|| {
                            snapshot
                                .rows
                                .iter()
                                .find(|row| {
                                    row.table == "worker_failures" && row.id == node.0.to_string()
                                })
                                .map(|row| &row.value)
                        });
                    let incident_generation = snapshot
                        .rows
                        .iter()
                        .find(|row| row.table == "nodes" && row.id == node.0.to_string())
                        .and_then(|row| row.value["generation"].as_u64());
                    if let Some(failure) = failure
                        && failure["generation"].as_u64() == incident_generation
                    {
                        summary.push_str(&format!(
                            " Last worker failure: {}: {}.",
                            failure["failure_class"].as_str().unwrap_or("unknown"),
                            failure["failure_message"]
                                .as_str()
                                .unwrap_or("worker run failed")
                        ));
                    }
                }
                if needs_rob {
                    // #403: an incident that needs Rob owes a postmortem.
                    commit.postmortems.push(postmortems::owed(
                        snapshot,
                        node,
                        format!("incident:{id}"),
                        postmortems::Reason::Escalation,
                        None,
                    )?);
                }
                commit.rows.push(put(
                    "incidents",
                    id.clone(),
                    json!(looks::Incident {
                        id: id.clone(),
                        node: Some(node),
                        input_failure: None,
                        open: true,
                        needs_rob,
                        blocking: n.waiting_incident,
                        summary: summary.clone(),
                        signature: signature.clone(),
                        effect: None,
                        post_key: Some(key.clone()),
                        cause: Some(looks::Cause::of(source, &class)),
                    }),
                ));
                delivery.note = Some(looks::incident_note(
                    &delivery.repository,
                    delivery.issue,
                    &summary,
                ));
                delivery.question_context = Some(json!({"issue_title":request.title,
                    "request":format!("{} (https://github.com/{}/{}/issues/{})", request.title,
                        delivery.repository.owner, delivery.repository.name, delivery.issue),
                    "signature":signature, "incident_id":id, "publication_diagnostic":diagnostic, "occurrences":occurrences,"worker_input_failure":input_failure}));
                if let Some(operations) = &self.bootstrap.operations {
                    delivery.repository = Repository {
                        owner: operations.owner.clone(),
                        name: operations.name.clone(),
                        installation: operations.installation_id,
                    };
                    delivery.issue = operations.number;
                }
            }
            if matches!(
                output,
                Out::Effect {
                    kind: tl3_kernel::EffectKind::StartRun(_)
                        | tl3_kernel::EffectKind::StartRebase
                        | tl3_kernel::EffectKind::SubmitChecks
                        | tl3_kernel::EffectKind::Enqueue,
                    ..
                }
            ) {
                commit
                    .rows
                    .push(put("watches", key.clone(), json!(delivery)));
            }
            commit.rows.push(self.allowance_row(delivery.clone()));
            commit.effects.push(delivery.into_effect());
        }
        dispatch_liveness::queue_pool_changes(snapshot, &previous, &world, commit)?;
        for (index, node) in world.nodes.iter().enumerate() {
            let old = previous
                .nodes
                .get(index)
                .map(|n| n.credit)
                .unwrap_or_default();
            let delta = node.credit.balance() - old.balance();
            let reason = if input_errors::deferred(commit) {
                format!("held:{}:{}", snapshot.revision.get(), commit.event_key)
            } else {
                commit.event_key.clone()
            };
            commit
                .credit
                .extend(snapshot::credit_entries(index, &reason, delta));
        }
        let closed: BTreeSet<_> = world
            .nodes
            .iter()
            .enumerate()
            .filter(|(_, node)| matches!(node.phase, tl3_kernel::Phase::Closed(_)))
            .map(|(index, _)| NodeId(index as u64))
            .collect();
        let closed_fact = |value: &Value| -> Result<bool> {
            let fact: adapters::mergify::QueueFact = decode_stored(value.clone())?;
            Ok(fact
                .node(snapshot)?
                .is_some_and(|node| closed.contains(&node)))
        };
        let mut rows = Vec::new();
        for row in std::mem::take(&mut commit.rows) {
            if let RowChange::Put { table, value, .. } = &row
                && table == "queue_facts"
                && closed_fact(value)?
            {
                continue;
            }
            rows.push(row);
        }
        commit.rows = rows;
        for row in snapshot
            .rows
            .iter()
            .filter(|row| row.table == "queue_facts")
        {
            if closed_fact(&row.value)? {
                commit.rows.push(RowChange::Delete {
                    table: "queue_facts".into(),
                    id: row.id.clone(),
                });
            }
        }
        for row in &snapshot.rows {
            let discard_rebase_context = row.table == "rebase_conflicts"
                && row
                    .id
                    .parse::<usize>()
                    .ok()
                    .and_then(|id| world.nodes.get(id))
                    .is_some_and(|node| {
                        node.merged.is_some() || matches!(node.phase, tl3_kernel::Phase::Closed(_))
                    });
            if discard_rebase_context
                || row.table == "check_reports"
                    && row
                        .id
                        .split(':')
                        .next()
                        .and_then(|id| id.parse::<u64>().ok())
                        .is_some_and(|id| closed.contains(&NodeId(id)))
            {
                commit.rows.push(RowChange::Delete {
                    table: row.table.clone(),
                    id: row.id.clone(),
                });
            }
        }
        self.retire_ended_effect_looks(snapshot, &world, commit)?;
        Ok(())
    }

    fn footprint(
        &mut self,
        snapshot: &Snapshot,
        world: &mut World,
        node: NodeId,
        paths: &[String],
    ) -> Result<tl3_kernel::Footprint> {
        let request = request(snapshot, node)?;
        let repo = self
            .store
            .intern_repo(&request.repo.owner, &request.repo.name)?;
        let files = paths
            .iter()
            .map(|p| {
                if p.is_empty() {
                    return Err(Error::Invalid("footprint path must be nonempty"));
                }
                self.store
                    .intern_path(repo, p)
                    .map_err(Error::from)
                    .map(|id| tl3_kernel::FileId(id.get() as u64))
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(world.intern_footprint(files))
    }

    fn operations_comment(
        &mut self,
        snapshot: &Snapshot,
        repository: &Repository,
        issue: u64,
        comment: &tl3_github::Comment,
        commit: &mut Commit,
    ) -> Result<bool> {
        let operations = self.bootstrap.operations.as_ref().is_some_and(|o| {
            o.owner.eq_ignore_ascii_case(&repository.owner)
                && o.name.eq_ignore_ascii_case(&repository.name)
                && o.installation_id == repository.installation
                && o.number == issue
        });
        enum OperationsCommand<'a> {
            ClearIncident(&'a str),
            ClearQuota(&'a str),
        }
        let body = comment.body.trim();
        let command = body
            .strip_prefix("clear incident ")
            .map(OperationsCommand::ClearIncident)
            .or_else(|| {
                body.strip_prefix("clear quota ")
                    .map(OperationsCommand::ClearQuota)
            });
        let request_incident = if let Some(OperationsCommand::ClearIncident(id)) = command {
            snapshot
                .rows
                .iter()
                .find(|r| r.table == "incidents" && r.id == id)
                .map(|row| -> Result<bool> {
                    let incident: looks::Incident = decode_stored(row.value.clone())?;
                    let Some(node) = incident.node else {
                        return Ok(incident
                            .input_failure
                            .as_ref()
                            .and_then(|f| f.target.as_ref())
                            .is_some_and(|target| {
                                target.issue == issue && target.repository == *repository
                            }));
                    };
                    let request = request(snapshot, node)?;
                    Ok(request.issue == issue
                        && request.repo.owner.eq_ignore_ascii_case(&repository.owner)
                        && request.repo.name.eq_ignore_ascii_case(&repository.name)
                        && request.repo.installation == repository.installation)
                })
                .transpose()?
                .unwrap_or(false)
        } else {
            false
        };
        if !operations && !request_incident {
            return Ok(false);
        }
        if comment.user.id != self.bootstrap.rob {
            return Ok(true);
        }
        let key = format!(
            "{}/{}/{}/{}",
            repository.owner.to_ascii_lowercase(),
            repository.name.to_ascii_lowercase(),
            repository.installation,
            comment.id
        );
        if snapshot
            .rows
            .iter()
            .any(|r| r.table == "operations_comments" && r.id == key)
        {
            return Ok(true);
        }
        if let Some(OperationsCommand::ClearIncident(id)) = command
            && let Some(row) = snapshot
                .rows
                .iter()
                .find(|r| r.table == "incidents" && r.id == id && r.value["open"] == true)
        {
            self.clear_incident(
                snapshot,
                decode_stored(row.value.clone())?,
                format!("incident-clear:{}:{id}", comment.id),
                commit,
            )?;
        }
        if operations && let Some(OperationsCommand::ClearQuota(model)) = command {
            commit.event_key = format!("model-quota-clear:{model}:{}", comment.id);
            let configured = self
                .adapters
                .get(executor::Upstream::Workers)?
                .configured_model(model);
            let line = if !configured {
                format!("{model} is not a configured model; no quota hold changed.")
            } else if !snapshot
                .rows
                .iter()
                .any(|row| row.table == "model_quotas" && row.id == model)
            {
                format!("no quota hold recorded for {model}")
            } else {
                let now = self.now_ms();
                self.operator_clear_quota(snapshot, model, now, commit)?;
                commit.rows.push(put(
                    "model_quota_clears",
                    model.into(),
                    json!({
                        "cleared_ms": now, "comment_id": comment.id, "operator": comment.user.id
                    }),
                ));
                let at = chrono::DateTime::from_timestamp_millis(now as i64)
                    .map(|at| at.format("%Y-%m-%d %H:%M:%S UTC").to_string())
                    .unwrap_or_else(|| now.to_string());
                format!("quota for {model} cleared by operator at {at}")
            };
            self.quota_clear_reply(snapshot, repository, issue, comment.id, line, commit);
        }
        commit.rows.push(put(
            "operations_comments",
            key,
            json!({"repository":repository, "issue":issue, "comment":comment}),
        ));
        Ok(true)
    }

    /// Rob's clear, which a new release also applies to TL3-side incidents (Q51).
    pub(crate) fn clear_incident(
        &mut self,
        snapshot: &Snapshot,
        mut incident: looks::Incident,
        inbox: String,
        commit: &mut Commit,
    ) -> Result<()> {
        if incident.blocking {
            commit.rows.push(put(
                "inbox",
                inbox,
                json!(Observation::Kernel(Event::Fact {
                    node: incident
                        .node
                        .ok_or(Error::Invalid("blocking incident needs a node"))?,
                    fact: Fact::IncidentCleared { by_rob: true }
                })),
            ));
        } else {
            // Q7: clearance covers the current persisted memory outcome, even
            // if the feed finished another held attempt before this comment.
            if let Some(failure) = incident.input_failure.as_mut()
                && failure.memory_version.is_some()
            {
                failure.memory_version = Some(self.store.memory_feed_version()?);
            }
            if incident.open && incident.signature.class == postmortems::PAUSED {
                self.reset_postmortem_pause(snapshot, commit);
            }
            self.authorize_approval_retry(snapshot, &incident, commit);
            self.resume_memory_incident(&incident, commit);
            incident.open = false;
            if !self.retry_notice(snapshot, &incident, commit)? {
                self.resume_incident_look(snapshot, &incident, commit)?;
            }
            commit
                .rows
                .push(put("incidents", incident.id.clone(), json!(incident)));
        }
        Ok(())
    }

    fn translate(
        &mut self,
        snapshot: &Snapshot,
        world: &mut World,
        observation: Observation,
        commit: &mut Commit,
    ) -> Result<Option<Event>> {
        if let Observation::GitHubComment {
            repository,
            issue,
            comment,
        } = &observation
            && self.operations_comment(snapshot, repository, *issue, comment, commit)?
        {
            return Ok(None);
        }

        let mut ci_details = match &observation {
            Observation::CiFailed { checks, .. } => findings::QueueCiEvidence {
                checks: checks.clone(),
                ..Default::default()
            },
            Observation::Queue(adapters::mergify::QueueFact {
                evidence: adapters::mergify::Evidence::CiFailed { details, .. },
                ..
            }) => details.clone(),
            _ => Default::default(),
        };
        ci_details = adapters::mergify::ci_evidence::bounded(ci_details);
        let ci_checks = ci_details.checks.clone();
        let mut failed_tests = ci_details.failed_tests.clone();
        let mut outside_paths = Vec::new();
        let mut outside_paths_total = 0;
        let mut detailed_review = false;
        let event = match observation {
            Observation::Prerequisite(fact) => {
                self.external_dependency_fact(snapshot, world, fact, commit)?;
                return Ok(None);
            }
            // Older controllers queued ScratchSaved after writing it with completion.
            Observation::ScratchSaved { .. }
            | Observation::ScratchPruned { .. }
            | Observation::TraceLogsCapped { .. } => {
                return Ok(None);
            }
            Observation::Progress { key, checkpoint } => {
                self.progress_allowance(snapshot, &key, checkpoint, commit)?;
                return Ok(None);
            }
            Observation::Quota {
                key,
                reset_ms,
                model,
            } => {
                self.quota_allowance(snapshot, &key, reset_ms, model.as_deref(), commit)?;
                return Ok(None);
            }
            Observation::Classified { key, reply } => {
                return self.classified(snapshot, world, &key, reply, commit);
            }
            Observation::Postmortem {
                key,
                reply,
                failure,
                judge_failure,
            } => {
                self.postmortem_finished(snapshot, &key, reply, failure, judge_failure, commit)?;
                return Ok(None);
            }
            Observation::LinkedRequest {
                repository,
                issue,
                original,
            } => {
                return self.linked_request(snapshot, repository, issue, original, commit);
            }
            Observation::Queue(mut fact) => {
                let identity = fact.identity();
                let Some(translation) =
                    tl3_controller_core::translate::begin(fact.recorded(snapshot)?)
                else {
                    return Ok(None);
                };
                let Some(node) = fact.node(snapshot)? else {
                    return Ok(None);
                };
                use adapters::mergify::Evidence;
                use tl3_controller_core::translate::{
                    self as core, Decision, IncidentAction, KernelKind as KernelFact,
                };
                use tl3_mergify::decide::translate as queue;
                let current = &world.nodes[node.0 as usize];
                let view = queue::NodeView {
                    phase: match current.phase {
                        tl3_kernel::Phase::Publishing => queue::Phase::Publishing,
                        tl3_kernel::Phase::Queued => queue::Phase::Queued,
                        tl3_kernel::Phase::Closed(_) => queue::Phase::Closed,
                        _ => queue::Phase::Other,
                    },
                    merged: current.merged.is_some(),
                    enqueue_pending: current.effect.is_some_and(|effect| {
                        core::enqueue_pending(
                            effect.kind == tl3_kernel::EffectKind::Enqueue,
                            effect.cancelling,
                        )
                    }),
                };
                // A closed node takes only a merge it has not recorded. A later
                // active enqueue must still be able to consume head evidence.
                if !queue::admit(fact.evidence.kind(), view) {
                    return Ok(None);
                }
                if let Evidence::CiFailed {
                    head,
                    infra,
                    head_check: false,
                    details,
                    ..
                } = &mut fact.evidence
                {
                    *details = adapters::mergify::ci_evidence::bounded(details.clone());
                    let plan = check_run(snapshot, node, Some(head))?;
                    let outside = adapters::mergify::ci_evidence::outside_change(
                        &fact.repository,
                        details,
                        plan.as_ref(),
                    );
                    let previous: Vec<_> = snapshot
                        .rows
                        .iter()
                        .filter(|row| row.table == "queue_facts")
                        .map(|row| core::PriorFailure {
                            same_repository: row.value["repository"] == json!(&fact.repository),
                            same_number: row.value["number"] == fact.number,
                            same_head: row.value["evidence"]["CiFailed"]["head"] == *head,
                            infra: row.value["evidence"]["CiFailed"]["infra"] == true,
                            overlapping_test:
                                row.value["evidence"]["CiFailed"]["details"]["failed_tests"]
                                    .as_array()
                                    .is_some_and(|previous| {
                                        previous.iter().any(|previous| {
                                            details
                                                .failed_tests
                                                .iter()
                                                .any(|test| previous["test"] == test.test)
                                        })
                                    }),
                        })
                        .collect();
                    let repeated = core::repeated(&previous);
                    let (classified, decision) = queue::classify_ci(*infra, outside, repeated);
                    *infra = classified;
                    details.decision = Some(decision.as_str().into());
                }
                let value = json!(&fact);
                let head_current = match &fact.evidence {
                    Evidence::CiFailed { head, .. }
                    | Evidence::Conflict { head, .. }
                    | Evidence::DequeuedByCommand { head, .. } => core::head_current(
                        self.head(node, head)?.0,
                        world.nodes[node.0 as usize].head.map(|head| head.0),
                    ),
                    _ => true,
                };
                let approved_head = match &fact.evidence {
                    Evidence::CheckMissing { head, .. } | Evidence::QueueBlocked { head, .. } => {
                        commit_sha(snapshot, current.approved) == Some(head)
                    }
                    _ => false,
                };
                let route =
                    match queue::route(fact.evidence.kind(), view, head_current, approved_head) {
                        queue::Route::Record => core::Route::Record,
                        queue::Route::Skip => core::Route::Skip,
                        queue::Route::SetupIncident
                        | queue::Route::MissingIncident
                        | queue::Route::BlockedIncident
                        | queue::Route::ClearIncident => core::Route::Incident,
                        queue::Route::Kernel(kernel) => core::Route::Kernel(match kernel {
                            queue::KernelFact::PrMerged { checks_passed } => {
                                KernelFact::PrMerged { checks_passed }
                            }
                            queue::KernelFact::CiFailed { infra } => KernelFact::CiFailed { infra },
                            queue::KernelFact::Conflict => KernelFact::Conflict,
                            queue::KernelFact::PrDequeuedByPerson => KernelFact::PrDequeuedByPerson,
                            queue::KernelFact::PrClosedByPerson => KernelFact::PrClosedByPerson,
                            queue::KernelFact::PushByOther => KernelFact::PushByOther,
                        }),
                    };
                let kind = match &fact.evidence {
                    Evidence::ChecksUnavailable { .. } => core::EvidenceKind::Setup,
                    Evidence::CheckMissing { .. } => core::EvidenceKind::Missing,
                    Evidence::QueueBlocked { .. } => core::EvidenceKind::Blocked,
                    Evidence::CheckReported { .. } => core::EvidenceKind::Reported,
                    Evidence::Merged { .. } => core::EvidenceKind::Merged,
                    Evidence::PushByOther { .. } => core::EvidenceKind::PushByOther,
                    _ => core::EvidenceKind::Other,
                };
                let kernel = match core::dispatch(translation, route, kind) {
                    Decision::Record => {
                        commit.rows.push(put("queue_facts", identity, value));
                        return Ok(None);
                    }
                    Decision::Skip => return Ok(None),
                    Decision::Kernel(kernel) => kernel,
                    // Setup rows (Q52, #447): none reaches the kernel, so no
                    // failure or credit is charged.
                    Decision::Incident(action) => {
                        match (action, fact.evidence) {
                            (
                                IncidentAction::Setup,
                                Evidence::ChecksUnavailable { permissions },
                            ) => {
                                self.check_setup_incident(snapshot, commit, node, &permissions)?;
                                commit.rows.push(put("queue_facts", identity, value));
                            }
                            (
                                IncidentAction::Missing,
                                Evidence::CheckMissing { head, required },
                            ) => {
                                self.missing_check_incident(
                                    snapshot, commit, node, &head, &required,
                                )?;
                                commit.rows.push(put("queue_facts", identity, value));
                            }
                            (
                                IncidentAction::Blocked,
                                Evidence::QueueBlocked {
                                    head,
                                    conditions,
                                    failed,
                                },
                            ) => {
                                self.queue_blocked_incident(
                                    snapshot,
                                    commit,
                                    node,
                                    &head,
                                    &conditions,
                                    &failed,
                                )?;
                                commit.rows.push(put("queue_facts", identity, value));
                            }
                            (IncidentAction::Clear, Evidence::CheckReported { head, required }) => {
                                let id = looks::missing_check_incident_id(node, &head, &required);
                                let incidents: Vec<_> = snapshot
                                    .rows
                                    .iter()
                                    .map(|row| core::IncidentRow {
                                        own_incident: row.table == "incidents" && row.id == id,
                                        open: row.value["open"] == true,
                                    })
                                    .collect();
                                if let Some(index) = core::clear_reported(&incidents) {
                                    let mut incident: looks::Incident =
                                        decode_stored(snapshot.rows[index].value.clone())?;
                                    incident.open = false;
                                    commit.rows.push(put("incidents", id, json!(incident)));
                                }
                            }
                            _ => {
                                return Err(Error::Invalid(
                                    "only setup evidence routes to an incident",
                                ));
                            }
                        }
                        return Ok(None);
                    }
                    Decision::InvalidIncident => {
                        return Err(Error::Invalid("only setup evidence routes to an incident"));
                    }
                    Decision::InvalidKernel => {
                        return Err(Error::Invalid(
                            "queue evidence routed to another kernel fact",
                        ));
                    }
                };
                let fact = match (kernel, fact.evidence) {
                    (KernelFact::PrMerged { checks_passed }, Evidence::Merged { head, .. }) => {
                        Fact::PrMerged {
                            head: self.head(node, &head)?,
                            checks_passed,
                        }
                    }
                    (KernelFact::CiFailed { infra }, _) => Fact::CiFailed { infra },
                    (KernelFact::Conflict, _) => Fact::Conflict,
                    (KernelFact::PrDequeuedByPerson, _) => Fact::PrDequeuedByPerson,
                    (KernelFact::PrClosedByPerson, _) => Fact::PrClosedByPerson,
                    (KernelFact::PushByOther, Evidence::PushByOther { head }) => {
                        let commits: Vec<_> = snapshot
                            .interned
                            .iter()
                            .map(|item| core::CommitRow {
                                is_commit: item.kind == tl3_store::InternKind::Commit,
                                id: item.id,
                            })
                            .collect();
                        let previous_sha =
                            core::previous_commit(&commits, current.head.map(|head| head.0 as i64))
                                .map(|index| snapshot.interned[index].value.as_str());
                        let carried = core::carry_plan(
                            node.0,
                            &head,
                            check_run(snapshot, node, previous_sha)?,
                        );
                        if let Some(carried) = carried {
                            commit.rows.push(put(
                                "check_plans",
                                format!("{}:{}", carried.node, carried.head),
                                json!(carried.plan),
                            ));
                        }
                        Fact::PushByOther {
                            head: self.head(node, &head)?,
                        }
                    }
                    _ => {
                        return Err(Error::Invalid(
                            "queue evidence routed to another kernel fact",
                        ));
                    }
                };
                commit.rows.push(put("queue_facts", identity, value));
                Event::Fact { node, fact }
            }
            Observation::GitHubComment {
                repository,
                issue,
                comment,
            } => {
                let author_matches = comment.user.id == self.bootstrap.rob;
                if tl3_controller_core::translate_misc::admit_comment(
                    author_matches,
                    author_matches && self.bootstrap.repos.contains(&repository),
                ) {
                    let id = format!(
                        "{}/{}/{}/{}",
                        repository.owner.to_ascii_lowercase(),
                        repository.name.to_ascii_lowercase(),
                        repository.installation,
                        comment.id
                    );
                    let retired = self.store.comment_retired(&id)?;
                    if tl3_controller_core::translate_misc::record_comment(
                        retired,
                        !retired
                            && snapshot
                                .rows
                                .iter()
                                .any(|row| row.table == "github_comments" && row.id == id),
                    ) {
                        let value =
                            json!({"repository": repository, "issue": issue, "comment": comment});
                        commit
                            .rows
                            .push(put("github_comments", id.clone(), value.clone()));
                        commit.rows.push(put("unclassified_comments", id, value));
                    }
                }
                return Ok(None);
            }
            Observation::Steering { node, command } => {
                return self.command_event(snapshot, world, node, command, commit);
            }
            Observation::Kernel(event) => {
                kernel_boundary::validate_raw_event(event).map_err(Error::Invalid)?
            }
            Observation::CiFailed { node, infra, .. } => Event::Fact {
                node,
                fact: Fact::CiFailed { infra },
            },
            Observation::WorktreePrepared {
                node,
                repository,
                base_sha,
                sha,
            } => {
                let Some(n) = world.nodes.get(node.0 as usize) else {
                    return Ok(None);
                };
                use tl3_controller_core::translate_checks::{Preparation, preparation};
                if preparation(
                    n.adopted,
                    n.adopted && commit_sha(snapshot, n.head) == Some(sha.as_str()),
                ) == Preparation::Ignore
                {
                    return Ok(None);
                }
                let id = node.0.to_string();
                let mut run = if let Some(materials) = self.store.worker_materials(&id)?
                    && let Some(run) = materials.get("check_run")
                {
                    decode_stored::<adapters::prismabuild::CheckRun>(run.clone())?
                } else {
                    let brief = self
                        .store
                        .worker_document(&id, "briefs")?
                        .ok_or(Error::Invalid("adoption needs its accepted brief"))?;
                    let plan = self
                        .store
                        .worker_document(&id, "plans")?
                        .ok_or(Error::Invalid("adoption needs its accepted plan"))?;
                    let inputs = production::maker_inputs(&brief, &plan)?;
                    adapters::prismabuild::CheckRun {
                        repository: repository.clone(),
                        base_sha: tl3_worktrees::CommitId::new(&base_sha)?,
                        reviewed_sha: None,
                        reproducing_unavailable: None,
                        selected_failures: Vec::new(),
                        plan: decode_stored(inputs["plan"]["checks"].clone())?,
                    }
                };
                run.repository = repository;
                run.base_sha = tl3_worktrees::CommitId::new(base_sha)?;
                commit
                    .rows
                    .push(put("check_plans", format!("{}:{sha}", node.0), json!(run)));
                return Ok(None);
            }
            Observation::CheckPlan { node, sha, run } => {
                request(snapshot, node)?;
                if let Some(n) = world.nodes.get(node.0 as usize)
                    && n.phase == tl3_kernel::Phase::Rebasing
                    && let Some(mut fixes) = self
                        .store
                        .worker_document(&node.0.to_string(), "review_fixes")?
                    && let tl3_controller_core::translate_checks::PlanUpdate::Retarget {
                        set_reviewed,
                        replace_tests,
                    } = tl3_controller_core::translate_checks::plan_update(
                        fixes["head_sha"].as_str() == commit_sha(snapshot, n.head),
                        run.reviewed_sha.is_some(),
                        run.reproducing_unavailable.is_some(),
                    )
                {
                    fixes["head_sha"] = json!(sha);
                    if set_reviewed && let Some(reviewed) = &run.reviewed_sha {
                        fixes["reviewed_sha"] = json!(reviewed);
                    }
                    if replace_tests && let Some(reason) = &run.reproducing_unavailable {
                        for finding in fixes["findings"].as_array_mut().unwrap() {
                            if finding.get("tests").is_some() {
                                finding.as_object_mut().unwrap().remove("tests");
                                finding["reason"] = json!(reason);
                            }
                        }
                    }
                    commit
                        .rows
                        .push(put("review_fixes", node.0.to_string(), fixes));
                }
                commit
                    .rows
                    .push(put("check_plans", format!("{}:{sha}", node.0), json!(run)));
                return Ok(None);
            }
            Observation::Checks {
                node,
                generation,
                sha,
                changed_paths,
                failed_tests: tests,
                report,
                pass,
                results,
                base_sha,
                action_key,
            } => {
                let request = request(snapshot, node)?;
                let repo = self
                    .store
                    .intern_repo(&request.repo.owner, &request.repo.name)?;
                let n = &world.nodes[node.index().ok_or(Error::Invalid("node index overflow"))?];
                let footprint: std::collections::BTreeSet<_> = world
                    .files(n.footprint)
                    .ok_or(Error::Invalid("unknown footprint"))?
                    .collect();
                let exact_head = n.head == Some(self.head(node, &sha)?);
                let decision = tl3_controller_core::translate_checks::checks_reply(
                    n.job,
                    n.generation,
                    generation,
                    exact_head,
                    pass,
                );
                if decision.record {
                    let prefix = format!("{}:", node.0);
                    let earlier: Vec<_> = snapshot
                        .rows
                        .iter()
                        .filter(|row| row.table == "check_reports" && row.id.starts_with(&prefix))
                        .collect();
                    for row in earlier
                        .iter()
                        .filter(|row| row.id != format!("{}:{sha}", node.0))
                    {
                        commit.rows.push(RowChange::Delete {
                            table: row.table.clone(),
                            id: row.id.clone(),
                        });
                    }
                    // Review must see an edit to a test that failed a selected check (#359).
                    let selected = adapters::prismabuild::selected_failures(
                        earlier.iter().map(|row| &row.value),
                        &results,
                    );
                    commit.rows.push(put(
                        "check_evidence",
                        format!("{}:{generation:020}", node.0),
                        evidence::check_record(
                            &json!({"head_sha":sha,"base_sha":base_sha,"action_key":action_key,
                            "report":report,"pass":pass,"results":results}),
                        ),
                    ));
                    let mut report = json!({"head_sha":sha, "base_sha": base_sha, "results": results, "action_key":action_key, "report":report, "pass":pass});
                    if !selected.is_empty() {
                        report["selected_failures"] = json!(selected);
                    }
                    commit
                        .rows
                        .push(put("check_reports", format!("{}:{sha}", node.0), report));
                }
                let mut in_footprint = true;
                for path in changed_paths {
                    let id = self.store.intern_path(repo, &path)?;
                    if !footprint.contains(&tl3_kernel::FileId(id.get() as u64)) {
                        outside_paths_total += 1;
                        if outside_paths.len() < findings::MAX_OUTSIDE_PATHS {
                            outside_paths.push(findings::clipped(&path, findings::MAX_PATH_CHARS));
                        }
                        in_footprint = false;
                    }
                }
                failed_tests = tests
                    .into_iter()
                    .take(findings::MAX_FAILED_TESTS)
                    .map(|test| findings::FailedTest {
                        test: findings::clipped(&test.test, 4096),
                        message: findings::clipped(&test.message, findings::MAX_MESSAGE_CHARS),
                    })
                    .collect();
                Event::Reply {
                    node,
                    generation,
                    reply: Reply::ChecksDone {
                        report,
                        pass: decision.pass,
                        in_footprint,
                    },
                }
            }
            Observation::WorkerFailed {
                node,
                generation,
                failure_class,
                failure_message,
            } => {
                use tl3_controller_core::translate_worker as worker;
                let n = world
                    .nodes
                    .get(node.0 as usize)
                    .ok_or(Error::Invalid("worker node missing"))?;
                if worker::worker_failed(n.run, n.generation, generation) == worker::Failure::Ignore
                {
                    return Ok(None);
                }
                commit.rows.push(put(
                    "worker_failures",
                    node.0.to_string(),
                    json!({
                        "node": node, "generation": generation,
                        "failure_class": failure_class, "failure_message": failure_message,
                    }),
                ));
                Event::Reply {
                    node,
                    generation,
                    reply: Reply::RunFailed,
                }
            }
            Observation::WorkerReply {
                node,
                generation,
                role,
                mut reply,
                model,
            } => {
                use tl3_controller_core::translate_worker as worker;
                let n = world
                    .nodes
                    .get(node.0 as usize)
                    .copied()
                    .ok_or(Error::Invalid("worker node missing"))?;
                if worker::worker_failed(n.run, n.generation, generation) == worker::Failure::Ignore
                {
                    return Ok(None);
                }
                let phase = match n.phase {
                    tl3_kernel::Phase::Ingesting => worker::Phase::Ingesting,
                    tl3_kernel::Phase::Auditing => worker::Phase::Auditing,
                    tl3_kernel::Phase::Planning { .. } => worker::Phase::Planning,
                    tl3_kernel::Phase::Making => worker::Phase::Making,
                    tl3_kernel::Phase::Reviewing => worker::Phase::Reviewing,
                    _ => worker::Phase::Other,
                };
                let worker_role = match role.as_str() {
                    "ingest" => worker::Role::Ingest,
                    "audit" => worker::Role::Audit,
                    "planner" => worker::Role::Planner,
                    "maker" => worker::Role::Maker,
                    "reviewer" => worker::Role::Reviewer,
                    "writer" => worker::Role::Writer,
                    "classifier" => worker::Role::Classifier,
                    "postmortem" => worker::Role::Postmortem,
                    _ => worker::Role::Unknown,
                };
                let valid_role = worker::valid_role(phase, worker_role);
                let mut mapped = match adapters::workers::protocol::map_reply(&role, &reply) {
                    Ok(reply) if valid_role => reply,
                    _ => {
                        return Ok(Some(Event::Reply {
                            node,
                            generation,
                            reply: Reply::RunFailed,
                        }));
                    }
                };
                // Legacy quota rows have no ordering identity. Preserve their
                // once-only reply consumption until fresh quota facts replace them.
                if let Some(row) = snapshot.rows.iter().find(|row| {
                    row.table == "model_quotas"
                        && Some(row.id.as_str()) == model.as_deref()
                        && row.value["finished_ms"].is_null()
                }) {
                    commit.rows.push(RowChange::Delete {
                        table: row.table.clone(),
                        id: row.id.clone(),
                    });
                }
                if let Some(protected) =
                    self.protected_reply(snapshot, commit, node, &role, &reply)?
                {
                    mapped = adapters::workers::protocol::MappedReply::Kernel(protected);
                }
                if role == "planner" && reply["kind"] == "leaf" {
                    let checks = reply["checks"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .enumerate()
                        .map(|(index, check)| {
                            let declared = |field: &str| check.get(field).cloned();
                            Ok(adapters::prismabuild::Check {
                                name: format!("check-{}", index + 1),
                                kind: serde_json::from_value(check["kind"].clone())?,
                                tests: adapters::workers::protocol::reply_field(check, "node_ids")?,
                                forcing: declared("forcing")
                                    .map(serde_json::from_value)
                                    .transpose()?,
                                proof: declared("proof").map(serde_json::from_value).transpose()?,
                                declared_symptom: declared("declared_symptom")
                                    .map(serde_json::from_value)
                                    .transpose()?,
                            })
                        })
                        .collect::<Result<Vec<_>>>()?;
                    if let Err(error) = adapters::prismabuild::validate_plan_size(&checks) {
                        match error {
                            Error::Invalid(message) => {
                                reply["violations"]
                                    .as_array_mut()
                                    .unwrap()
                                    .push(json!(message));
                                mapped = adapters::workers::protocol::MappedReply::Kernel(
                                    Reply::PlanCantMeet,
                                );
                            }
                            error => return Err(error),
                        }
                    }
                }
                if role == "reviewer" {
                    let mut criteria =
                        adapters::workers::review::criteria(&mut self.store, snapshot, node)?;
                    criteria["head_sha"] = json!(n.head.and_then(|head| {
                        snapshot
                            .interned
                            .iter()
                            .find(|i| {
                                i.kind == tl3_store::InternKind::Commit && i.id == head.0 as i64
                            })
                            .map(|i| i.value.clone())
                    }));
                    let blockers =
                        match adapters::workers::review::validate_reply(&reply, &criteria) {
                            Ok(blockers) => blockers,
                            Err(_) => {
                                return Ok(Some(Event::Reply {
                                    node,
                                    generation,
                                    reply: Reply::RunFailed,
                                }));
                            }
                        };
                    commit.rows.push(put(
                        "review_evidence",
                        format!("{}:{generation:020}", node.0),
                        evidence::review_record(
                            &json!({"head_sha":criteria["head_sha"],"verdict":reply["verdict"],
                            "blocking_findings":reply["blocking_findings"]}),
                        ),
                    ));
                    let verdict = match reply["verdict"].as_str() {
                        Some("approve") => worker::ReviewVerdict::Approve,
                        Some("changes") => worker::ReviewVerdict::Changes,
                        _ => worker::ReviewVerdict::Other,
                    };
                    let route = worker::review_route(verdict, n.head.map(|head| head.0));
                    match route {
                        worker::ReviewRoute::Approve(head) => {
                            mapped =
                                adapters::workers::protocol::MappedReply::Kernel(Reply::Approved {
                                    head: HeadId(head),
                                });
                        }
                        worker::ReviewRoute::MissingHead => {
                            return Err(Error::Invalid("review has no head"));
                        }
                        _ => {}
                    }
                    if route == worker::ReviewRoute::Changes {
                        let mut feedback = self
                            .store
                            .worker_document(&node.0.to_string(), "maker_findings")?
                            .unwrap_or(json!([]));
                        let reports = feedback
                            .as_array_mut()
                            .ok_or(Error::Invalid("invalid maker findings"))?;
                        let formatted: Vec<_> = blockers
                            .iter()
                            .take(findings::MAX_REVIEW_FINDINGS)
                            .map(|finding| {
                                format!(
                                    "{}: {}",
                                    findings::clipped(
                                        &finding.problem,
                                        findings::MAX_MESSAGE_CHARS
                                    ),
                                    findings::clipped(
                                        &finding.evidence,
                                        findings::MAX_MESSAGE_CHARS
                                    ),
                                )
                            })
                            .collect();
                        let lengths: Vec<_> = formatted
                            .iter()
                            .map(|report| report.chars().count())
                            .collect();
                        let count = worker::findings_prefix(
                            &lengths,
                            findings::MAX_REVIEW_FINDINGS,
                            findings::MAX_REPORT_CHARS,
                        );
                        for (index, (finding, report)) in
                            blockers.iter().zip(formatted).take(count).enumerate()
                        {
                            let goal_ids = if finding.citation.kind
                                == adapters::workers::review::CitationKind::Goal
                            {
                                vec![finding.citation.id.clone()]
                            } else {
                                Vec::new()
                            };
                            reports.push(serde_json::to_value(findings::Finding {
                                id: Some(format!("review-{generation}-{index}")),
                                reviewed_sha: Some(
                                    criteria["head_sha"]
                                        .as_str()
                                        .ok_or(Error::Invalid(
                                            "review finding needs its exact commit",
                                        ))?
                                        .to_owned(),
                                ),
                                kind: findings::Kind::ReviewFinding,
                                report,
                                goal_ids,
                                failed_tests: Vec::new(),
                                outside_paths: Vec::new(),
                                outside_paths_total: 0,
                                ci_checks: Vec::new(),
                            })?);
                        }
                        commit
                            .rows
                            .push(put("maker_findings", node.0.to_string(), feedback));
                        detailed_review = true;
                    }
                }
                let document = worker::document(worker_role).map(|document| match document {
                    worker::Document::Briefs => "briefs",
                    worker::Document::Audits => "audits",
                    worker::Document::MakerReplies => "maker_replies",
                    worker::Document::Plans => "plans",
                });
                if role == "planner" {
                    let mut findings = self
                        .store
                        .worker_document(&node.0.to_string(), "findings")?
                        .unwrap_or(json!([]));
                    let reports = findings
                        .as_array_mut()
                        .ok_or(Error::Invalid("findings must be text reports"))?;
                    for violation in adapters::workers::protocol::reply_field::<Vec<String>>(
                        &reply,
                        "violations",
                    )? {
                        let violation = json!(violation);
                        if !reports.contains(&violation) {
                            reports.push(violation);
                        }
                    }
                    if let Some(report) = reply["report"].as_str() {
                        reports.push(json!(report));
                    }
                    commit
                        .rows
                        .push(put("findings", node.0.to_string(), findings));
                    if let adapters::workers::protocol::MappedReply::Kernel(Reply::PlanLeaf {
                        fits,
                        ..
                    }) = mapped
                    {
                        let paths = adapters::workers::protocol::reply_field::<Vec<String>>(
                            &reply, "files",
                        )?;
                        mapped =
                            adapters::workers::protocol::MappedReply::Kernel(Reply::PlanLeaf {
                                footprint: self.footprint(snapshot, world, node, &paths)?,
                                fits,
                            });
                    }
                }
                if let Some(document) = document {
                    commit
                        .rows
                        .push(put(document, node.0.to_string(), reply.clone()));
                }
                if role == "ingest" && reply["kind"] == "brief" {
                    self.prepare_merge_dependencies(snapshot, world, node, &reply, commit)?;
                }
                if role == "ingest" {
                    commit.rows.push(put(
                        "goals",
                        node.0.to_string(),
                        json!(crate::goals::identified(
                            reply.get("goals").unwrap_or(&json!([]))
                        )?),
                    ));
                }
                let mapped = match mapped {
                    adapters::workers::protocol::MappedReply::Kernel(reply) => reply,
                    adapters::workers::protocol::MappedReply::Made(sha) => {
                        let inputs = self
                            .store
                            .worker_materials(&node.0.to_string())?
                            .ok_or(Error::Invalid("maker materials missing"))?;
                        let goals = inputs.get("maker").unwrap_or(&inputs)["goals"]
                            .as_array()
                            .ok_or(Error::Invalid("maker goals missing"))?;
                        let checks = adapters::workers::protocol::reply_field::<Vec<Value>>(
                            &reply,
                            "self_check",
                        )?;
                        let expected: BTreeSet<_> =
                            goals.iter().map(|g| g["id"].as_str()).collect();
                        let checked: BTreeSet<_> =
                            checks.iter().map(|c| c["goal_id"].as_str()).collect();
                        let feedback = self
                            .store
                            .worker_document(&node.0.to_string(), "maker_findings")?
                            .unwrap_or(json!([]));
                        let previous = commit_sha(snapshot, n.head);
                        let current = findings::current_review(&feedback, n.round, previous);
                        let reviewed = current.head;
                        let fixes = adapters::workers::reproducing::check(&reply, current);
                        let same_goals = expected == checked;
                        let same_count = checks.len() == goals.len();
                        let handover_valid = same_goals
                            && same_count
                            && crate::goals::handover(goals, &checks).is_ok();
                        if !worker::maker_self_check(same_goals, same_count, handover_valid) {
                            Reply::RunFailed
                        } else if let Ok(fixes) = fixes {
                            if !fixes.evidence["findings"].as_array().unwrap().is_empty() {
                                let mut run = check_run(snapshot, node, Some(&sha))?
                                    .or(check_run(snapshot, node, previous)?)
                                    .ok_or(Error::Invalid(
                                        "review fix needs its original check plan",
                                    ))?;
                                run.plan.retain(|check| {
                                    check.kind != adapters::prismabuild::Kind::Reproducing
                                });
                                run.plan.extend(fixes.checks);
                                run.reviewed_sha = run
                                    .plan
                                    .iter()
                                    .any(|check| {
                                        check.kind == adapters::prismabuild::Kind::Reproducing
                                    })
                                    .then_some(reviewed)
                                    .flatten()
                                    .map(tl3_worktrees::CommitId::new)
                                    .transpose()?;
                                commit.rows.push(put(
                                    "check_plans",
                                    format!("{}:{sha}", node.0),
                                    json!(run),
                                ));
                                let mut evidence = fixes.evidence;
                                evidence["head_sha"] = json!(sha);
                                commit
                                    .rows
                                    .push(put("review_fixes", node.0.to_string(), evidence));
                            }
                            Reply::MakerDone {
                                head: self.head(node, &sha)?,
                            }
                        } else {
                            Reply::RunFailed
                        }
                    }
                    adapters::workers::protocol::MappedReply::NeedsFiles { paths, fits } => {
                        let request = request(snapshot, node)?;
                        let repo = self
                            .store
                            .intern_repo(&request.repo.owner, &request.repo.name)?;
                        let mut files = world
                            .files(n.footprint)
                            .ok_or(Error::Invalid("unknown footprint"))?
                            .collect::<Vec<_>>();
                        for path in paths {
                            files.push(tl3_kernel::FileId(
                                self.store.intern_path(repo, &path)?.get() as u64,
                            ));
                        }
                        Reply::MakerNeedsFiles {
                            files: world.intern_footprint(files),
                            fits,
                        }
                    }
                };
                if mapped != Reply::RunFailed {
                    commit.rows.push(RowChange::Delete {
                        table: "worker_failures".into(),
                        id: node.0.to_string(),
                    });
                }
                if role == "reviewer" {
                    commit.rows.push(put(
                        "reviewer_session",
                        node.0.to_string(),
                        json!({"attempt": n.attempt, "replanned": n.replanned,
                            "review_restarted": n.review_restarted,
                            "out_of_room": reply["verdict"] == "out_of_room",
                            "generation": generation,
                            "head_sha": n.head.and_then(|head| snapshot.interned.iter()
                                .find(|i| i.kind == tl3_store::InternKind::Commit && i.id == head.0 as i64)
                                .map(|i| i.value.clone()))}),
                    ));
                }
                if role == "maker" && mapped != Reply::RunFailed {
                    commit.rows.push(put(
                        "maker_session",
                        node.0.to_string(),
                        json!({
                            "attempt": n.attempt, "replanned": n.replanned, "generation": generation,
                            "needs_files": reply["kind"] == "needs_files" && reply["fits"] == true,
                            "model": model,
                        }),
                    ));
                }
                Event::Reply {
                    node,
                    generation,
                    reply: mapped,
                }
            }
            Observation::Plan {
                node,
                generation,
                paths,
                fits,
            } => Event::Reply {
                node,
                generation,
                reply: Reply::PlanLeaf {
                    footprint: self.footprint(snapshot, world, node, &paths)?,
                    fits,
                },
            },
            Observation::Made {
                node,
                generation,
                sha,
            } => Event::Reply {
                node,
                generation,
                reply: Reply::MakerDone {
                    head: self.head(node, &sha)?,
                },
            },
            Observation::Reviewed {
                node,
                generation,
                sha,
            } => Event::Reply {
                node,
                generation,
                reply: Reply::Approved {
                    head: self.head(node, &sha)?,
                },
            },
            Observation::RebaseConflict {
                node,
                generation,
                head,
                target,
                files,
            } => {
                let current = world
                    .nodes
                    .get(node.0 as usize)
                    .ok_or(Error::Invalid("rebase observation has no request"))?;
                use tl3_controller_core::translate_arms::{
                    RebaseDisposition, RebasePhase, rebase_reply,
                };
                let phase = if current.phase == tl3_kernel::Phase::Rebasing {
                    RebasePhase::Rebasing
                } else {
                    RebasePhase::Other
                };
                if rebase_reply(phase, current.generation, generation) == RebaseDisposition::Ignore
                {
                    return Ok(None);
                }
                commit.rows.push(put(
                    "rebase_conflicts",
                    node.0.to_string(),
                    json!({"head": head, "target": target, "files": files}),
                ));
                Event::Reply {
                    node,
                    generation,
                    reply: Reply::RebaseNeedsChanges,
                }
            }
            Observation::Rebased {
                node,
                generation,
                sha,
            } => {
                let current = world
                    .nodes
                    .get(node.0 as usize)
                    .ok_or(Error::Invalid("rebase observation has no request"))?;
                use tl3_controller_core::translate_arms::{
                    RebaseDisposition, RebasePhase, rebase_reply,
                };
                let phase = if current.phase == tl3_kernel::Phase::Rebasing {
                    RebasePhase::Rebasing
                } else {
                    RebasePhase::Other
                };
                if rebase_reply(phase, current.generation, generation) == RebaseDisposition::Ignore
                {
                    return Ok(None);
                }
                commit.rows.push(RowChange::Delete {
                    table: "rebase_conflicts".into(),
                    id: node.0.to_string(),
                });
                Event::Reply {
                    node,
                    generation,
                    reply: Reply::RebaseClean {
                        head: self.head(node, &sha)?,
                    },
                }
            }
            Observation::Merged {
                node,
                sha,
                checks_passed,
            } => Event::Fact {
                node,
                fact: Fact::PrMerged {
                    head: self.head(node, &sha)?,
                    checks_passed,
                },
            },
            Observation::PullRequest { node, number } => {
                if number == 0 {
                    return Err(Error::Invalid("pull request number must be positive"));
                }
                let request = request(snapshot, node)?;
                let repo = self
                    .store
                    .intern_repo(&request.repo.owner, &request.repo.name)?;
                let id = self.store.intern_pull_request(repo, number)?;
                commit.rows.push(put(
                    "pull_requests",
                    node.0.to_string(),
                    json!({"number": number, "id": id.get()}),
                ));
                let mut outputs = Vec::new();
                self.announce_dependency_endings(
                    snapshot,
                    world,
                    vec![node],
                    &mut outputs,
                    commit,
                )?;
                self.record_kernel_result(snapshot, world.clone(), outputs, commit)?;
                return Ok(None);
            }
        };
        let finding = {
            use tl3_controller_core::translate_tail as tail;
            let (node, input) = match event {
                Event::Reply {
                    node,
                    generation,
                    reply,
                } => (
                    Some(node),
                    tail::Input::Reply {
                        generation,
                        reply: match reply {
                            Reply::ChecksDone {
                                report,
                                pass,
                                in_footprint,
                            } => tail::Reply::ChecksDone {
                                report,
                                pass,
                                in_footprint,
                            },
                            Reply::ChangesRequested => tail::Reply::ChangesRequested,
                            Reply::MakerCantMeet => tail::Reply::MakerCantMeet,
                            _ => tail::Reply::Other,
                        },
                    },
                ),
                Event::Fact {
                    node,
                    fact: Fact::CiFailed { infra },
                } => (Some(node), tail::Input::CiFailed { infra }),
                _ => (None, tail::Input::Other),
            };
            let current = node
                .and_then(|node| world.nodes.get(node.0 as usize))
                .map(|n| tail::Node {
                    phase: match n.phase {
                        tl3_kernel::Phase::Checking => tail::Phase::Checking,
                        tl3_kernel::Phase::Reviewing => tail::Phase::Reviewing,
                        tl3_kernel::Phase::Making => tail::Phase::Making,
                        tl3_kernel::Phase::Queued => tail::Phase::Queued,
                        _ => tail::Phase::Other,
                    },
                    generation: n.generation,
                    run: n.run,
                    job: n.job,
                });
            let report = match tail::finding(current, input, detailed_review) {
                tail::Finding::Check { pass, in_footprint } => Some((findings::Kind::FailedCheck,
                    findings::check_report(pass, in_footprint, &failed_tests, &outside_paths, outside_paths_total))),
                tail::Finding::Review => Some((findings::Kind::ReviewFinding,
                    "The independent reviewer requested changes. No detailed review report was supplied.".into())),
                tail::Finding::Maker => Some((findings::Kind::ReviewFinding,
                    commit.rows.iter().find_map(|row| match row {
                        RowChange::Put { table, value, .. } if table == "maker_replies" => Some(value.to_string()),
                        _ => None,
                    }).unwrap_or_else(|| "The maker could not meet the goals.".into()))),
                tail::Finding::Ci => Some((findings::Kind::FailedCheck,
                    findings::ci_report(&ci_checks, &failed_tests))),
                tail::Finding::None => None,
            };
            node.zip(report)
                .map(|(node, (kind, report))| (node, kind, report))
        };
        if let Some((node, kind, report)) = finding {
            let materials = self.store.worker_materials(&node.0.to_string())?;
            let goals = materials
                .as_ref()
                .and_then(|materials| materials.get("maker").unwrap_or(materials).get("goals"));
            let goal_ids = match goals {
                Some(goals) => goals
                    .as_array()
                    .ok_or(Error::Invalid("maker goals must be an array"))?
                    .iter()
                    .map(|goal| {
                        goal["id"]
                            .as_str()
                            .filter(|id| !id.trim().is_empty())
                            .map(str::to_owned)
                            .ok_or(Error::Invalid("maker goal needs a string id"))
                    })
                    .collect::<Result<Vec<_>>>()?,
                None => {
                    eprintln!(
                        "maker finding for node {} has no goal materials; retaining unbound evidence",
                        node.0
                    );
                    Vec::new()
                }
            };
            let mut findings = self
                .store
                .worker_document(&node.0.to_string(), "maker_findings")?
                .unwrap_or(json!([]));
            let finding = findings::Finding {
                id: None,
                reviewed_sha: None,
                kind,
                report: findings::clipped(&report, findings::MAX_REPORT_CHARS),
                goal_ids,
                failed_tests,
                outside_paths,
                outside_paths_total,
                ci_checks,
            };
            findings
                .as_array_mut()
                .ok_or(Error::Invalid("invalid maker findings"))?
                .push(serde_json::to_value(finding)?);
            commit
                .rows
                .push(put("maker_findings", node.0.to_string(), findings));
        }
        Ok(Some(self.priority_reserve(snapshot, event)?))
    }

    /// #416: the kernel starts credit at the ingest size plus its reserve of 3.
    /// A configured priority reserve adds its excess to the size, so the kernel
    /// stays unchanged; it reads the size only for starting credit.
    fn priority_reserve(&self, snapshot: &Snapshot, event: Event) -> Result<Event> {
        let Event::Reply {
            node,
            generation,
            reply: Reply::IngestDone { size },
        } = event
        else {
            return Ok(event);
        };
        if self.priority_reserve.is_empty() {
            return Ok(event);
        }
        let Some(reserve) = request(snapshot, node)?
            .priority
            .and_then(|priority| self.priority_reserve.get(&priority))
        else {
            return Ok(event);
        };
        let extra = u8::try_from(u64::from(*reserve).saturating_sub(tl3_kernel::RESERVE))
            .unwrap_or(u8::MAX);
        Ok(Event::Reply {
            node,
            generation,
            reply: Reply::IngestDone {
                size: size.saturating_add(extra),
            },
        })
    }
    fn head(&mut self, node: NodeId, sha: &str) -> Result<HeadId> {
        tl3_worktrees::CommitId::new(sha)?;
        Ok(HeadId(
            self.store.intern_commit(&node.0.to_string(), sha)?.get() as u64,
        ))
    }
}

fn stored_repositories(snapshot: &Snapshot) -> Result<Vec<Repository>> {
    let mut repositories = BTreeMap::new();
    for row in snapshot.rows.iter().filter(|row| row.table == "repos") {
        let index = row
            .id
            .parse::<usize>()
            .map_err(|_| Error::Invalid("invalid repository index"))?;
        let repository: Repository = decode_stored(row.value.clone())?;
        if repositories.insert(index, repository).is_some() {
            return Err(Error::Invalid("duplicate repository index"));
        }
    }
    let mut result = Vec::new();
    for (index, repository) in repositories {
        if index != result.len() {
            return Err(Error::Invalid("gap in repository indices"));
        }
        result.push(repository);
    }
    Ok(result)
}

fn latest_request<'a>(
    snapshot: &'a Snapshot,
    repo: &Repository,
    issue: u64,
) -> Option<&'a tl3_store::WorldRow> {
    snapshot
        .rows
        .iter()
        .filter(|r| {
            r.table == "requests"
                && r.value["issue"] == issue
                && serde_json::from_value::<Repository>(r.value["repo"].clone())
                    .is_ok_and(|stored| stored == *repo)
        })
        .max_by_key(|r| r.id.parse::<u64>().unwrap_or(0))
}

fn request(snapshot: &Snapshot, node: NodeId) -> Result<Request> {
    decode_stored(
        snapshot
            .rows
            .iter()
            .find(|r| r.table == "requests" && r.id == node.0.to_string())
            .ok_or(Error::Invalid("missing request binding"))?
            .value
            .clone(),
    )
}
fn put(table: &str, id: String, value: Value) -> RowChange {
    RowChange::Put {
        table: table.into(),
        id,
        value,
    }
}

fn hydrate(effect: &Effect, snapshot: &Snapshot) -> Result<Delivery> {
    let mut delivery: Delivery = decode_stored(effect.payload.clone())?;
    delivery.epoch = effect.epoch.get();
    delivery.checks = check_run(snapshot, delivery.node(), delivery.head_sha.as_deref())?;
    delivery.pull_request = snapshot
        .rows
        .iter()
        .find(|r| r.table == "pull_requests" && r.id == delivery.node().0.to_string())
        .and_then(|r| r.value["number"].as_u64());
    if matches!(
        delivery.output,
        DeliveryOutput::Notice {
            notice: tl3_kernel::Notice::MeasuresPosted,
            ..
        }
    ) {
        delivery.note = crate::goals::load(snapshot, delivery.node())?.map(|goals| {
            goals
                .iter()
                .filter_map(|g| g["description"].as_str())
                .map(|s| format!("- {s}"))
                .collect::<Vec<_>>()
                .join("\n")
        });
    }
    Ok(delivery)
}

fn inbox_key(key: &str, index: usize, observation: &Observation) -> Result<String> {
    Ok(format!("{key}:{index:08}:{}", evidence_key(observation)?))
}

fn evidence_key(observation: &Observation) -> Result<String> {
    Ok(format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(observation)?)
    ))
}

fn observation_upstream(
    snapshot: &Snapshot,
    observation: &Observation,
) -> Result<Option<executor::Upstream>> {
    use executor::Upstream;
    Ok(match observation {
        Observation::Progress { .. }
        | Observation::Quota { .. }
        | Observation::TraceLogsCapped { .. }
        | Observation::Steering { .. }
        | Observation::Kernel(Event::Fact {
            fact: Fact::UpstreamStalled,
            ..
        }) => None,
        Observation::GitHubComment { .. }
        | Observation::Prerequisite(_)
        | Observation::LinkedRequest { .. }
        | Observation::PullRequest { .. } => Some(Upstream::GitHub),
        Observation::Classified { .. }
        | Observation::Postmortem { .. }
        | Observation::WorkerReply { .. }
        | Observation::WorkerFailed { .. }
        | Observation::Plan { .. }
        | Observation::Made { .. }
        | Observation::Reviewed { .. } => Some(Upstream::Workers),
        Observation::CheckPlan { .. } | Observation::Checks { .. } => Some(Upstream::PrismaBuild),
        Observation::RebaseConflict { .. }
        | Observation::Rebased { .. }
        | Observation::WorktreePrepared { .. }
        | Observation::ScratchSaved { .. }
        | Observation::ScratchPruned { .. } => Some(Upstream::Worktrees),
        Observation::Merged { .. } | Observation::Queue(_) | Observation::CiFailed { .. } => {
            Some(Upstream::Mergify)
        }
        Observation::Kernel(Event::Reply {
            node, generation, ..
        }) => snapshot
            .rows
            .iter()
            .filter(|r| r.table == "allowances")
            .map(|r| decode_stored::<looks::AllowanceState>(r.value.clone()))
            .collect::<std::result::Result<Vec<_>, _>>()?
            .into_iter()
            .find(|s| {
                s.delivery.node() == *node
                    && s.delivery.generation == *generation
                    && match s.delivery.output {
                        DeliveryOutput::Effect { kind, .. } => snapshot::decode(snapshot)
                            .is_ok_and(|world| {
                                world
                                    .nodes
                                    .get(node.0 as usize)
                                    .is_some_and(|n| n.operation_pending(kind, *generation))
                            }),
                        DeliveryOutput::Notice { .. } | DeliveryOutput::Comment { .. } => false,
                    }
            })
            .map(|s| executor::upstream(s.delivery.output)),
        Observation::Kernel(Event::Fact {
            fact: Fact::IncidentCleared { .. },
            ..
        }) => None,
        Observation::Kernel(Event::Fact { .. }) => Some(Upstream::GitHub),
        Observation::Kernel(_) => None,
    })
}

fn queue_retirement(commit: &mut Commit, delivery: &Delivery) {
    if approval_delivery::is_approval(delivery) {
        for table in [
            approval_delivery::AUTHORIZATIONS,
            approval_delivery::LINEAGE_SAFE,
            "notice_incidents",
        ] {
            commit.rows.push(RowChange::Delete {
                table: table.into(),
                id: delivery.key.clone(),
            });
        }
    }
    if executor::upstream(delivery.output) == executor::Upstream::Mergify
        || approval_delivery::is_approval(delivery)
    {
        commit.rows.push(put(
            "retiring_effects",
            delivery.key.clone(),
            json!(delivery),
        ));
    }
}

fn check_run(
    snapshot: &Snapshot,
    node: NodeId,
    sha: Option<&str>,
) -> Result<Option<adapters::prismabuild::CheckRun>> {
    let Some(sha) = sha else {
        return Ok(None);
    };
    let mut run: Option<adapters::prismabuild::CheckRun> = snapshot
        .rows
        .iter()
        .find(|r| r.table == "check_plans" && r.id == format!("{}:{sha}", node.0))
        .map(|r| decode_stored(r.value.clone()))
        .transpose()?;
    if let Some(run) = &mut run {
        let prefix = format!("{}:", node.0);
        run.selected_failures = adapters::prismabuild::selected_failures(
            snapshot
                .rows
                .iter()
                .filter(|r| r.table == "check_reports" && r.id.starts_with(&prefix))
                .map(|r| &r.value),
            &[],
        );
    }
    Ok(run)
}

fn commit_sha(snapshot: &Snapshot, head: Option<HeadId>) -> Option<&str> {
    let head = head?;
    snapshot
        .interned
        .iter()
        .find(|item| item.kind == tl3_store::InternKind::Commit && item.id == head.0 as i64)
        .map(|item| item.value.as_str())
}

#[cfg(test)]
mod git_tests;

#[cfg(test)]
mod store_support;

#[cfg(test)]
mod store_support_tests;

#[cfg(test)]
mod stored_shape_tests {
    use super::*;

    // #375: persisted rows replay across releases. #416's new fields appear only
    // when set, so a request or input written today matches the old shape.
    #[test]
    fn old_request_rows_decode_and_new_fields_serialize_only_when_set() {
        let old = json!({
            "repo": {"owner": "owner", "name": "sandbox", "installation": 3},
            "issue": 12, "title": "Fix", "issue_id": 99, "source": null, "proposed": null,
        });
        let request: Request = decode_stored(old.clone()).unwrap();
        assert_eq!(request.priority, None);
        assert_eq!(json!(request), old);
        let ranked = Request {
            priority: Some(1),
            ..request
        };
        assert_eq!(json!(ranked)["priority"], 1);
    }

    #[test]
    fn selection_displacement_carries_request_history_into_check_delivery() {
        let f = git_tests::Repo::new();
        let run = f.delivery().checks.unwrap();
        let mut snapshot = tl3_store::Snapshot {
            revision: tl3_store::Revision::ZERO,
            epoch: tl3_store::Epoch::ZERO,
            rows: vec![],
            pending_effects: vec![],
            credit: vec![],
            interned: vec![],
        };
        snapshot.rows.push(tl3_store::WorldRow {
            arrival: 0,
            version: tl3_store::Revision::ZERO,
            table: "check_plans".into(),
            id: format!("0:{}", f.head),
            value: json!(run),
        });
        snapshot.rows.push(tl3_store::WorldRow {
            arrival: 0,
            version: tl3_store::Revision::ZERO,
            table: "check_reports".into(),
            id: "0:earlier".into(),
            value: json!({"selected_failures":["tests/test_old.py", "tests/test_companion.py"]}),
        });
        snapshot.rows.push(tl3_store::WorldRow {
            arrival: 0,
            version: tl3_store::Revision::ZERO,
            table: "check_reports".into(),
            id: "1:other-request".into(),
            value: json!({"selected_failures":["tests/test_unrelated.py"]}),
        });
        let next = check_run(&snapshot, NodeId(0), Some(&f.head))
            .unwrap()
            .unwrap();
        assert_eq!(
            next.selected_failures,
            ["tests/test_companion.py", "tests/test_old.py"]
        );
    }

    #[test]
    fn labelled_issues_serialize_labels_only_when_present() {
        let old = json!({
            "delivery": "label", "owner": "owner", "repo": "sandbox", "installation": 3,
            "number": 12, "title": "Fix", "actor": 42, "label": "tl3",
        });
        let issue: LabelledIssue = serde_json::from_value(old.clone()).unwrap();
        assert!(issue.labels.is_empty());
        assert_eq!(json!(issue), old);
    }
}

#[cfg(test)]
mod publication_tests;
