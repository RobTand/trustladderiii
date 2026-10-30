#![forbid(unsafe_code)]

pub mod findings;

pub mod adapters;
pub mod cli;
pub mod clock;
pub mod executor;
mod http;
pub mod ingress;
mod snapshot;
pub mod status;
pub mod status_web;

use executor::{Completion, Delivery, Observation, Reconciliation, Registry};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeSet;
use tl3_kernel::{Ack, Event, Fact, HeadId, NodeId, Out, Reply, World};
use tl3_store::{
    Commit, CreditEntry, Effect, EffectState, EffectTransition, NewEffect, RowChange, Snapshot,
    Store,
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
}

impl From<tl3_github::Error> for Error {
    fn from(error: tl3_github::Error) -> Self {
        Self::Adapter(Box::new(error))
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
    pub slots: u8,
    pub pool: u8,
}
impl Bootstrap {
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
    clock: Box<dyn clock::Clock>,
    github_translators: Vec<ingress::Translation>,
    held_effects: BTreeSet<String>,
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
        Ok(Self {
            store,
            adapters,
            bootstrap,
            clock: Box::new(clock),
            github_translators: Vec::new(),
            held_effects: BTreeSet::new(),
        })
    }

    pub fn now(&self) -> std::time::SystemTime {
        self.clock.now()
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
        if issue.actor != self.bootstrap.rob || issue.label != "tl3" {
            return Ok(false);
        }
        let Some(repo) = self
            .bootstrap
            .repos
            .iter()
            .find(|r| {
                r.owner.eq_ignore_ascii_case(&issue.owner)
                    && r.name.eq_ignore_ascii_case(&issue.repo)
                    && r.installation == issue.installation
            })
            .cloned()
        else {
            return Ok(false);
        };
        let snapshot = self.store.load()?;
        let world = snapshot::decode(&snapshot)?;
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
        // Distinct deliveries of the same label still name one request.
        let existing = snapshot
            .rows
            .iter()
            .filter(|r| r.table == "requests")
            .map(|r| serde_json::from_value::<Request>(r.value.clone()))
            .collect::<std::result::Result<Vec<_>, _>>()?
            .iter()
            .any(|r| r.issue_id == issue_id);
        let mut commit = Commit::new(
            format!("github:{}", issue.delivery),
            json!({"labelled_issue": issue}),
            &snapshot,
        );
        if !existing {
            let node = world.nodes.len().to_string();
            commit.rows.push(put(
                "requests",
                node,
                json!(Request {
                    repo,
                    issue: issue.number,
                    title: issue.title.clone(),
                    issue_id
                }),
            ));
            self.apply(
                &snapshot,
                world,
                Event::NewRequest {
                    repo: tl3_kernel::RepoId(index),
                },
                &mut commit,
            )?;
        }
        self.store.commit(&commit)?;
        self.retry_held_effects();
        Ok(true)
    }

