//! #817 / extraction equivalence: changing reads, clock samples and commit loss.
use super::*;
use proptest::prelude::*;
use std::cell::RefCell;
use tl3_kernel::{Event, Priority, RepoId};
use tl3_store::{Effect, Epoch, Revision, WorldRow};

struct Reference<I> {
    store: I,
}
impl<I: RecoveryIo> Reference<I> {
    fn allowance_row(&self, delivery: Delivery) -> RowChange {
        self.store.allowance_row(delivery)
    }
    #[allow(clippy::too_many_arguments)]
    fn input_failed(
        &self,
        snapshot: &Snapshot,
        scope: &str,
        target: Option<IssueTarget>,
        title: &str,
        node: Option<NodeId>,
        upstream: crate::executor::Upstream,
        error: Error,
        commit: &mut Commit,
    ) -> Result<()> {
        self.store.input_failed(
            snapshot, scope, target, title, node, upstream, error, commit,
        )
    }
}
include!("../tests/fixtures/dispatch_recovery_original.rs");

#[derive(Clone, Default, Debug, PartialEq, Eq)]
struct Trace {
    calls: Vec<String>,
    state: BTreeMap<(String, String), Vec<u8>>,
    effects: Vec<String>,
    clock: usize,
}

struct Recorded {
    reads: Vec<Option<Effect>>,
    clocks: Vec<u64>,
    crash: u8,
    trace: RefCell<Trace>,
}
impl RecoveryIo for Recorded {
    fn effect(&mut self, key: &str) -> Result<Option<Effect>> {
        let mut trace = self.trace.borrow_mut();
        trace.calls.push(format!("effect:{key}"));
        Ok(self.reads.remove(0))
    }
    fn allowance_row(&self, delivery: Delivery) -> RowChange {
        let mut trace = self.trace.borrow_mut();
        trace.calls.push("clock".into());
        let now = self.clocks[trace.clock];
        trace.clock += 1;
        put(
            "allowances",
            delivery.key.clone(),
            json!(crate::looks::AllowanceState::new(delivery, now, 5000)),
        )
    }
    fn input_failed(
        &self,
        _: &Snapshot,
        scope: &str,
        target: Option<IssueTarget>,
        title: &str,
        node: Option<NodeId>,
        upstream: crate::executor::Upstream,
        error: Error,
        commit: &mut Commit,
    ) -> Result<()> {
        self.trace
            .borrow_mut()
            .calls
            .push(format!("incident:{scope}:{upstream:?}:{error:?}"));
        commit.rows.push(put(
            "incidents",
            scope.into(),
            json!({
                "node": node, "target": target.map(|t| t.issue), "title": title,
                "open": true, "signature":{"class":MISSING}
            }),
        ));
        Ok(())
    }
    fn commit(&mut self, commit: &Commit) -> Result<()> {
        let mut trace = self.trace.borrow_mut();
        trace.calls.push(format!("commit:{commit:?}"));
        // A store commit is atomic: loss before landing, or response loss after
        // landing. No schedule pretends individual transaction rows can land.
        if self.crash == 0 {
            return Err(Error::Invalid("injected precommit loss"));
        }
        for row in &commit.rows {
            match row {
                RowChange::Put { table, id, value } => {
                    trace.state.insert(
                        (table.clone(), id.clone()),
                        serde_json::to_vec(value).unwrap(),
                    );
                }
                RowChange::Delete { table, id } => {
                    trace.state.remove(&(table.clone(), id.clone()));
                }
            }
        }
        trace
            .effects
            .extend(commit.effects.iter().map(|e| format!("{e:?}")));
        if self.crash == 1 {
            return Err(Error::Invalid("injected lost commit response"));
        }
        Ok(())
    }
}

