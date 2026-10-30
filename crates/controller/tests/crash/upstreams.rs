use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};
use tl3_controller::executor::Output as Out;
use tl3_controller::{
    adapters,
    executor::{Completion, Delivery, Executor, Observation, Reconciliation, Registry, Upstream},
};
use tl3_kernel::{Ack, EffectKind, Event, Fact};

#[derive(Default, Serialize, Deserialize)]
pub struct State {
    pub epoch: i64,
    pub records: BTreeMap<String, Record>,
    pub resources: BTreeMap<String, Value>,
    pub removed_duplicates: usize,
    #[serde(default)]
    pub redeliveries: usize,
}

#[derive(Serialize, Deserialize)]
pub struct Record {
    pub delivery: Delivery,
    pub completion: Completion,
    pub applications: usize,
    pub copies: usize,
    pub probes: usize,
}

pub fn load(path: &Path) -> State {
    match fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).unwrap(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => State::default(),
        Err(e) => panic!("read fake upstream: {e}"),
    }
}

fn save(path: &Path, state: &State) {
    let temporary = path.with_extension("tmp");
    fs::write(&temporary, serde_json::to_vec(state).unwrap()).unwrap();
    fs::rename(temporary, path).unwrap();
}

struct Fake {
    inner: Box<dyn Executor>,
    path: PathBuf,
    scenario: String,
    seed: u64,
}

fn unknown() -> tl3_controller::Error {
    tl3_controller::Error::Adapter(Box::new(std::io::Error::other("simulated lost response")))
}

pub fn asynchronous(output: Out) -> bool {
    matches!(
        output,
        Out::Effect {
            kind: EffectKind::StartRun(_)
                | EffectKind::SubmitChecks
                | EffectKind::StartRebase
                | EffectKind::Enqueue,
            ..
        }
    )
}

fn converges(output: Out) -> bool {
    matches!(
        output,
        Out::Effect {
            kind: EffectKind::PrepareWorktree
                | EffectKind::RemoveWorktree
                | EffectKind::Push
                | EffectKind::OpenPr
                | EffectKind::ClosePr
                | EffectKind::Enqueue
                | EffectKind::WithdrawJob
                | EffectKind::StopRun
                | EffectKind::PostApproval,
            ..
        }
    )
}

fn desired(delivery: &Delivery) -> Option<(String, Value)> {
    let node = delivery.node().0;
    let (name, value) = match delivery.output {
        Out::Effect { kind, .. } => match kind {
            EffectKind::PrepareWorktree => ("worktree", json!(true)),
            EffectKind::RemoveWorktree => ("worktree", json!(false)),
            EffectKind::Push => ("branch", json!(delivery.head_sha)),
            EffectKind::OpenPr => ("pr", json!("open")),
            EffectKind::ClosePr => ("pr", json!("closed")),
            EffectKind::Enqueue => ("queue", json!(true)),
            EffectKind::WithdrawJob => ("job", json!("ended")),
            EffectKind::StopRun => ("run", json!("ended")),
            EffectKind::StartRun(_) | EffectKind::StartRebase => {
                ("run", json!(delivery.generation))
            }
            EffectKind::SubmitChecks => ("job", json!(delivery.key)),
            EffectKind::PostApproval => {
                return Some((
                    format!("approval:{node}:{}", delivery.head_sha.as_deref().unwrap()),
                    json!(true),
                ));
            }
        },
        Out::Notice { .. } | Out::Comment { .. } => return None,
    };
    Some((format!("{name}:{node}"), value))
}

