#![forbid(unsafe_code)]

pub mod adoption;
pub mod findings;

pub mod adapters;
pub mod cli;
pub mod clock;
pub mod executor;
mod goals;
mod http;
pub mod ingress;
pub mod looks;
mod notices;
pub mod production;
mod snapshot;
pub mod status;
pub mod status_web;
pub mod steering;

use executor::{
    Completion, Delivery, Observation, Output as DeliveryOutput, Reconciliation, Registry,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use tl3_kernel::{Ack, Event, Fact, HeadId, NodeId, Out, Reply, World};
use tl3_store::{
    Commit, CreditEntry, Effect, EffectState, EffectTransition, RowChange, Snapshot, Store,
};

pub type Result<T> = std::result::Result<T, Error>;
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Store(#[from] tl3_store::Error),
    #[error("{0}")]
    Config(#[from] tl3_config::Error),
    #[error("invalid controller data: {0}")]
    Json(#[from] serde_json::Error),
    #[error("controller I/O failed")]
    Io(#[from] std::io::Error),
    #[error("upstream adapter failed: {0}")]
    Adapter(#[source] Box<dyn std::error::Error + Send + Sync>),
    #[error("{0}")]
    Invalid(&'static str),
    #[error("writer drafts failed the message checker after two reruns")]
    MessageRejected,
    #[error("upstream permanently failed: {class}")]
    Permanent {
        class: &'static str,
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },
    #[error("upstream quota exhausted")]
    Quota { retry_after: std::time::Duration },
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

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Repository {
    pub owner: String,
    pub name: String,
    pub installation: u64,
}
#[derive(Clone, Debug)]
pub struct Bootstrap {
    pub repos: Vec<Repository>,
    pub rob: u64,
    pub operations: Option<tl3_config::OperationsIssue>,
    pub slots: u8,
    pub pool: u8,
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
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Boundary {
    BeforeDispatch,
    AfterDispatch,
    BeforeReconcile,
    AfterReconcile,
}

pub struct Controller {
    store: Store,
    adapters: Registry,
    bootstrap: Bootstrap,
    github_translators: Vec<ingress::Translation>,
    clock: Box<dyn clock::Clock>,
    held_effects: BTreeMap<executor::Upstream, BTreeMap<String, String>>,
    allowances: tl3_config::Allowances,
}
impl Controller {
    pub fn open(store: Store, adapters: Registry, bootstrap: Bootstrap) -> Result<Self> {
        Self::open_with_clock(store, adapters, bootstrap, clock::SystemClock)
    }

    pub fn open_with_clock(
        mut store: Store,
        adapters: Registry,
        bootstrap: Bootstrap,
        clock: impl clock::Clock + 'static,
    ) -> Result<Self> {
        store.claim_controller()?;
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
        Ok(Self {
            store,
            adapters,
            bootstrap,
            github_translators: Vec::new(),
            clock: Box::new(clock),
            held_effects: BTreeMap::new(),
            allowances: tl3_config::Allowances::default(),
        })
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

    pub fn intake(&mut self, issue: LabelledIssue) -> Result<bool> {
        let Some(repository) = self.intake_repository(&issue) else {
            return Ok(false);
        };
        let snapshot = self.store.load()?;
        let mut commit = Commit::new(
            format!("github:{}", issue.delivery),
            json!({"labelled_issue": issue}),
            &snapshot,
        );
        if let Some(event) = self.prepare_intake(&snapshot, &issue, repository, &mut commit)? {
            self.apply(&snapshot, snapshot::decode(&snapshot)?, event, &mut commit)?;
        }
        self.store.commit(&commit)?;
        self.retry_held_effects(executor::Upstream::GitHub);
        Ok(true)
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

    fn prepare_intake(
        &mut self,
        snapshot: &Snapshot,
        issue: &LabelledIssue,
        repo: Repository,
        commit: &mut Commit,
    ) -> Result<Option<Event>> {
        let world = snapshot::decode(snapshot)?;
        let index = snapshot
            .rows
            .iter()
            .find(|r| r.table == "repos" && r.value == json!(repo))
            .ok_or(Error::Invalid(
                "served repository requires a snapshot migration",
            ))?
            .id
            .parse::<u64>()
            .map_err(|_| Error::Invalid("invalid repository index"))?;
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
                commit.rows.push(put("request_links", node.clone(), json!({
                    "original": original, "authorized_by": self.bootstrap.rob, "issue": replacement
                })));
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
            commit.rows.push(put(
                "requests",
                node,
                json!(Request {
                    repo,
                    issue: issue.number,
                    title: issue.title.clone(),
                    issue_id,
                    source,
                }),
            ));
            return Ok(Some(Event::NewRequest {
                repo: tl3_kernel::RepoId(index),
            }));
        }
        Ok(None)
    }

    /// Upstream inbox keys must be stable across webhook redelivery and looks.
    pub fn observe(&mut self, key: &str, observation: Observation) -> Result<()> {
        let snapshot = self.store.load()?;
        let news = observation_upstream(&snapshot, &observation)?;
        if let Observation::Queue(fact) = &observation {
            let identity = fact.identity();
            if snapshot.rows.iter().any(|row| (row.table == "queue_facts" && row.id == identity)
                || (row.table == "inbox" && serde_json::from_value::<Observation>(row.value.clone())
                    .ok().is_some_and(|pending| matches!(pending, Observation::Queue(pending) if pending.identity() == identity)))) {
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
        let mut commit = Commit::new(format!("observe:{key}"), json!(&observation), &snapshot);
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
        self.held_effects.remove(&source);
    }

    fn is_held(&self, delivery: &Delivery) -> bool {
        self.held_effects
            .get(&executor::upstream(delivery.output))
            .is_some_and(|effects| effects.contains_key(&delivery.key))
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
        let snapshot = self.store.load()?;
        for row in snapshot
            .rows
            .iter()
            .filter(|r| r.table == "retiring_effects")
        {
            match self.retire_effect(&snapshot, row) {
                Ok(()) => return Ok(true),
                Err(error) => eprintln!("adapter retirement failed for {}: {error}", row.id),
            }
        }
        if let Some(row) = snapshot.rows.iter().find(|r| r.table == "inbox") {
            let observation: Observation = serde_json::from_value(row.value.clone())?;
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
                            self.complete(&effect.key, completion)?;
                            return Ok(true);
                        }
                        waiting_for_queue = true;
                    }
                }
            }
            if !waiting_for_queue {
                let mut world = snapshot::decode(&snapshot)?;
                let mut commit =
                    Commit::new(format!("consume:{}", row.id), row.value.clone(), &snapshot);
                if let Some(event) =
                    self.translate(&snapshot, &mut world, observation, &mut commit)?
                {
                    self.apply(&snapshot, world, event, &mut commit)?;
                }
                commit.rows.push(RowChange::Delete {
                    table: "inbox".into(),
                    id: row.id.clone(),
                });
                self.store.commit(&commit)?;
                return Ok(true);
            }
        }
        if self.start_replacement(&snapshot)?
            || self.retire_comment(&snapshot)?
            || self.schedule_comment(&snapshot)?
        {
            return Ok(true);
        }
        let mut progressed = false;
        let world = snapshot::decode(&snapshot)?;
        for effect in &snapshot.pending_effects {
            let delivery = hydrate(effect, &snapshot)?;
            let cancelling = effect.epoch < snapshot.epoch
                || match delivery.output {
                    DeliveryOutput::Effect { node, seq, .. } => world.nodes
                        [node.index().ok_or(Error::Invalid("node index overflow"))?]
                    .effect_cancelling(seq),
                    DeliveryOutput::Notice { .. } | DeliveryOutput::Comment { .. } => false,
                };
            if cancelling {
                match effect.state {
                    EffectState::Intended => {
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
                    self.transition(effect, EffectState::Sent)?;
                    hook(Boundary::BeforeDispatch, &effect.key);
                    let current = self.store.load()?;
                    let delivery = hydrate(effect, &current)?;
                    let result = self.adapters.adapter(&delivery)?.execute(&delivery);
                    hook(Boundary::AfterDispatch, &effect.key);
                    match result {
                        Ok(completion) => {
                            self.complete(&effect.key, completion)?;
                            return Ok(true);
                        }
                        Err(Error::Permanent { class, .. }) => {
                            self.complete(&effect.key, Completion::permanent(class))?;
                            return Ok(true);
                        }
                        Err(error) => {
                            // A transport error is not an upstream acknowledgement.
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
                    if looks::quota_waiting(&snapshot, &effect.key, self.now_ms())? {
                        continue;
                    }
                    hook(Boundary::BeforeReconcile, &effect.key);
                    let reconciled = self.adapters.adapter(&delivery)?.reconcile(&delivery);
                    hook(Boundary::AfterReconcile, &effect.key);
                    let result = match reconciled {
                        Ok(result) => result,
                        Err(error) => {
                            eprintln!("adapter reconciliation failed for {}: {error}", effect.key);
                            continue;
                        }
                    };
                    match result {
                        Reconciliation::Found(completion) => {
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
                        Reconciliation::Pending => {}
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
            let mut delivery: Delivery = serde_json::from_value(row.value.clone())?;
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
                && if delivery.is_classifier() {
                    snapshot
                        .rows
                        .iter()
                        .any(|r| r.table == "comment_jobs" && r.id == delivery.key)
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
                    Ok(Reconciliation::Found(completion)) => completion,
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
        let delivery: Delivery = serde_json::from_value(row.value.clone())?;
        if executor::upstream(delivery.output) == executor::Upstream::Mergify {
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
        if to == EffectState::Uncertain
            && let Some(class) = self
                .held_effects
                .get(&executor::upstream(delivery.output))
                .and_then(|effects| effects.get(&effect.key))
        {
            self.failure_allowance(&snapshot, &effect.key, class, &mut commit)?;
        }
        self.store.commit(&commit)?;
        Ok(())
    }

    fn cancel_absent(&mut self, effect: &Effect, delivery: &Delivery) -> Result<()> {
        let snapshot = self.store.load()?;
        let mut commit = Commit::new(
            format!("cancel:{}:{}", effect.key, snapshot.revision.get()),
            json!({"cancelled_absent": effect.key}),
            &snapshot,
        );
        commit.transitions.push(EffectTransition {
            key: effect.key.clone(),
            from: effect.state,
            to: EffectState::Cancelled,
        });
        if let DeliveryOutput::Effect { node, seq, .. } = delivery.output {
            self.apply(
                &snapshot,
                snapshot::decode(&snapshot)?,
                Event::Ack {
                    node,
                    seq,
                    ack: Ack::Retryable,
                },
                &mut commit,
            )?;
        }
        self.store.commit(&commit)?;
        self.remove_hold(delivery);
        Ok(())
    }

    fn complete(&mut self, key: &str, completion: Completion) -> Result<()> {
        let snapshot = self.store.load()?;
        let effect = snapshot
            .pending_effects
            .iter()
            .find(|e| e.key == key)
            .ok_or(Error::Invalid("completion has no pending effect"))?;
        let delivery: Delivery = serde_json::from_value(effect.payload.clone())?;
        let mut commit = Commit::new(
            format!("completion:{key}:{}", effect.updated_revision.get()),
            json!(&completion),
            &snapshot,
        );
        if completion.ack == Ack::Permanent
            && let Some(class) = completion.failure_class.as_deref()
        {
            self.failure_allowance(&snapshot, key, class, &mut commit)?;
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
                Ack::Permanent => EffectState::Failed,
            }
        };
        commit.transitions.push(EffectTransition {
            key: key.into(),
            from,
            to: state,
        });
        if let DeliveryOutput::Effect { node, seq, .. } = delivery.output {
            self.apply(
                &snapshot,
                snapshot::decode(&snapshot)?,
                Event::Ack {
                    node,
                    seq,
                    ack: completion.ack,
                },
                &mut commit,
            )?;
        }
        if completion.ack == Ack::Permanent
            && !matches!(delivery.output, DeliveryOutput::Effect { .. })
        {
            self.failed_notice(
                &snapshot,
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
            queue_retirement(&mut commit, &delivery);
            commit.transitions.push(EffectTransition {
                key: key.into(),
                from: EffectState::Intended,
                to: EffectState::Cancelled,
            });
        }
        if completion.ack == Ack::Confirmed {
            if let Some(marker) = &delivery.marker_key {
                commit.rows.push(RowChange::Delete {
                    table: "notice_incidents".into(),
                    id: marker.clone(),
                });
            }
            self.record_progress(&snapshot, &mut commit, key, completion.progress.as_ref())?;
        }
        // Upstream facts remain evidence even when the attempted write was refused.
        for (index, observation) in completion.observations.iter().enumerate() {
            self.queue_observation(&snapshot, &mut commit, key, index, observation)?;
        }
        self.store.commit(&commit)?;
        self.remove_hold(&delivery);
        Ok(())
    }

    fn apply(
        &mut self,
        snapshot: &Snapshot,
        world: World,
        event: Event,
        commit: &mut Commit,
    ) -> Result<()> {
        let previous = snapshot::decode(snapshot)?;
        let before = snapshot::encode(&previous)?;
        let (world, outputs) = tl3_kernel::step(world, event);
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
        commit.result = json!({"kernel_event": event});
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
                self.pending_replacement(snapshot, commit, node)?;
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
                Some(value) => serde_json::from_value(value)?,
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
                let run: adapters::prismabuild::CheckRun = serde_json::from_value(run.clone())?;
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
                let context = json!({"node": node, "question": question, "generation": n.generation, "pieces": plan.as_ref().and_then(|p| p.get("pieces")).cloned().unwrap_or(json!([])), "plan": plan});
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
                paths,
                attempt: n.attempt,
                round: n.round,
                replanned: n.replanned,
                checks,
                job_generation: n.job,
                question: n.question,
                marker_key: None,
                note: None,
                question_context,
                steering: None,
                request_text: request.source,
            };
            if let Out::Notice {
                notice: tl3_kernel::Notice::Incident { needs_rob },
                ..
            } = output
            {
                let id = looks::incident_id(snapshot, index);
                let (source, class) = self.attach_incident(snapshot, node, &id, commit)?;
                let signature = looks::IncidentSignature {
                    upstream: source,
                    class: class.clone(),
                };
                let needs_rob =
                    looks::incident_needs_rob(looks::IncidentCause::Kernel { needs_rob });
                let mut summary = looks::incident_summary(&request.title, &signature, needs_rob);
                if class == "queue_unfinished"
                    && let Some(detail) = looks::queue_incident_detail(snapshot, node)
                {
                    summary.push(' ');
                    summary.push_str(&detail);
                }
                commit.rows.push(put(
                    "incidents",
                    id.clone(),
                    json!(looks::Incident {
                        id: id.clone(),
                        node,
                        open: true,
                        needs_rob,
                        blocking: n.waiting_incident,
                        summary: summary.clone(),
                        signature: signature.clone(),
                        effect: None,
                        post_key: Some(key.clone()),
                    }),
                ));
                delivery.note = Some(looks::incident_note(
                    &delivery.repository,
                    delivery.issue,
                    &id,
                    &summary,
                ));
                delivery.question_context = Some(json!({"issue_title":request.title,
                    "request":format!("{} (https://github.com/{}/{}/issues/{})", request.title,
                        delivery.repository.owner, delivery.repository.name, delivery.issue),
                    "signature":signature}));
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
        for (index, node) in world.nodes.iter().enumerate() {
            let old = previous
                .nodes
                .get(index)
                .map(|n| n.credit)
                .unwrap_or_default();
            let delta = node.credit.balance() - old.balance();
            if delta != 0 {
                commit.credit.push(CreditEntry {
                    node: index.to_string(),
                    root: index.to_string(),
                    reason: commit.event_key.clone(),
                    amount: delta,
                });
            }
        }
        let closed: BTreeSet<_> = world
            .nodes
            .iter()
            .enumerate()
            .filter(|(_, node)| matches!(node.phase, tl3_kernel::Phase::Closed(_)))
            .map(|(index, _)| NodeId(index as u64))
            .collect();
        let closed_fact = |value: &Value| -> Result<bool> {
            let fact: adapters::mergify::QueueFact = serde_json::from_value(value.clone())?;
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
            if row.table == "check_reports"
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
                self.store
                    .intern_path(repo, p)
                    .map(|id| tl3_kernel::FileId(id.get() as u64))
            })
            .collect::<tl3_store::Result<Vec<_>>>()?;
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
        if !operations {
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
        if let Some(id) = comment.body.trim().strip_prefix("clear incident ")
            && let Some(row) = snapshot
                .rows
                .iter()
                .find(|r| r.table == "incidents" && r.id == id && r.value["open"] == true)
        {
            let mut incident: looks::Incident = serde_json::from_value(row.value.clone())?;
            if incident.blocking {
                commit.rows.push(put(
                    "inbox",
                    format!("incident-clear:{}:{id}", comment.id),
                    json!(Observation::Kernel(Event::Fact {
                        node: incident.node,
                        fact: Fact::IncidentCleared { by_rob: true }
                    })),
                ));
            } else {
                incident.open = false;
                if !self.retry_notice(snapshot, &incident, commit)? {
                    self.resume_incident_look(snapshot, &incident, commit)?;
                }
                commit
                    .rows
                    .push(put("incidents", id.into(), json!(incident)));
            }
        }
        commit.rows.push(put(
            "operations_comments",
            key,
            json!({"repository":repository, "issue":issue, "comment":comment}),
        ));
        Ok(true)
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

        let ci_checks = match &observation {
            Observation::CiFailed { checks, .. } => checks
                .iter()
                .take(20)
                .map(|check| findings::CiFailure {
                    name: findings::clipped(&check.name, 256),
                    summary: findings::clipped(&check.summary, findings::MAX_MESSAGE_CHARS),
                    url: check.url.as_ref().map(|url| findings::clipped(url, 4096)),
                })
                .collect(),
            _ => Vec::new(),
        };
        let mut failed_tests = Vec::new();
        let mut outside_paths = Vec::new();
        let mut outside_paths_total = 0;
        let mut detailed_review = false;
        let event = match observation {
            Observation::Progress { key, checkpoint } => {
                self.progress_allowance(snapshot, &key, checkpoint, commit)?;
                return Ok(None);
            }
            Observation::Quota { key, reset_ms } => {
                self.quota_allowance(snapshot, &key, reset_ms, commit)?;
                return Ok(None);
            }
            Observation::Classified { key, reply } => {
                return self.classified(snapshot, world, &key, reply, commit);
            }
            Observation::LinkedRequest {
                repository,
                issue,
                original,
            } => {
                return self.linked_request(snapshot, repository, issue, original, commit);
            }
            Observation::Queue(fact) => {
                let identity = fact.identity();
                if snapshot
                    .rows
                    .iter()
                    .any(|row| row.table == "queue_facts" && row.id == identity)
                {
                    return Ok(None);
                }
                let Some(node) = fact.node(snapshot)? else {
                    return Ok(None);
                };
                use adapters::mergify::Evidence;
                let current = &world.nodes[node.0 as usize];
                if matches!(current.phase, tl3_kernel::Phase::Closed(_))
                    && (!matches!(fact.evidence, Evidence::Merged { .. })
                        || current.merged.is_some())
                {
                    return Ok(None);
                }
                let value = json!(&fact);
                if let Evidence::CiFailed { head, .. } | Evidence::Conflict { head, .. } =
                    &fact.evidence
                {
                    let observed = self.head(node, head)?;
                    if world.nodes[node.0 as usize].head != Some(observed) {
                        commit.rows.push(put("queue_facts", identity, value));
                        return Ok(None);
                    }
                }
                let fact = match fact.evidence {
                    Evidence::Merged {
                        head,
                        checks_passed,
                    } => Fact::PrMerged {
                        head: self.head(node, &head)?,
                        checks_passed,
                    },
                    Evidence::CiFailed { infra, .. } => Fact::CiFailed { infra },
                    Evidence::Conflict { .. } => Fact::Conflict,
                    Evidence::DequeuedByPerson { .. } => Fact::PrDequeuedByPerson,
                    Evidence::ClosedByPerson { .. } => Fact::PrClosedByPerson,
                    Evidence::PushByOther { head } => {
                        let previous_sha = current.head.and_then(|head| {
                            snapshot
                                .interned
                                .iter()
                                .find(|i| {
                                    i.kind == tl3_store::InternKind::Commit && i.id == head.0 as i64
                                })
                                .map(|i| i.value.as_str())
                        });
                        if let Some(run) = check_run(snapshot, node, previous_sha)? {
                            commit.rows.push(put(
                                "check_plans",
                                format!("{}:{head}", node.0),
                                json!(run),
                            ));
                        }
                        Fact::PushByOther {
                            head: self.head(node, &head)?,
                        }
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
                if comment.user.id == self.bootstrap.rob
                    && self.bootstrap.repos.contains(&repository)
                {
                    let id = format!(
                        "{}/{}/{}/{}",
                        repository.owner.to_ascii_lowercase(),
                        repository.name.to_ascii_lowercase(),
                        repository.installation,
                        comment.id
                    );
                    if !self.store.comment_retired(&id)?
                        && !snapshot
                            .rows
                            .iter()
                            .any(|row| row.table == "github_comments" && row.id == id)
                    {
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
            Observation::Kernel(
                Event::Ack { .. } | Event::NewRequest { .. } | Event::NewAdoption { .. },
            ) => {
                return Err(Error::Invalid(
                    "acknowledgements require an effect key; requests require authorized intake",
                ));
            }
            Observation::Kernel(event) => event,
            Observation::CiFailed { node, infra, .. } => Event::Fact {
                node,
                fact: Fact::CiFailed { infra },
            },
            Observation::CheckPlan { node, sha, run } => {
                request(snapshot, node)?;
                if let Some(n) = world.nodes.get(node.0 as usize)
                    && n.phase == tl3_kernel::Phase::Rebasing
                    && let Some(mut fixes) = self
                        .store
                        .worker_document(&node.0.to_string(), "review_fixes")?
                    && fixes["head_sha"].as_str() == commit_sha(snapshot, n.head)
                {
                    fixes["head_sha"] = json!(sha);
                    if let Some(reviewed) = &run.reviewed_sha {
                        fixes["reviewed_sha"] = json!(reviewed);
                    }
                    if let Some(reason) = &run.reproducing_unavailable {
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
                ..
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
                if n.job == Some(generation) && n.generation == generation && exact_head {
                    let prefix = format!("{}:", node.0);
                    for row in snapshot.rows.iter().filter(|row| {
                        row.table == "check_reports"
                            && row.id.starts_with(&prefix)
                            && row.id != format!("{}:{sha}", node.0)
                    }) {
                        commit.rows.push(RowChange::Delete {
                            table: row.table.clone(),
                            id: row.id.clone(),
                        });
                    }
                    commit.rows.push(put(
                        "check_reports",
                        format!("{}:{sha}", node.0),
                        json!({"base_sha": base_sha, "results": results}),
                    ));
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
                        pass: pass && exact_head,
                        in_footprint,
                    },
                }
            }
            Observation::WorkerReply {
                node,
                generation,
                role,
                reply,
            } => {
                let n = world
                    .nodes
                    .get(node.0 as usize)
                    .copied()
                    .ok_or(Error::Invalid("worker node missing"))?;
                if n.run != Some(generation) || n.generation != generation {
                    return Ok(None);
                }
                let valid_role = matches!(
                    (n.phase, role.as_str()),
                    (tl3_kernel::Phase::Ingesting, "ingest")
                        | (tl3_kernel::Phase::Auditing, "audit")
                        | (tl3_kernel::Phase::Making, "maker")
                        | (tl3_kernel::Phase::Planning { .. }, "planner")
                        | (tl3_kernel::Phase::Reviewing, "reviewer")
                );
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
                    if reply["verdict"] == "approve" {
                        mapped =
                            adapters::workers::protocol::MappedReply::Kernel(Reply::Approved {
                                head: n.head.ok_or(Error::Invalid("review has no head"))?,
                            });
                    }
                    if reply["verdict"] == "changes" {
                        let mut feedback = self
                            .store
                            .worker_document(&node.0.to_string(), "maker_findings")?
                            .unwrap_or(json!([]));
                        let reports = feedback
                            .as_array_mut()
                            .ok_or(Error::Invalid("invalid maker findings"))?;
                        let mut remaining = findings::MAX_REPORT_CHARS;
                        for (index, finding) in blockers
                            .iter()
                            .take(findings::MAX_REVIEW_FINDINGS)
                            .enumerate()
                        {
                            let report = format!(
                                "{}: {}",
                                findings::clipped(&finding.problem, findings::MAX_MESSAGE_CHARS),
                                findings::clipped(&finding.evidence, findings::MAX_MESSAGE_CHARS),
                            );
                            let length = report.chars().count();
                            if length > remaining {
                                break;
                            }
                            remaining -= length;
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
                let document = match role.as_str() {
                    "ingest" => Some("briefs"),
                    "audit" => Some("audits"),
                    "reviewer" => None,
                    "maker" => Some("maker_replies"),
                    _ => Some("plans"),
                };
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
                        if expected != checked || checks.len() != goals.len() {
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
                                    .map(str::to_owned);
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
                if role == "reviewer" {
                    commit.rows.push(put(
                        "reviewer_session",
                        node.0.to_string(),
                        json!({"attempt": n.attempt, "replanned": n.replanned,
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
                            "needs_files": reply["kind"] == "needs_files" && reply["fits"] == true
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
            Observation::Rebased {
                node,
                generation,
                sha,
            } => Event::Reply {
                node,
                generation,
                reply: Reply::RebaseClean {
                    head: self.head(node, &sha)?,
                },
            },
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
                return Ok(None);
            }
        };
        let finding = match event {
            Event::Reply {
                node,
                generation,
                reply,
            } if world.nodes.get(node.0 as usize).is_some_and(|n| {
                (n.run == Some(generation) || n.job == Some(generation))
                    && n.generation == generation
            }) =>
            {
                let n = &world.nodes[node.0 as usize];
                let report = match (n.phase, reply) {
                    (tl3_kernel::Phase::Checking, Reply::ChecksDone { report: true, pass, in_footprint })
                        if !pass || !in_footprint => Some((findings::Kind::FailedCheck,
                            findings::check_report(pass, in_footprint, &failed_tests, &outside_paths, outside_paths_total))),
                    (tl3_kernel::Phase::Reviewing, Reply::ChangesRequested) if !detailed_review => Some((
                        findings::Kind::ReviewFinding,
                        "The independent reviewer requested changes. No detailed review report was supplied.".into(),
                    )),
                    (tl3_kernel::Phase::Making, Reply::MakerCantMeet) => Some((
                        findings::Kind::ReviewFinding,
                        commit.rows.iter().find_map(|row| match row {
                            RowChange::Put { table, value, .. } if table == "maker_replies" => Some(value.to_string()),
                            _ => None,
                        }).unwrap_or_else(|| "The maker could not meet the goals.".into()),
                    )),
                    _ => None,
                };
                report.map(|(kind, report)| (node, kind, report))
            }
            Event::Fact {
                node,
                fact: Fact::CiFailed { infra: false },
            } if world
                .nodes
                .get(node.0 as usize)
                .is_some_and(|n| n.phase == tl3_kernel::Phase::Queued) =>
            {
                Some((
                    node,
                    findings::Kind::FailedCheck,
                    findings::ci_report(&ci_checks),
                ))
            }
            _ => None,
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
        Ok(Some(event))
    }
    fn head(&mut self, node: NodeId, sha: &str) -> Result<HeadId> {
        Ok(HeadId(
            self.store.intern_commit(&node.0.to_string(), sha)?.get() as u64,
        ))
    }
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
            r.table == "requests" && r.value["repo"] == json!(repo) && r.value["issue"] == issue
        })
        .max_by_key(|r| r.id.parse::<u64>().unwrap_or(0))
}

fn request(snapshot: &Snapshot, node: NodeId) -> Result<Request> {
    serde_json::from_value(
        snapshot
            .rows
            .iter()
            .find(|r| r.table == "requests" && r.id == node.0.to_string())
            .ok_or(Error::Invalid("missing request binding"))?
            .value
            .clone(),
    )
    .map_err(Into::into)
}
fn put(table: &str, id: String, value: Value) -> RowChange {
    RowChange::Put {
        table: table.into(),
        id,
        value,
    }
}

fn hydrate(effect: &Effect, snapshot: &Snapshot) -> Result<Delivery> {
    let mut delivery: Delivery = serde_json::from_value(effect.payload.clone())?;
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
        | Observation::Kernel(Event::Fact {
            fact: Fact::UpstreamStalled,
            ..
        }) => None,
        Observation::GitHubComment { .. }
        | Observation::LinkedRequest { .. }
        | Observation::PullRequest { .. } => Some(Upstream::GitHub),
        Observation::Classified { .. }
        | Observation::WorkerReply { .. }
        | Observation::Plan { .. }
        | Observation::Made { .. }
        | Observation::Reviewed { .. } => Some(Upstream::Workers),
        Observation::CheckPlan { .. } | Observation::Checks { .. } => Some(Upstream::PrismaBuild),
        Observation::Rebased { .. } => Some(Upstream::Worktrees),
        Observation::Merged { .. } | Observation::Queue(_) | Observation::CiFailed { .. } => {
            Some(Upstream::Mergify)
        }
        Observation::Kernel(Event::Reply {
            node, generation, ..
        }) => snapshot
            .rows
            .iter()
            .filter(|r| r.table == "allowances")
            .map(|r| serde_json::from_value::<looks::AllowanceState>(r.value.clone()))
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
    if executor::upstream(delivery.output) == executor::Upstream::Mergify {
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
    snapshot
        .rows
        .iter()
        .find(|r| r.table == "check_plans" && r.id == format!("{}:{sha}", node.0))
        .map(|r| serde_json::from_value(r.value.clone()))
        .transpose()
        .map_err(Into::into)
}

fn commit_sha(snapshot: &Snapshot, head: Option<HeadId>) -> Option<&str> {
    let head = head?;
    snapshot
        .interned
        .iter()
        .find(|item| item.kind == tl3_store::InternKind::Commit && item.id == head.0 as i64)
        .map(|item| item.value.as_str())
}