    /// Upstream inbox keys must be stable across webhook redelivery and looks.
    pub fn observe(&mut self, key: &str, observation: Observation) -> Result<()> {
        let snapshot = self.store.load()?;
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
            if snapshot.rows.iter().any(|row| {
                (row.table == "github_comments" && row.id == id)
                    || (row.table == "inbox" && row.id == format!("external:{key}"))
            }) {
                return Ok(());
            }
        }
        let mut commit = Commit::new(format!("observe:{key}"), json!(&observation), &snapshot);
        commit
            .rows
            .push(put("inbox", format!("external:{key}"), json!(&observation)));
        self.store.commit(&commit)?;
        self.retry_held_effects();
        Ok(())
    }

    /// Allow one new attempt after input or an authoritative look (ticket #15).
    /// Holds are in memory, so reopening the controller also allows a retry.
    pub fn retry_held_effects(&mut self) {
        self.held_effects.clear();
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
                            Out::Effect {
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
        let mut progressed = false;
        let world = snapshot::decode(&snapshot)?;
        for effect in &snapshot.pending_effects {
            let delivery = hydrate(effect, &snapshot)?;
            let cancelling = effect.epoch < snapshot.epoch
                || match delivery.output {
                    Out::Effect { node, seq, .. } => world.nodes
                        [node.index().ok_or(Error::Invalid("node index overflow"))?]
                    .effect_cancelling(seq),
                    Out::Notice { .. } => false,
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
                        Err(error) => {
                            // A transport error is not an upstream acknowledgement.
                            eprintln!("adapter execute failed for {}: {error}", effect.key);
                            self.held_effects.insert(effect.key.clone());
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
                            self.complete(&effect.key, completion)?;
                            return Ok(true);
                        }
                        Reconciliation::Absent => {
                            if effect.state == EffectState::Cancelling {
                                self.cancel_absent(effect, &delivery)?;
                                return Ok(true);
                            }
                            if !self.held_effects.contains(&effect.key) {
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
            if effect.state.is_pending() {
                continue;
            }
            let active = effect.state == EffectState::Confirmed
                && match delivery.output {
                    Out::Effect { kind, .. } => node.operation_pending(kind, delivery.generation),
                    Out::Notice { .. } => false,
                };
            let mut commit = Commit::new(
                format!("look:{}:{}", row.id, snapshot.revision.get()),
                json!({"look": row.id}),
                &snapshot,
            );
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
                        Completion::confirmed(vec![Observation::Kernel(Event::Reply {
                            node: delivery.node(),
                            generation: delivery.generation,
                            reply: Reply::RunFailed,
                        })])
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
                    let progress_changed = record_progress(
                        &snapshot,
                        &mut commit,
                        &row.id,
                        completion.progress.as_ref(),
                    )?;
                    if progress_changed {
                        // Recording a look must not schedule another look by itself.
                        self.store.commit(&commit)?;
                    }
                    continue;
                }
                for (index, observation) in completion.observations.iter().enumerate() {
                    commit.rows.push(put(
                        "inbox",
                        format!("{}:{index:08}", delivery.key),
                        json!(observation),
                    ));
                }
            }
            commit.rows.push(RowChange::Delete {
                table: "watches".into(),
                id: row.id.clone(),
            });
            queue_retirement(&mut commit, &delivery);
            self.store.commit(&commit)?;
            return Ok(true);
        }
        self.adapters.retire(&mut self.store);
        Ok(progressed)
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
        if let Out::Effect { node, seq, .. } = delivery.output {
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
        self.held_effects.remove(&effect.key);
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
        if let Out::Effect { node, seq, .. } = delivery.output {
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
        if completion.ack == Ack::Retryable
            && state == EffectState::Intended
            && matches!(delivery.output, Out::Effect { .. })
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
            record_progress(&snapshot, &mut commit, key, completion.progress.as_ref())?;
            for (index, observation) in completion.observations.iter().enumerate() {
                commit.rows.push(put(
                    "inbox",
                    format!("{key}:{index:08}"),
                    json!(observation),
                ));
            }
        }
        self.store.commit(&commit)?;
        self.held_effects.remove(key);
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
        commit
            .rows
            .extend(snapshot::changes(&before, snapshot::encode(&world)?));
        commit.result = json!({"kernel_event": event});
        let dictionary = self.store.load()?.interned;
        for (index, output) in outputs.into_iter().enumerate() {
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
            let checks = check_run(snapshot, node, head_sha.as_deref())?;
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
            let delivery = Delivery {
                key: key.clone(),
                epoch: snapshot.epoch.get(),
                output,
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
                note: None,
                question_context,
            };
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
            commit.effects.push(NewEffect {
                key,
                kind: format!("{:?}", executor::upstream(output)),
                payload: json!(delivery),
            });
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

    fn translate(
        &mut self,
        snapshot: &Snapshot,
        world: &mut World,
        observation: Observation,
        commit: &mut Commit,
    ) -> Result<Option<Event>> {
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
        let event = match observation {
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
                    Evidence::PushByOther { head } => Fact::PushByOther {
                        head: self.head(node, &head)?,
                    },
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
                    if !snapshot
                        .rows
                        .iter()
                        .any(|row| row.table == "github_comments" && row.id == id)
                    {
                        commit.rows.push(put(
                            "github_comments",
                            id,
                            json!({"repository": repository, "issue": issue, "comment": comment}),
                        ));
                    }
                }
                return Ok(None);
            }
            Observation::Kernel(Event::Ack { .. } | Event::NewRequest { .. }) => {
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
                let document = match role.as_str() {
                    "ingest" => "briefs",
                    "audit" => "audits",
                    "maker" => "maker_replies",
                    _ => "plans",
                };
                if role == "planner" {
                    commit.rows.push(put(
                        "plan_reports",
                        format!("{}:{generation}", node.0),
                        reply.clone(),
                    ));
                    let mut findings = self
                        .store
                        .worker_document(&node.0.to_string(), "findings")?
                        .unwrap_or(json!([]));
                    let reports = findings
                        .as_array_mut()
                        .ok_or(Error::Invalid("findings must be text reports"))?;
                    for violation in reply["violations"].as_array().unwrap() {
                        if !reports.contains(violation) {
                            reports.push(violation.clone());
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
                        let paths = reply["files"]
                            .as_array()
                            .unwrap()
                            .iter()
                            .map(|p| p.as_str().unwrap().to_owned())
                            .collect::<Vec<_>>();
                        mapped =
                            adapters::workers::protocol::MappedReply::Kernel(Reply::PlanLeaf {
                                footprint: self.footprint(snapshot, world, node, &paths)?,
                                fits,
                            });
                    }
                }
                commit
                    .rows
                    .push(put(document, node.0.to_string(), reply.clone()));
                if role == "ingest" {
                    commit.rows.push(put(
                        "goals",
                        node.0.to_string(),
                        reply.get("goals").cloned().unwrap_or(json!([])),
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
                        let checks = reply["self_check"].as_array().unwrap();
                        let expected: BTreeSet<_> =
                            goals.iter().map(|g| g["id"].as_str()).collect();
                        let checked: BTreeSet<_> =
                            checks.iter().map(|c| c["goal_id"].as_str()).collect();
                        if expected != checked || checks.len() != goals.len() {
                            Reply::RunFailed
                        } else {
                            Reply::MakerDone {
                                head: self.head(node, &sha)?,
                            }
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
                    (tl3_kernel::Phase::Reviewing, Reply::ChangesRequested) => Some((
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
        Out::Notice {
            notice: tl3_kernel::Notice::MeasuresPosted,
            ..
        }
    ) {
        let goals = snapshot
            .rows
            .iter()
            .find(|r| r.table == "goals" && r.id == delivery.node().0.to_string());
        delivery.note = goals.and_then(|r| r.value.as_array()).map(|goals| {
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

fn queue_retirement(commit: &mut Commit, delivery: &Delivery) {
    if executor::upstream(delivery.output) == executor::Upstream::Mergify {
        commit.rows.push(put(
            "retiring_effects",
            delivery.key.clone(),
            json!(delivery),
        ));
    }
}

fn record_progress(
    snapshot: &Snapshot,
    commit: &mut Commit,
    key: &str,
    token: Option<&executor::ProgressToken>,
) -> Result<bool> {
    let Some(token) = token else { return Ok(false) };
    let Some(row) = snapshot
        .rows
        .iter()
        .find(|row| row.table == "watches" && row.id == key)
    else {
        return Ok(false);
    };
    let previous: Option<executor::WatchProgress> = row
        .value
        .get("progress")
        .map(|value| serde_json::from_value(value.clone()))
        .transpose()?;
    let progress = executor::WatchProgress {
        previous: previous.as_ref().map(|progress| progress.current.clone()),
        current: token.clone(),
    };
    if previous.as_ref() == Some(&progress) {
        return Ok(false);
    }
    let mut value = row.value.clone();
    value["progress"] = json!(progress);
    commit.rows.push(put("watches", key.into(), value));
    Ok(true)
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