impl Executor for Fake {
    fn execute(&mut self, delivery: &Delivery) -> tl3_controller::Result<Completion> {
        let mut state = load(&self.path);
        if delivery.epoch < state.epoch {
            return Err(unknown());
        }
        state.epoch = delivery.epoch;
        if let Some(record) = state.records.get(&delivery.key) {
            let result = record.completion.clone();
            state.redeliveries += 1;
            save(&self.path, &state);
            return Ok(result);
        }
        if self.scenario == "stop-absent"
            && state.records.is_empty()
            && matches!(
                delivery.output,
                Out::Effect {
                    kind: EffectKind::StartRun(_),
                    ..
                }
            )
        {
            return Err(unknown());
        }
        let failed = matches!(
            delivery.output,
            Out::Effect {
                kind: EffectKind::PrepareWorktree,
                ..
            }
        ) && !state.records.values().any(|r| {
            matches!(
                r.delivery.output,
                Out::Effect {
                    kind: EffectKind::PrepareWorktree,
                    ..
                }
            )
        });
        let mut completion = if failed && matches!(self.scenario.as_str(), "retry" | "permanent") {
            Completion {
                ack: if self.scenario == "retry" {
                    Ack::Retryable
                } else {
                    Ack::Permanent
                },
                observations: vec![],
            }
        } else {
            self.inner.execute(delivery)?
        };
        if let [Observation::Plan { paths, .. }] = completion.observations.as_mut_slice() {
            *paths = (0..3 + self.seed % 3)
                .map(|i| format!("src/seed_{}_file_{i}.rs", self.seed))
                .collect();
        }
        if self.scenario == "rebase"
            && matches!(
                delivery.output,
                Out::Effect {
                    kind: EffectKind::Enqueue,
                    ..
                }
            )
            && !state.records.values().any(|r| {
                matches!(
                    r.delivery.output,
                    Out::Effect {
                        kind: EffectKind::Enqueue,
                        ..
                    }
                )
            })
        {
            completion.observations = vec![Observation::Kernel(Event::Fact {
                node: delivery.node(),
                fact: Fact::Conflict,
            })];
        }
        if matches!(
            (self.scenario.as_str(), delivery.output),
            (
                "stop-run",
                Out::Effect {
                    kind: EffectKind::StartRun(tl3_kernel::Role::Make),
                    ..
                }
            ) | (
                "stop-checks",
                Out::Effect {
                    kind: EffectKind::SubmitChecks,
                    ..
                }
            ) | (
                "stop-pr",
                Out::Effect {
                    kind: EffectKind::Enqueue,
                    ..
                }
            )
        ) {
            completion.observations.clear();
        }
        let applied = completion.ack == Ack::Confirmed;
        let mut applications = usize::from(applied);
        if applied && let Some((key, value)) = desired(delivery) {
            if converges(delivery.output) && state.resources.get(&key) == Some(&value) {
                applications = 0;
            }
            state.resources.insert(key, value);
        }
        if applied
            && matches!(
                delivery.output,
                Out::Effect {
                    kind: EffectKind::RemoveWorktree,
                    ..
                }
            )
        {
            state
                .resources
                .insert(format!("branch:{}", delivery.node().0), Value::Null);
        }
        let notice = matches!(delivery.output, Out::Notice { .. });
        state.records.insert(
            delivery.key.clone(),
            Record {
                delivery: delivery.clone(),
                completion: completion.clone(),
                applications,
                copies: if notice { 2 } else { usize::from(applied) },
                probes: 0,
            },
        );
        save(&self.path, &state);
        // All messages lose their answer so their next authoritative read must
        // remove the rare duplicate. Synchronous writes also lose their answer;
        // asynchronous starts lose it only when selected by the seed.
        if notice
            || !asynchronous(delivery.output)
            || (self.scenario == "stop-sent"
                && matches!(
                    delivery.output,
                    Out::Effect {
                        kind: EffectKind::StartRun(_),
                        ..
                    }
                ))
            || (delivery.key.bytes().fold(self.seed, |a, b| {
                a.wrapping_mul(31).wrapping_add(u64::from(b))
            }) % 3
                == 0)
        {
            return Err(unknown());
        }
        if asynchronous(delivery.output) {
            completion.observations.clear();
        }
        Ok(completion)
    }

    fn reconcile(&mut self, delivery: &Delivery) -> tl3_controller::Result<Reconciliation> {
        let mut state = load(&self.path);
        let Some(record) = state.records.get_mut(&delivery.key) else {
            return Ok(Reconciliation::Absent);
        };
        record.probes += 1;
        if record.copies > 1 {
            state.removed_duplicates += record.copies - 1;
            record.copies = 1;
        }
        let completion = record.completion.clone();
        let pending = record.probes == 1 && asynchronous(delivery.output);
        save(&self.path, &state);
        Ok(if pending {
            Reconciliation::Pending
        } else {
            Reconciliation::Found(completion)
        })
    }
}

