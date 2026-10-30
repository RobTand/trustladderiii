#![forbid(unsafe_code)]

pub mod adapters;
pub mod cli;
pub mod executor;
mod snapshot;
pub mod status;

use executor::{Completion, Delivery, Observation, Reconciliation, Registry};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
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
    pub actor: u64,
    pub label: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Request {
    repo: Repository,
    issue: u64,
    issue_id: i64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Boundary {
    BeforeDispatch,
    AfterDispatch,
}

pub struct Controller {
    store: Store,
    adapters: Registry,
    bootstrap: Bootstrap,
}
impl Controller {
    pub fn open(mut store: Store, adapters: Registry, bootstrap: Bootstrap) -> Result<Self> {
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
        })
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
        Ok(true)
    }

    /// Upstream inbox keys must be stable across webhook redelivery and looks.
    pub fn observe(&mut self, key: &str, observation: Observation) -> Result<()> {
        let snapshot = self.store.load()?;
        let mut commit = Commit::new(format!("observe:{key}"), json!(&observation), &snapshot);
        commit
            .rows
            .push(put("inbox", format!("external:{key}"), json!(&observation)));
        self.store.commit(&commit)?;
        Ok(())
    }

    pub fn drive_once(&mut self) -> Result<bool> {
        self.drive_once_with(|_, _| {})
    }

    /// The hook exposes durable boundaries for process-level fault injection.
    pub fn drive_once_with(&mut self, mut hook: impl FnMut(Boundary, &str)) -> Result<bool> {
        let snapshot = self.store.load()?;
        if let Some(row) = snapshot.rows.iter().find(|r| r.table == "inbox") {
            let observation: Observation = serde_json::from_value(row.value.clone())?;
            let mut world = snapshot::decode(&snapshot)?;
            let mut commit =
                Commit::new(format!("consume:{}", row.id), row.value.clone(), &snapshot);
            if let Some(event) = self.translate(&snapshot, &mut world, observation, &mut commit)? {
                self.apply(&snapshot, world, event, &mut commit)?;
            }
            commit.rows.push(RowChange::Delete {
                table: "inbox".into(),
                id: row.id.clone(),
            });
            self.store.commit(&commit)?;
            return Ok(true);
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
                    let result = match self.adapters.adapter(&delivery)?.reconcile(&delivery) {
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
                            self.transition(effect, EffectState::Intended)?;
                            progressed = true;
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
        for row in snapshot.rows.iter().filter(|row| row.table == "watches") {
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
                delivery.epoch = effect.epoch.get();
                let result = self.adapters.adapter(&delivery)?.reconcile(&delivery);
                let completion = match result {
                    Ok(Reconciliation::Found(completion)) => Some(completion),
                    Ok(Reconciliation::Absent)
                        if executor::upstream(delivery.output) == executor::Upstream::Workers =>
                    {
                        Some(Completion::confirmed(vec![Observation::Kernel(
                            Event::Reply {
                                node: delivery.node(),
                                generation: delivery.generation,
                                reply: Reply::RunFailed,
                            },
                        )]))
                    }
                    Err(error) => {
                        eprintln!("adapter watch failed for {}: {error}", delivery.key);
                        None
                    }
                    _ => None,
                };
                if let Some(completion) = completion {
                    if completion.ack != Ack::Confirmed || completion.observations.is_empty() {
                        continue;
                    }
                    for (index, observation) in completion.observations.iter().enumerate() {
                        commit.rows.push(put(
                            "inbox",
                            format!("{}:{index:08}", delivery.key),
                            json!(observation),
                        ));
                    }
                } else {
                    continue;
                }
            }
            commit.rows.push(RowChange::Delete {
                table: "watches".into(),
                id: row.id.clone(),
            });
            self.store.commit(&commit)?;
            return Ok(true);
        }
        Ok(progressed)
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
            commit.transitions.push(EffectTransition {
                key: key.into(),
                from: EffectState::Intended,
                to: EffectState::Cancelled,
            });
        }
        if completion.ack == Ack::Confirmed {
            for (index, observation) in completion.observations.iter().enumerate() {
                commit.rows.push(put(
                    "inbox",
                    format!("{key}:{index:08}"),
                    json!(observation),
                ));
            }
        }
        self.store.commit(&commit)?;
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
                note: None,
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
        Ok(())
    }

    fn translate(
        &mut self,
        snapshot: &Snapshot,
        world: &mut World,
        observation: Observation,
        commit: &mut Commit,
    ) -> Result<Option<Event>> {
        let event = match observation {
            Observation::Kernel(Event::Ack { .. } | Event::NewRequest { .. }) => {
                return Err(Error::Invalid(
                    "acknowledgements require an effect key; requests require authorized intake",
                ));
            }
            Observation::Kernel(event) => event,
            Observation::WorkerReply {
                node,
                generation,
                role,
                reply,
            } => {
                let n = world
                    .nodes
                    .get(node.0 as usize)
                    .ok_or(Error::Invalid("worker node missing"))?;
                if n.run != Some(generation) || n.generation != generation {
                    return Ok(None);
                }
                let valid_role = matches!(
                    (n.phase, role.as_str()),
                    (tl3_kernel::Phase::Ingesting, "ingest")
                        | (tl3_kernel::Phase::Auditing, "audit")
                );
                let mapped = match adapters::workers::protocol::map_reply(&role, &reply) {
                    Ok(reply) if valid_role => reply,
                    _ => {
                        return Ok(Some(Event::Reply {
                            node,
                            generation,
                            reply: Reply::RunFailed,
                        }));
                    }
                };
                let document = if role == "ingest" { "briefs" } else { "audits" };
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
            } => {
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
                Event::Reply {
                    node,
                    generation,
                    reply: Reply::PlanLeaf {
                        footprint: world.intern_footprint(files),
                        fits,
                    },
                }
            }
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
