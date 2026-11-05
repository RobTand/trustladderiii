use super::*;
use std::{
    cell::RefCell,
    panic::{AssertUnwindSafe, catch_unwind},
};
use tl3_controller::adapters::prismabuild::CheckRun;
use tl3_kernel::{Node, Phase, RepoId};
use tl3_store::{Epoch, Interned, Revision, WorldRow};

pub(super) const OLD_HEAD: &str = "1111111111111111111111111111111111111111";
pub(super) const NEW_HEAD: &str = "2222222222222222222222222222222222222222";
#[derive(Clone, Debug)]
pub(super) struct Schedule {
    pub fact: QueueFact,
    pub snapshot: Snapshot,
    pub phase: u8,
    pub merged: bool,
    pub enqueue: bool,
    pub cancelling: bool,
    pub current: Option<u64>,
    pub approved: Option<u64>,
    /// Each call consumes its own response, including decode/read errors.
    pub plans: Vec<std::result::Result<Option<CheckRun>, &'static str>>,
    pub intern_failures: Vec<bool>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Run {
    pub calls: Vec<Value>,
    pub rows: Vec<Value>,
    pub effects: Vec<Value>,
    pub result: Value,
    pub pending_intern: BTreeMap<(String, String), u64>,
    pub persisted_intern: BTreeMap<(String, String), u64>,
    pub persisted_rows: Vec<Value>,
    pub writes: usize,
}
#[derive(Clone, Copy, Debug)]
struct Cut {
    write: usize,
    after: bool,
}
#[derive(Debug)]
struct Crash;
struct Recording {
    run: Run,
    plans: Vec<std::result::Result<Option<CheckRun>, &'static str>>,
    reads: usize,
    failures: Vec<bool>,
    cut: Option<Cut>,
    next_id: u64,
}
thread_local! { static ACTIVE: RefCell<Option<Recording>> = const { RefCell::new(None) }; }
fn with<T>(f: impl FnOnce(&mut Recording) -> T) -> T {
    ACTIVE.with(|r| f(r.borrow_mut().as_mut().expect("recorded Queue scope")))
}
fn call(op: &str, args: Value) {
    with(|r| r.run.calls.push(json!({"op":op,"args":args})));
}
pub(super) struct Controller {
    store: Store,
}
struct Store;
impl Store {
    fn intern_commit(&mut self, scope: &str, sha: &str) -> Result<u64> {
        let existing = with(|r| {
            r.run
                .pending_intern
                .get(&(scope.into(), sha.into()))
                .copied()
        });
        call(
            "intern.select",
            json!({"scope":scope,"sha":sha,"result":existing}),
        );
        if let Some(id) = existing {
            return Ok(id);
        }
        with(|r| {
            let write = r.run.writes;
            r.run.writes += 1;
            r.run
                .calls
                .push(json!({"op":"intern.insert","scope":scope,"sha":sha}));
            if r.cut.is_some_and(|c| c.write == write && !c.after) {
                std::panic::panic_any(Crash);
            }
            let failed = r.failures.get(write).copied().unwrap_or(false);
            let id = if failed {
                None
            } else {
                let id = r.next_id;
                r.next_id += 1;
                r.run.pending_intern.insert((scope.into(), sha.into()), id);
                Some(id)
            };
            r.run.calls.push(json!({"op":"intern.result","id":id}));
            if r.cut.is_some_and(|c| c.write == write && c.after) {
                std::panic::panic_any(Crash);
            }
            id.ok_or(Error::Store(tl3_store::Error::Invalid(
                "injected intern failure",
            )))
        })
    }
}
pub(super) type World = tl3_kernel::World;
pub(super) struct Commit {
    pub rows: Rows,
}
pub(super) struct Rows;
impl Rows {
    pub fn push(&mut self, row: Value) {
        call("row.push", row.clone());
        with(|r| r.run.rows.push(row));
    }
}
pub(super) fn put(table: &str, id: String, value: Value) -> Value {
    json!({"table":table,"id":id,"value":value})
}
pub(super) fn check_run(_: &Snapshot, node: NodeId, sha: Option<&str>) -> Result<Option<CheckRun>> {
    let plan = with(|r| {
        let plan = r.plans.get(r.reads).cloned().unwrap_or(Ok(None));
        r.reads += 1;
        plan
    });
    call("check_run", json!({"node":node,"sha":sha,"result":plan}));
    plan.map_err(|_| {
        Error::StoredJson(serde_json::from_value::<CheckRun>(Value::Null).unwrap_err())
    })
}
impl Controller {
    pub(super) fn head(&mut self, node: NodeId, sha: &str) -> Result<HeadId> {
        tl3_worktrees::CommitId::new(sha)?;
        Ok(HeadId(self.store.intern_commit(&node.0.to_string(), sha)?))
    }
    // Unchanged looks helpers above this boundary compile verbatim. Group D owns
    // their persistence/notice policy; this boundary records their full arguments.
    pub(super) fn setup_incident(
        &self,
        snapshot: &Snapshot,
        commit: &mut Commit,
        node: NodeId,
        id: String,
        summary: String,
    ) -> Result<()> {
        call(
            "setup_incident",
            json!({"node":node,"id":id,"summary":summary}),
        );
        if snapshot
            .rows
            .iter()
            .any(|row| row.table == "incidents" && row.id == id && row.value["open"] == true)
        {
            return Ok(());
        }
        let value = incident(&id, true, &summary);
        commit.rows.push(put("incidents", id.clone(), value));
        let effect = json!({"incident":id,"notice":"Incident","needs_rob":false});
        call("effect.push", effect.clone());
        with(|r| r.run.effects.push(effect));
        Ok(())
    }
}
pub(super) fn incident(id: &str, open: bool, summary: &str) -> Value {
    json!({"id":id,"node":0,"open":open,"needs_rob":false,"blocking":false,
        "summary":summary,"signature":{"upstream":"Mergify","class":"setup"},
        "effect":null,"post_key":null,"cause":"upstream","input_failure":null})
}
pub(super) fn row(table: &str, id: &str, value: Value) -> WorldRow {
    WorldRow {
        table: table.into(),
        id: id.into(),
        value,
        version: Revision::ZERO,
        arrival: 0,
    }
}
pub(super) fn repository() -> Repository {
    Repository {
        owner: "RobTand".into(),
        name: "trustladderiii".into(),
        installation: 3,
    }
}
pub(super) fn stable(evidence: Evidence) -> Schedule {
    let repository = repository();
    Schedule {
        fact: QueueFact {
            repository: repository.clone(),
            number: 100,
            evidence,
        },
        snapshot: Snapshot {
            revision: Revision::ZERO,
            epoch: Epoch::ZERO,
            rows: vec![
                row(
                    "requests",
                    "0",
                    json!({"repo":repository,"issue":1,"issue_id":9,"title":"Queue translation fixture"}),
                ),
                row("pull_requests", "0", json!({"number":100})),
            ],
            pending_effects: vec![],
            credit: vec![],
            interned: vec![Interned {
                id: 1,
                kind: InternKind::Commit,
                scope: "0".into(),
                value: OLD_HEAD.into(),
            }],
        },
        phase: 0,
        merged: false,
        enqueue: true,
        cancelling: false,
        current: Some(1),
        approved: Some(1),
        plans: vec![],
        intern_failures: vec![],
    }
}
pub(super) fn plan(marker: &str) -> CheckRun {
    serde_json::from_value(json!({"repository":format!("repo-{marker}"),"base_sha":OLD_HEAD,"plan":[],"selected_failures":[marker]})).unwrap()
}
fn run(s: &Schedule, new: bool, cut: Option<Cut>) -> Run {
    static HOOK: std::sync::Once = std::sync::Once::new();
    HOOK.call_once(|| {
        let old = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            if !info.payload().is::<Crash>() {
                old(info)
            }
        }));
    });
    let initial: BTreeMap<_, _> = s
        .snapshot
        .interned
        .iter()
        .filter(|i| i.kind == InternKind::Commit)
        .map(|i| ((i.scope.clone(), i.value.clone()), i.id as u64))
        .collect();
    let next_id = initial.values().copied().max().unwrap_or(0) + 1;
    ACTIVE.with(|r| {
        *r.borrow_mut() = Some(Recording {
            run: Run {
                calls: vec![],
                rows: vec![],
                effects: vec![],
                result: Value::Null,
                pending_intern: initial.clone(),
                persisted_intern: initial,
                persisted_rows: vec![],
                writes: 0,
            },
            plans: s.plans.clone(),
            reads: 0,
            failures: s.intern_failures.clone(),
            cut,
            next_id,
        })
    });
    let result = catch_unwind(AssertUnwindSafe(|| {
        let mut node = Node::new(RepoId(0));
        node.phase = match s.phase % 4 {
            0 => Phase::Publishing,
            1 => Phase::Queued,
            2 => Phase::Closed(tl3_kernel::Outcome::Stopped),
            _ => Phase::Ingesting,
        };
        node.head = s.current.map(HeadId);
        node.approved = s.approved.map(HeadId);
        if s.merged {
            node.merged = Some(HeadId(1));
        }
        if s.enqueue {
            node.effect = Some(tl3_kernel::InFlight {
                seq: 0,
                kind: tl3_kernel::EffectKind::Enqueue,
                cancelling: s.cancelling,
            });
        }
        let mut world = World::new(&[], 1);
        world.nodes.push(node);
        let mut controller = Controller { store: Store };
        let mut commit = Commit { rows: Rows };
        let result = if new {
            current::translate(
                &mut controller,
                &s.snapshot,
                &world,
                Observation::Queue(s.fact.clone()),
                &mut commit,
            )
        } else {
            reference::translate(
                &mut controller,
                &s.snapshot,
                &world,
                Observation::Queue(s.fact.clone()),
                &mut commit,
            )
        };
        match result {
            Ok(event) => {
                call("transaction.commit", json!(event));
                with(|r| {
                    r.run.persisted_intern = r.run.pending_intern.clone();
                    r.run.persisted_rows = r.run.rows.clone();
                });
                json!({"event":event})
            }
            Err(error) => json!({"error":error.to_string()}),
        }
    }));
    let result = match result {
        Ok(v) => v,
        Err(error) if error.is::<Crash>() => json!({"crash":true}),
        Err(error) => std::panic::resume_unwind(error),
    };
    ACTIVE.with(|r| {
        let mut recording = r.borrow_mut().take().unwrap();
        recording.run.result = result;
        recording.run
    })
}
pub(super) fn compare(s: &Schedule) -> Run {
    let old = run(s, false, None);
    let new = run(s, true, None);
    assert_eq!(old, new, "Queue I/O schedule {s:?}");
    for write in 0..old.writes {
        for after in [false, true] {
            let cut = Some(Cut { write, after });
            assert_eq!(
                run(s, false, cut),
                run(s, true, cut),
                "Queue crash {cut:?}, schedule {s:?}"
            );
        }
    }
    old
}

