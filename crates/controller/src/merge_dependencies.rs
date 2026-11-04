use crate::{Controller, Error, Repository, Request, Result, decode_stored, executor, put};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use tl3_kernel::{
    DependencyStatus, DependencyTarget, Event, Fact, NodeId, NoteKind, Out, Outcome, Phase,
    PrerequisiteKind, Reply, World,
};
use tl3_store::{Commit, RowChange, Snapshot};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ExternalFact {
    pub repository: Repository,
    pub kind: PrerequisiteKind,
    pub number: u64,
    pub status: DependencyStatus,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Declaration {
    target: DependencyTarget,
    status: DependencyStatus,
    source_quote: String,
}

pub fn issue_status(issue: &tl3_github::Issue) -> DependencyStatus {
    match (issue.state.as_str(), issue.state_reason.as_deref()) {
        ("closed", Some("completed")) => DependencyStatus::Delivered,
        ("closed", Some("not_planned")) => DependencyStatus::Abandoned,
        _ => DependencyStatus::Pending,
    }
}

pub(crate) fn dependency_note(notice: tl3_kernel::Notice) -> Option<String> {
    match notice {
        tl3_kernel::Notice::Note(NoteKind::DependencyAbandoned { number }) => Some(format!(
            "Prerequisite #{number} was closed as not planned. This request will wait until you change or stop it."
        )),
        tl3_kernel::Notice::Note(NoteKind::DependencyCycle { number }) => Some(format!(
            "Waiting for #{number} would create a cycle. This request will wait until you change or stop it."
        )),
        _ => None,
    }
}

fn same_repository(a: &Repository, b: &Repository) -> bool {
    a.installation == b.installation
        && a.owner.eq_ignore_ascii_case(&b.owner)
        && a.name.eq_ignore_ascii_case(&b.name)
}

fn bindings(snapshot: &Snapshot, commit: &Commit) -> Result<BTreeMap<NodeId, Request>> {
    let mut requests = BTreeMap::new();
    for row in snapshot.rows.iter().filter(|row| row.table == "requests") {
        requests.insert(
            NodeId(
                row.id
                    .parse()
                    .map_err(|_| Error::Invalid("invalid request node"))?,
            ),
            decode_stored(row.value.clone())?,
        );
    }
    for row in &commit.rows {
        if let RowChange::Put { table, id, value } = row
            && table == "requests"
        {
            requests.insert(
                NodeId(
                    id.parse()
                        .map_err(|_| Error::Invalid("invalid request node"))?,
                ),
                decode_stored(value.clone())?,
            );
        }
    }
    Ok(requests)
}

fn bound_root(
    requests: &BTreeMap<NodeId, Request>,
    snapshot: &Snapshot,
    repo: &Repository,
    kind: PrerequisiteKind,
    number: u64,
) -> Option<NodeId> {
    requests.iter().rev().find_map(|(&node, request)| {
        if !same_repository(&request.repo, repo) {
            return None;
        }
        let matches = match kind {
            PrerequisiteKind::Issue => request.issue == number,
            PrerequisiteKind::PullRequest => {
                request
                    .proposed
                    .as_ref()
                    .is_some_and(|pr| pr.number == number)
                    || snapshot.rows.iter().any(|row| {
                        row.table == "pull_requests"
                            && row.id == node.0.to_string()
                            && row.value["number"] == number
                    })
            }
        };
        matches.then_some(node)
    })
}

fn root_status(world: &World, node: NodeId) -> DependencyStatus {
    let n = &world.nodes[node.0 as usize];
    if n.merged.is_some()
        || matches!(
            n.phase,
            Phase::Closed(Outcome::Merged | Outcome::AlreadyDone)
        )
    {
        DependencyStatus::Delivered
    } else {
        DependencyStatus::Pending
    }
}

fn fact_key(repo: &Repository, kind: PrerequisiteKind, number: u64) -> String {
    format!(
        "{}/{}/{}/{kind:?}/{number}",
        repo.owner.to_ascii_lowercase(),
        repo.name.to_ascii_lowercase(),
        repo.installation
    )
}

fn stored_row(snapshot: &Snapshot, commit: &Commit, table: &str, id: &str) -> Option<Value> {
    commit
        .rows
        .iter()
        .rev()
        .find_map(|row| match row {
            RowChange::Put {
                table: t,
                id: key,
                value,
            } if t == table && key == id => Some(value.clone()),
            _ => None,
        })
        .or_else(|| {
            snapshot
                .rows
                .iter()
                .find(|r| r.table == table && r.id == id)
                .map(|r| r.value.clone())
        })
}

fn retained_fact(
    snapshot: &Snapshot,
    commit: &Commit,
    repo: &Repository,
    kind: PrerequisiteKind,
    number: u64,
) -> Result<Option<ExternalFact>> {
    stored_row(
        snapshot,
        commit,
        "prerequisite_facts",
        &fact_key(repo, kind, number),
    )
    .map(decode_stored::<ExternalFact>)
    .transpose()
    .map(|fact| fact.filter(|f| f.status != DependencyStatus::Pending))
}

fn retain_fact(
    snapshot: &Snapshot,
    commit: &mut Commit,
    fact: ExternalFact,
) -> Result<ExternalFact> {
    if let Some(previous) =
        retained_fact(snapshot, commit, &fact.repository, fact.kind, fact.number)?
    {
        return Ok(previous);
    }
    commit.rows.push(put(
        "prerequisite_facts",
        fact_key(&fact.repository, fact.kind, fact.number),
        json!(fact),
    ));
    Ok(fact)
}

fn apply_fact(
    world: &mut World,
    node: NodeId,
    fact: Fact,
    outputs: &mut Vec<Out>,
    commit: &mut Commit,
) {
    let event = Event::Fact { node, fact };
    let (next, emitted) = tl3_kernel::step(world.clone(), event);
    *world = next;
    outputs.extend(emitted);
    if !commit.result.is_object() {
        commit.result = json!({});
    }
    commit
        .result
        .as_object_mut()
        .unwrap()
        .entry("dependency_events")
        .or_insert_with(|| json!([]))
        .as_array_mut()
        .unwrap()
        .push(json!(event));
}

impl Controller {
    pub(crate) fn prepare_merge_dependencies(
        &mut self,
        snapshot: &Snapshot,
        world: &World,
        node: NodeId,
        reply: &Value,
        commit: &mut Commit,
    ) -> Result<()> {
        let prerequisites = reply.get("merge_prerequisites").and_then(Value::as_array);
        if prerequisites.is_none_or(Vec::is_empty) {
            commit
                .rows
                .push(put("merge_prerequisites", node.0.to_string(), json!([])));
            return Ok(());
        }
        let prerequisites = prerequisites.unwrap();
        let request = crate::request(snapshot, node)?;
        let requests = bindings(snapshot, commit)?;
        let materials = self
            .store
            .worker_materials(&node.0.to_string())?
            .ok_or(Error::Invalid("prerequisite source issue missing"))?;
        let issue = &materials["issue"];
        let mut sources = vec![
            issue["body"].as_str().unwrap_or_default(),
            issue["title"].as_str().unwrap_or_default(),
        ];
        if let Some(comments) = issue["comments"].as_array() {
            sources.extend(comments.iter().filter_map(|c| c["body"].as_str()));
        }
        let mut declarations = Vec::new();
        for prerequisite in prerequisites {
            let kind = match prerequisite["kind"].as_str() {
                Some("issue") => PrerequisiteKind::Issue,
                Some("pull_request") => PrerequisiteKind::PullRequest,
                _ => return Err(Error::Invalid("invalid merge prerequisite kind")),
            };
            let number = prerequisite["number"]
                .as_u64()
                .filter(|n| *n > 0)
                .ok_or(Error::Invalid("invalid merge prerequisite number"))?;
            let quote = prerequisite["source_quote"]
                .as_str()
                .filter(|q| !q.is_empty())
                .ok_or(Error::Invalid("merge prerequisite source quote missing"))?;
            let reference = format!("#{number}");
            if !sources.iter().any(|source| source.contains(quote))
                || !quote.match_indices(&reference).any(|(i, _)| {
                    quote[i + reference.len()..]
                        .chars()
                        .next()
                        .is_none_or(|c| !c.is_ascii_digit())
                })
                || declarations
                    .iter()
                    .any(|d: &Declaration| d.target.kind == kind && d.target.number == number)
            {
                return Err(Error::Invalid(
                    "merge prerequisite quote does not match the source issue",
                ));
            }
            let root = bound_root(&requests, snapshot, &request.repo, kind, number);
            let status =
                if let Some(fact) = retained_fact(snapshot, commit, &request.repo, kind, number)? {
                    fact.status
                } else {
                    let observed = if root
                        .is_some_and(|root| root_status(world, root) == DependencyStatus::Delivered)
                    {
                        DependencyStatus::Delivered
                    } else {
                        self.adapters
                            .get(executor::Upstream::GitHub)?
                            .prerequisite(&request.repo, kind, number)?
                    };
                    let status = if observed == DependencyStatus::Pending {
                        root.map_or(observed, |root| root_status(world, root))
                    } else {
                        observed
                    };
                    retain_fact(
                        snapshot,
                        commit,
                        ExternalFact {
                            repository: request.repo.clone(),
                            kind,
                            number,
                            status,
                        },
                    )?
                    .status
                };
            declarations.push(Declaration {
                target: DependencyTarget {
                    kind,
                    number,
                    request: root,
                },
                status,
                source_quote: quote.into(),
            });
        }
        commit.rows.push(put(
            "merge_prerequisites",
            node.0.to_string(),
            json!(declarations),
        ));
        Ok(())
    }

    pub(crate) fn declare_merge_dependencies(
        &self,
        world: &mut World,
        event: Event,
        outputs: &mut Vec<Out>,
        commit: &mut Commit,
    ) -> Result<()> {
        let Event::Reply {
            node,
            generation,
            reply: Reply::IngestDone { .. },
        } = event
        else {
            return Ok(());
        };
        if !world.nodes.get(node.0 as usize).is_some_and(|n| {
            n.phase == Phase::Ingesting && n.generation == generation && n.run == Some(generation)
        }) {
            return Ok(());
        }
        let declarations = commit.rows.iter().rev().find_map(|row| match row {
            RowChange::Put { table, id, value }
                if table == "merge_prerequisites" && id == &node.0.to_string() =>
            {
                Some(value.clone())
            }
            _ => None,
        });
        if let Some(declarations) = declarations {
            let declarations = decode_stored::<Vec<Declaration>>(declarations)?;
            for dependency in world.dependencies().to_vec() {
                if dependency.node == node
                    && !declarations.iter().any(|d| {
                        d.target.kind == dependency.target.kind
                            && d.target.number == dependency.target.number
                    })
                {
                    apply_fact(
                        world,
                        node,
                        Fact::DependencyRemove {
                            kind: dependency.target.kind,
                            number: dependency.target.number,
                        },
                        outputs,
                        commit,
                    );
                }
            }
            for declaration in declarations {
                apply_fact(
                    world,
                    node,
                    Fact::Dependency {
                        target: declaration.target,
                        status: declaration.status,
                    },
                    outputs,
                    commit,
                );
            }
        }
        Ok(())
    }

    pub(crate) fn announce_dependency_endings(
        &self,
        snapshot: &Snapshot,
        world: &mut World,
        outputs: &mut Vec<Out>,
        commit: &mut Commit,
    ) -> Result<()> {
        let requests = bindings(snapshot, commit)?;
        // Root completion must remain observable even when no waiter exists yet.
        for (&root, request) in &requests {
            if root_status(world, root) == DependencyStatus::Delivered {
                retain_fact(
                    snapshot,
                    commit,
                    ExternalFact {
                        repository: request.repo.clone(),
                        kind: PrerequisiteKind::Issue,
                        number: request.issue,
                        status: DependencyStatus::Delivered,
                    },
                )?;
                if let Some(pr) = request.proposed.as_ref() {
                    retain_fact(
                        snapshot,
                        commit,
                        ExternalFact {
                            repository: request.repo.clone(),
                            kind: PrerequisiteKind::PullRequest,
                            number: pr.number,
                            status: DependencyStatus::Delivered,
                        },
                    )?;
                }
                if let Some(number) =
                    stored_row(snapshot, commit, "pull_requests", &root.0.to_string())
                        .and_then(|pr| pr["number"].as_u64())
                {
                    retain_fact(
                        snapshot,
                        commit,
                        ExternalFact {
                            repository: request.repo.clone(),
                            kind: PrerequisiteKind::PullRequest,
                            number,
                            status: DependencyStatus::Delivered,
                        },
                    )?;
                }
            }
        }
        for dependency in world.dependencies().to_vec() {
            if dependency.rejected {
                continue;
            }
            let Some(request) = requests.get(&dependency.node) else {
                continue;
            };
            let root = bound_root(
                &requests,
                snapshot,
                &request.repo,
                dependency.target.kind,
                dependency.target.number,
            );
            let status = retained_fact(
                snapshot,
                commit,
                &request.repo,
                dependency.target.kind,
                dependency.target.number,
            )?
            .map_or_else(
                || root.map_or(DependencyStatus::Pending, |node| root_status(world, node)),
                |fact| fact.status,
            );
            let ending = root
                .filter(|root| world.nodes[root.0 as usize].phase.is_closed())
                .map(|root| {
                    format!(
                        "{}:{}:{}:{:?}:{}",
                        root.0,
                        world.nodes[root.0 as usize].generation,
                        dependency.node.0,
                        dependency.target.kind,
                        dependency.target.number
                    )
                });
            let unannounced = ending
                .as_ref()
                .is_some_and(|id| stored_row(snapshot, commit, "dependency_endings", id).is_none());
            if (dependency.status == DependencyStatus::Pending
                && (root != dependency.target.request || status != DependencyStatus::Pending))
                || unannounced
            {
                apply_fact(
                    world,
                    dependency.node,
                    Fact::Dependency {
                        target: DependencyTarget {
                            request: root,
                            ..dependency.target
                        },
                        status,
                    },
                    outputs,
                    commit,
                );
            }
            if unannounced {
                commit.rows.push(put(
                    "dependency_endings",
                    ending.unwrap(),
                    json!({"status":status}),
                ));
            }
        }
        Ok(())
    }

    pub(crate) fn external_dependency_fact(
        &mut self,
        snapshot: &Snapshot,
        world: &mut World,
        fact: ExternalFact,
        commit: &mut Commit,
    ) -> Result<()> {
        if fact.number == 0
            || !self
                .bootstrap
                .repos
                .iter()
                .any(|repo| same_repository(repo, &fact.repository))
        {
            return Err(Error::Invalid(
                "merge prerequisite fact is outside served repositories",
            ));
        }
        let fact = retain_fact(snapshot, commit, fact)?;
        let requests = bindings(snapshot, commit)?;
        let mut outputs = Vec::new();
        for dependency in world.dependencies().to_vec() {
            if dependency.target.kind == fact.kind
                && dependency.target.number == fact.number
                && requests
                    .get(&dependency.node)
                    .is_some_and(|r| same_repository(&r.repo, &fact.repository))
            {
                apply_fact(
                    world,
                    dependency.node,
                    Fact::Dependency {
                        target: dependency.target,
                        status: fact.status,
                    },
                    &mut outputs,
                    commit,
                );
            }
        }
        self.announce_dependency_endings(snapshot, world, &mut outputs, commit)?;
        self.record_kernel_result(snapshot, world.clone(), outputs, commit)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        Bootstrap, LabelledIssue,
        executor::Observation,
        git_tests::{Repo, support::Database},
    };
    use tl3_kernel::{Ack, Command, EarlyEnd};

    fn fixture(db: &mut Database, name: &str) -> (Repo, Controller, Repository) {
        let repo = Repo::new();
        let mut config =
            tl3_config::Config::parse(include_str!("../../config/example.toml")).unwrap();
        config.store = db.create(name);
        config.limits.agent_pool = 4;
        config.limits.w = 3;
        let bootstrap = Bootstrap::from_config(&config).unwrap();
        let repository = bootstrap.repos[0].clone();
        for number in [42, 2761, 2772, 2779] {
            crate::adapters::github::set_fake_issue(&repo.root.path().join("fakes"), &serde_json::from_value(json!({"number":number,"title":"Prerequisite","body":null,"state":"open","updated_at":"2026-11-03T23:00:00Z"})).unwrap());
        }
        let controller = Controller::open(
            tl3_store::Store::open(&config.store).unwrap(),
            crate::adapters::simulation(&repo.root.path().join("fakes")).unwrap(),
            bootstrap,
        )
        .unwrap();
        (repo, controller, repository)
    }

    fn intake(c: &mut Controller, repo: &Repository, number: u64) -> NodeId {
        let count = c.world().unwrap().nodes.len();
        c.intake(LabelledIssue {
            delivery: format!("intake-{number}-{count}"),
            owner: repo.owner.clone(),
            repo: repo.name.clone(),
            installation: repo.installation,
            number,
            title: "Implement after prerequisites".into(),
            actor: c.bootstrap.rob,
            label: "tl3".into(),
            labels: vec!["tl3".into()],
        })
        .unwrap();
        let node = NodeId(c.world().unwrap().nodes.len() as u64 - 1);
        confirm(c, node);
        node
    }

    fn event(c: &mut Controller, event: Event) {
        let snapshot = c.snapshot().unwrap();
        let world = crate::snapshot::decode(&snapshot).unwrap();
        let mut commit = Commit::new(
            format!("test-event-{}", snapshot.revision.get()),
            json!(event),
            &snapshot,
        );
        c.apply(&snapshot, world, event, &mut commit).unwrap();
        c.store.commit(&commit).unwrap();
    }

    fn confirm(c: &mut Controller, node: NodeId) {
        for _ in 0..32 {
            let Some(effect) = c.world().unwrap().nodes[node.0 as usize].effect else {
                return;
            };
            event(
                c,
                Event::Ack {
                    node,
                    seq: effect.seq,
                    ack: Ack::Confirmed,
                },
            );
        }
        panic!("effects did not settle");
    }

    fn reply(c: &mut Controller, node: NodeId, reply: Reply) {
        confirm(c, node);
        let generation = c.world().unwrap().nodes[node.0 as usize].generation;
        event(
            c,
            Event::Reply {
                node,
                generation,
                reply,
            },
        );
        confirm(c, node);
    }

    fn observation(c: &mut Controller, observation: Observation) {
        let snapshot = c.snapshot().unwrap();
        let mut world = crate::snapshot::decode(&snapshot).unwrap();
        let mut commit = Commit::new(
            format!("test-observation-{}", snapshot.revision.get()),
            json!(observation),
            &snapshot,
        );
        if let Some(event) = c
            .translate(&snapshot, &mut world, observation, &mut commit)
            .unwrap()
        {
            c.apply(&snapshot, world, event, &mut commit).unwrap();
        }
        c.store.commit(&commit).unwrap();
    }

    fn ingest_brief(c: &mut Controller, node: NodeId, numbers: &[u64]) {
        let quote = format!(
            "Merge prerequisites: {}",
            numbers
                .iter()
                .map(|n| format!("#{n}"))
                .collect::<Vec<_>>()
                .join(", ")
        );
        c.store
            .put_worker_materials(
                &node.0.to_string(),
                &json!({"issue":{"title":"Implement", "body":quote}}),
            )
            .unwrap();
        let generation = c.world().unwrap().nodes[node.0 as usize].generation;
        observation(
            c,
            Observation::WorkerReply {
                node,
                generation,
                role: "ingest".into(),
                model: None,
                reply: json!({"kind":"brief", "brief":"Implement after prerequisites merge", "goals":[{"description":"Implement", "verification":"test"}], "memory_ids":[], "files":[], "forecast":{"tokenizer":"o200k_base", "brief_tokens":1,"file_tokens":{},"overhead_tokens":0,"tokens":1,"fit_tokens":100,"context_tokens":150,"fits":true}, "merge_prerequisites": numbers.iter().map(|n| json!({"kind":"issue", "number":n,"source_quote":quote})).collect::<Vec<_>>()}),
            },
        );
    }

    fn ingest(c: &mut Controller, node: NodeId, numbers: &[u64]) {
        ingest_brief(c, node, numbers);
        reply(c, node, Reply::AuditAgrees);
    }

    fn external(c: &mut Controller, repo: &Repository, number: u64, status: DependencyStatus) {
        observation(
            c,
            Observation::Prerequisite(ExternalFact {
                repository: repo.clone(),
                kind: PrerequisiteKind::Issue,
                number,
                status,
            }),
        );
    }

    #[test]
    fn ingest_audit_wait_for_every_prerequisite_and_persist_delivery() {
        let mut db = Database::start();
        let (_repo, mut c, repo) = fixture(&mut db, "prerequisite_delivery");
        intake(&mut c, &repo, 2779);
        intake(&mut c, &repo, 2772);
        let waiter = intake(&mut c, &repo, 42);
        ingest(&mut c, waiter, &[2779, 2772]);
        assert_eq!(c.world().unwrap().nodes[2].phase, Phase::Unplanned);
        assert_eq!(c.world().unwrap().nodes[2].credit.balance(), 0);
        external(&mut c, &repo, 2779, DependencyStatus::Delivered);
        assert_eq!(c.world().unwrap().nodes[2].phase, Phase::Unplanned);
        // Decode the durable snapshot, rather than continuing with an in-memory world.
        let snapshot = c.snapshot().unwrap();
        assert_eq!(
            crate::snapshot::decode(&snapshot).unwrap().dependencies()[0].status,
            DependencyStatus::Delivered
        );
        assert!(
            snapshot
                .rows
                .iter()
                .any(|r| r.table == "prerequisite_facts")
        );
        external(&mut c, &repo, 2772, DependencyStatus::Delivered);
        assert_eq!(
            c.world().unwrap().nodes[2].phase,
            Phase::Planning { must_split: false }
        );
    }

    #[test]
    fn stopped_open_request_rebinds_and_announces_its_later_ending() {
        let mut db = Database::start();
        let (_repo, mut c, repo) = fixture(&mut db, "prerequisite_restart");
        let root = intake(&mut c, &repo, 2761);
        let waiter = intake(&mut c, &repo, 42);
        ingest(&mut c, waiter, &[2761]);
        event(
            &mut c,
            Event::Command {
                node: root,
                id: 1,
                command: Command::Stop,
            },
        );
        confirm(&mut c, root);
        assert_eq!(
            c.world().unwrap().nodes[0].phase,
            Phase::Closed(Outcome::Stopped)
        );
        assert_eq!(c.world().unwrap().nodes[1].phase, Phase::Unplanned);
        // Restart's intake transaction publishes a new binding for the same issue.
        let snapshot = c.snapshot().unwrap();
        let world = c.world().unwrap();
        let restarted = NodeId(world.nodes.len() as u64);
        let request = crate::request(&snapshot, root).unwrap();
        let mut commit = Commit::new("restart-prerequisite", json!(null), &snapshot);
        commit
            .rows
            .push(put("requests", restarted.0.to_string(), json!(request)));
        c.apply(
            &snapshot,
            world,
            Event::NewRequest {
                repo: tl3_kernel::RepoId(0),
                priority: tl3_kernel::Priority::Unranked,
            },
            &mut commit,
        )
        .unwrap();
        c.store.commit(&commit).unwrap();
        confirm(&mut c, restarted);
        assert_eq!(
            c.world().unwrap().dependencies()[0].target.request,
            Some(restarted)
        );
        reply(
            &mut c,
            restarted,
            Reply::IngestEarlyEnd(EarlyEnd::AlreadyDone),
        );
        reply(&mut c, restarted, Reply::AuditAgrees);
        assert_eq!(
            c.world().unwrap().nodes[1].phase,
            Phase::Planning { must_split: false }
        );
    }

    #[test]
    fn abandonment_and_cycles_remain_blocked_and_emit_one_note() {
        let mut db = Database::start();
        let (_repo, mut c, repo) = fixture(&mut db, "prerequisite_abandoned");
        let a = intake(&mut c, &repo, 2761);
        let b = intake(&mut c, &repo, 42);
        ingest(&mut c, a, &[42]);
        ingest(&mut c, b, &[2761]);
        let world = c.world().unwrap();
        assert!(world.dependency_graph_acyclic());
        assert!(world.dependencies()[1].rejected);
        assert!(world.nodes.iter().all(|n| n.phase == Phase::Unplanned));
        external(&mut c, &repo, 42, DependencyStatus::Abandoned);
        external(&mut c, &repo, 42, DependencyStatus::Abandoned);
        external(&mut c, &repo, 42, DependencyStatus::Delivered);
        let snapshot = c.snapshot().unwrap();
        let notes = snapshot
            .pending_effects
            .iter()
            .filter(|r| r.payload.to_string().contains("DependencyAbandoned"))
            .count();
        assert_eq!(notes, 1);
        assert_eq!(c.world().unwrap().nodes[0].phase, Phase::Unplanned);
    }

    #[test]
    fn declaration_reconciles_already_observed_delivery_for_active_root() {
        let mut db = Database::start();
        let (_repo, mut c, repo) = fixture(&mut db, "observed_delivery");
        intake(&mut c, &repo, 2761);
        external(&mut c, &repo, 2761, DependencyStatus::Delivered);
        external(&mut c, &repo, 2761, DependencyStatus::Pending);
        let waiter = intake(&mut c, &repo, 42);
        ingest(&mut c, waiter, &[2761]);
        let world = c.world().unwrap();
        assert_eq!(world.dependencies()[0].status, DependencyStatus::Delivered);
        assert_eq!(
            world.nodes[waiter.0 as usize].phase,
            Phase::Planning { must_split: false }
        );
    }

    #[test]
    fn declaration_reconciles_already_observed_abandonment_for_active_root() {
        let mut db = Database::start();
        let (_repo, mut c, repo) = fixture(&mut db, "observed_abandonment");
        intake(&mut c, &repo, 2761);
        external(&mut c, &repo, 2761, DependencyStatus::Abandoned);
        external(&mut c, &repo, 2761, DependencyStatus::Pending);
        let waiter = intake(&mut c, &repo, 42);
        ingest(&mut c, waiter, &[2761]);
        assert_eq!(
            c.world().unwrap().dependencies()[0].status,
            DependencyStatus::Abandoned
        );
        assert_eq!(
            c.world().unwrap().nodes[waiter.0 as usize].phase,
            Phase::Unplanned
        );
    }

    #[test]
    fn reingest_retains_abandonment_and_announces_once() {
        let mut db = Database::start();
        let (_repo, mut c, repo) = fixture(&mut db, "reingest_abandoned");
        intake(&mut c, &repo, 2761);
        let waiter = intake(&mut c, &repo, 42);
        ingest_brief(&mut c, waiter, &[2761]);
        external(&mut c, &repo, 2761, DependencyStatus::Abandoned);
        reply(&mut c, waiter, Reply::AuditDisagrees);
        assert_eq!(
            c.world().unwrap().nodes[waiter.0 as usize].phase,
            Phase::Ingesting
        );
        ingest(&mut c, waiter, &[2761]);
        assert_eq!(
            c.world().unwrap().dependencies()[0].status,
            DependencyStatus::Abandoned
        );
        assert_eq!(
            c.world().unwrap().nodes[waiter.0 as usize].phase,
            Phase::Unplanned
        );
        assert_eq!(
            c.snapshot()
                .unwrap()
                .pending_effects
                .iter()
                .filter(|r| r.payload.to_string().contains("DependencyAbandoned"))
                .count(),
            1
        );
    }

    #[test]
    fn stopped_root_announces_once_even_when_status_stays_pending() {
        let mut db = Database::start();
        let (_repo, mut c, repo) = fixture(&mut db, "stop_announcement");
        let root = intake(&mut c, &repo, 2761);
        let waiter = intake(&mut c, &repo, 42);
        ingest(&mut c, waiter, &[2761]);
        let revision = c.snapshot().unwrap().revision;
        event(
            &mut c,
            Event::Command {
                node: root,
                id: 1,
                command: Command::Stop,
            },
        );
        confirm(&mut c, root);
        let announcements = |c: &mut Controller| {
            c.store
                .events_after(revision, 100)
                .unwrap()
                .iter()
                .flat_map(|r| {
                    r.receipt.result["dependency_events"]
                        .as_array()
                        .cloned()
                        .unwrap_or_default()
                })
                .filter(|e| {
                    e["Fact"]["node"] == json!(waiter)
                        && e["Fact"]["fact"]["Dependency"]["status"] == "Pending"
                })
                .count()
        };
        assert_eq!(
            announcements(&mut c),
            1,
            "a stopped open prerequisite announces its ending"
        );
        external(&mut c, &repo, 99, DependencyStatus::Delivered);
        confirm(&mut c, root);
        assert_eq!(announcements(&mut c), 1, "an ending is announced only once");
        assert_eq!(
            c.world().unwrap().nodes[waiter.0 as usize].phase,
            Phase::Unplanned
        );
    }

    #[test]
    fn issue_terminal_reason_distinguishes_completion_and_abandonment() {
        for (state, reason, expected) in [
            ("open", None, DependencyStatus::Pending),
            ("closed", Some("completed"), DependencyStatus::Delivered),
            ("closed", Some("not_planned"), DependencyStatus::Abandoned),
            ("closed", None, DependencyStatus::Pending),
        ] {
            let issue = serde_json::from_value(json!({"number":2761,"title":"Prerequisite","body":null,"state":state,"state_reason":reason,"updated_at":"2026-11-03T23:00:00Z","pull_request":null})).unwrap();
            assert_eq!(issue_status(&issue), expected);
        }
    }
}