fn fixture() -> (Snapshot, World, Vec<Effect>) {
    let mut world = World::new(&[2], 3);
    for _ in 0..3 {
        world = tl3_kernel::step(
            world,
            Event::NewRequest {
                repo: RepoId(0),
                priority: Priority::P1,
            },
        )
        .0;
    }
    let mut rows = Vec::new();
    let mut effects = Vec::new();
    for (i, node) in world.nodes.iter_mut().enumerate() {
        node.effect = None;
        let delivery: Delivery = serde_json::from_value(json!({
            "key":format!("effect:{i}:0"),"epoch":0,"generation":node.generation,
            "output":{"Effect":{"node":i,"seq":0,"kind":{"StartRun":"Ingest"},"head":null}},
            "repository":{"owner":"owner","name":"repo","installation":1},
            "issue":i+1,"paths":[],"attempt":0,"round":0
        }))
        .unwrap();
        effects.push(Effect {
            key: delivery.key.clone(),
            epoch: Epoch::ZERO,
            kind: "workers".into(),
            payload: json!(delivery),
            state: EffectState::Confirmed,
            created_revision: Revision::ZERO,
            updated_revision: Revision::ZERO,
        });
        rows.push(WorldRow {
            version: Revision::ZERO,
            arrival: 0,
            table: "requests".into(),
            id: i.to_string(),
            value: json!({
                "repo":delivery.repository,"issue":i+1,"issue_id":i+1,"title":"fixture"
            }),
        });
        rows.push(WorldRow {
            version: Revision::ZERO,
            arrival: 0,
            table: QUEUED.into(),
            id: i.to_string(),
            value: json!(true),
        });
    }
    (
        Snapshot {
            revision: Revision::ZERO,
            epoch: Epoch::ZERO,
            rows,
            pending_effects: vec![],
            credit: vec![],
            interned: vec![],
        },
        world,
        effects,
    )
}

/// #816 / #817: every old/new I/O call, effect and persisted byte agrees,
/// including when later reads and clocks differ and commit replies are lost.
#[test]
fn extraction_preserves_io_schedules_and_crash_state() {
    use proptest::test_runner::{Config, FileFailurePersistence, RngSeed, TestRunner};
    let mut runner = TestRunner::new(Config {
        cases: 256,
        rng_seed: RngSeed::Fixed(817_816),
        failure_persistence: Some(Box::new(FileFailurePersistence::Direct(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/dispatch-recovery.proptest-regressions"
        )))),
        ..Config::default()
    });
    runner
        .run(
            &(
                prop::array::uniform3(0u8..8),
                prop::array::uniform3(any::<u64>()),
                0u8..3,
            ),
            |(modes, clocks, crash)| {
                let (mut snapshot, mut world, effects) = fixture();
                let mut reads = vec![];
                for (i, mode) in modes.into_iter().enumerate() {
                    if mode == 0 {
                        world.nodes[i].run = None;
                    }
                    if mode == 1 || mode == 2 {
                        snapshot.rows.push(WorldRow {
                            version: Revision::ZERO,
                            arrival: 0,
                            table: "watches".into(),
                            id: effects[i].key.clone(),
                            value: effects[i].payload.clone(),
                        });
                    }
                    if mode == 1 || mode == 3 {
                        snapshot.rows.push(WorldRow {
                            version: Revision::ZERO,
                            arrival: 0,
                            table: "allowances".into(),
                            id: effects[i].key.clone(),
                            value: json!({}),
                        });
                    }
                    if mode == 0 || mode == 1 {
                        continue;
                    }
                    let mut effect = effects[i].clone();
                    if mode == 5 {
                        effect.payload["generation"] = json!(999);
                    }
                    if mode == 6 {
                        effect.state = EffectState::Cancelled;
                    }
                    reads.push(if mode == 7 { None } else { Some(effect) });
                }
                let initial: BTreeMap<_, _> = snapshot
                    .rows
                    .iter()
                    .map(|r| {
                        (
                            (r.table.clone(), r.id.clone()),
                            serde_json::to_vec(&r.value).unwrap(),
                        )
                    })
                    .collect();
                let io = || Recorded {
                    reads: reads.clone(),
                    clocks: clocks.to_vec(),
                    crash,
                    trace: RefCell::new(Trace {
                        state: initial.clone(),
                        ..Trace::default()
                    }),
                };
                let mut old = Reference { store: io() };
                let mut new = Recovery { store: io() };
                let ids = [NodeId(0), NodeId(1), NodeId(2)];
                let a = old.reconcile_worker_waits_in(&snapshot, &world, &ids, &mut |_, _| {});
                let b = new.reconcile_worker_waits_in(&snapshot, &world, &ids, &mut |_, _| {});
                prop_assert_eq!(format!("{a:?}"), format!("{b:?}"));
                prop_assert_eq!(old.store.trace.into_inner(), new.store.trace.into_inner());
                Ok(())
            },
        )
        .unwrap();
}