pub const UPSTREAMS: [Upstream; 5] = [
    Upstream::GitHub,
    Upstream::Mergify,
    Upstream::PrismaBuild,
    Upstream::Workers,
    Upstream::Worktrees,
];
pub fn path(root: &Path, upstream: Upstream) -> PathBuf {
    root.join(format!("{upstream:?}.json"))
}

fn fake(root: &Path, scenario: &str, seed: u64, upstream: Upstream) -> Fake {
    fs::create_dir_all(root).unwrap();
    let receipts = root.join(format!("{upstream:?}"));
    fs::create_dir_all(&receipts).unwrap();
    let inner: Box<dyn Executor> = match upstream {
        Upstream::GitHub => Box::new(adapters::github::Fake::new(&receipts)),
        Upstream::Mergify => Box::new(adapters::mergify::Fake::new(&receipts)),
        Upstream::PrismaBuild => Box::new(adapters::prismabuild::Fake::new(&receipts)),
        Upstream::Workers => Box::new(adapters::workers::Fake::new(&receipts)),
        Upstream::Worktrees => Box::new(adapters::worktrees::Fake::new(&receipts)),
    };
    Fake {
        inner,
        path: path(root, upstream),
        scenario: scenario.into(),
        seed,
    }
}

pub fn registry(root: &Path, scenario: &str, seed: u64) -> Registry {
    let mut registry = Registry::default();
    for upstream in UPSTREAMS {
        registry.register(upstream, fake(root, scenario, seed, upstream));
    }
    registry
}

pub fn verify_contracts() {
    use tl3_kernel::{EffectKind::*, NodeId, Role};
    let mut outputs: Vec<_> = [
        StartRun(Role::Ingest),
        StartRun(Role::Audit),
        StartRun(Role::Plan),
        StartRun(Role::Make),
        StartRun(Role::Review),
        SubmitChecks,
        StartRebase,
        PrepareWorktree,
        Push,
        PostApproval,
        OpenPr,
        Enqueue,
        StopRun,
        WithdrawJob,
        ClosePr,
        RemoveWorktree,
    ]
    .into_iter()
    .map(|kind| Out::Effect {
        node: NodeId(0),
        seq: 0,
        kind,
        head: None,
    })
    .collect();
    outputs.push(Out::Notice {
        node: NodeId(0),
        notice: tl3_kernel::Notice::MeasuresPosted,
    });
    for output in outputs {
        let root = tempfile::tempdir().unwrap();
        let upstream = tl3_controller::executor::upstream(output);
        let mut delivery = Delivery {
            key: "original".into(),
            epoch: 0,
            output,
            generation: 1,
            head_sha: Some(format!("{:040x}", 123)),
            repository: crate::bootstrap().repos.remove(0),
            issue: 42,
            pull_request: Some(100),
            paths: vec![],
            attempt: 1,
            round: 1,
            checks: None,
            job_generation: None,
            note: None,
            question: None,
            question_context: None,
            steering: None,
            request_text: None,
        };
        let mut executor = fake(root.path(), "merge", 1, upstream);
        let _ = executor.execute(&delivery);
        executor.execute(&delivery).unwrap();
        delivery.key = "new-epoch".into();
        delivery.epoch = 1;
        let _ = executor.execute(&delivery);
        drop(executor);
        let mut executor = fake(root.path(), "merge", 1, upstream);
        delivery.key = "stale-refused".into();
        delivery.epoch = 0;
        assert!(
            executor.execute(&delivery).is_err(),
            "{output:?}: stale write accepted after restart"
        );
        delivery.key = "original".into();
        let _ = executor.reconcile(&delivery).unwrap();
        assert!(
            matches!(
                executor.reconcile(&delivery).unwrap(),
                Reconciliation::Found(_)
            ),
            "{output:?}: old record unreadable"
        );
        let state = load(&path(root.path(), upstream));
        assert!(!state.records.contains_key("stale-refused"));
        assert_eq!(state.records["original"].applications, 1);
        assert_eq!(state.records["original"].copies, 1);
        if converges(output) {
            assert_eq!(
                state.records["new-epoch"].applications, 0,
                "{output:?}: desired state applied twice"
            );
        }
    }
    println!("17 upstream delivery contracts and restart epoch fences passed");
}