pub(super) fn clear_stale(s: &Schedule) {
    // Run establishes the shared recorded boundary for this unchanged helper.
    let _ = run(s, false, None);
    ACTIVE.with(|r| {
        *r.borrow_mut() = Some(Recording {
            run: Run {
                calls: vec![],
                rows: vec![],
                effects: vec![],
                result: Value::Null,
                pending_intern: BTreeMap::new(),
                persisted_intern: BTreeMap::new(),
                persisted_rows: vec![],
                writes: 0,
            },
            plans: vec![],
            reads: 0,
            failures: vec![],
            cut: None,
            next_id: 1,
        })
    });
    let controller = Controller { store: Store };
    let mut world = World::new(&[], 1);
    let mut node = Node::new(RepoId(0));
    node.phase = Phase::Queued;
    node.head = Some(HeadId(2));
    node.approved = Some(HeadId(2));
    world.nodes.push(node);
    let mut commit = Commit { rows: Rows };
    controller
        .resolve_stale_missing_checks(&s.snapshot, &world, &mut commit)
        .unwrap();
    let run = ACTIVE.with(|r| r.borrow_mut().take().unwrap().run);
    assert!(
        run.rows
            .iter()
            .any(|r| r["table"] == "incidents" && r["value"]["open"] == false)
    );
    assert_eq!(run.writes, 0);
}
