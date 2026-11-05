use super::*;
use std::{
    cell::RefCell,
    collections::BTreeMap,
    panic::{AssertUnwindSafe, catch_unwind},
};
use tl3_kernel::{Node, Phase, RepoId};

pub(super) const SHA: &str = "1111111111111111111111111111111111111111";
pub(super) const OTHER: &str = "2222222222222222222222222222222222222222";
pub(super) type World = tl3_kernel::World;
#[derive(Clone, Debug)]
pub(super) struct Step {
    pub conflict: bool,
    pub present: bool,
    pub phase: u8,
    pub current: u64,
    pub observed: u64,
    pub node: u64,
    pub sha: String,
    pub files: Vec<String>,
}
impl Default for Step {
    fn default() -> Self {
        Self {
            conflict: true,
            present: true,
            phase: 0,
            current: 7,
            observed: 7,
            node: 0,
            sha: SHA.into(),
            files: vec!["src/one.rs".into(), "tests/two.rs".into()],
        }
    }
}
#[derive(Clone, Debug)]
pub(super) struct Schedule {
    pub steps: Vec<Step>,
    pub interned: BTreeMap<(String, String), u64>,
    pub initial_rows: BTreeMap<String, Value>,
    pub failures: Vec<bool>,
    pub select_failures: Vec<bool>,
}
impl Default for Schedule {
    fn default() -> Self {
        Self {
            steps: vec![Step::default()],
            interned: BTreeMap::new(),
            initial_rows: BTreeMap::from([("0".into(), json!({"head":OTHER,"files":["old.rs"]}))]),
            failures: vec![],
            select_failures: vec![],
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Run {
    pub calls: Vec<Value>,
    pub results: Vec<Value>,
    pub staged_rows: Vec<Value>,
    pub persisted_rows: BTreeMap<String, Value>,
    pub pending_intern: BTreeMap<(String, String), u64>,
    pub persisted_intern: BTreeMap<(String, String), u64>,
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
    cut: Option<Cut>,
    failures: Vec<bool>,
    select_failures: Vec<bool>,
    select_reads: usize,
}
thread_local! { static ACTIVE: RefCell<Option<Recording>> = const { RefCell::new(None) }; }
fn with<T>(f: impl FnOnce(&mut Recording) -> T) -> T {
    ACTIVE.with(|r| f(r.borrow_mut().as_mut().expect("recorded rebase scope")))
}
fn call(op: &str, args: Value) {
    with(|r| r.run.calls.push(json!({"op":op,"args":args})));
}
fn begin_write(op: &str, args: Value) -> (usize, bool) {
    with(|r| {
        let index = r.run.writes;
        r.run.writes += 1;
        r.run.calls.push(json!({"op":op,"args":args}));
        if r.cut.is_some_and(|c| c.write == index && !c.after) {
            std::panic::panic_any(Crash);
        }
        (index, r.failures.get(index).copied().unwrap_or(false))
    })
}
fn end_write(index: usize) {
    with(|r| {
        if r.cut.is_some_and(|c| c.write == index && c.after) {
            std::panic::panic_any(Crash);
        }
    });
}
pub(super) struct Controller {
    pub(super) store: Store,
}
pub(super) struct Store;
pub(super) struct InternId(i64);
impl InternId {
    pub(super) fn get(&self) -> i64 {
        self.0
    }
}
impl Store {
    pub(super) fn intern_commit(&mut self, scope: &str, sha: &str) -> Result<InternId> {
        let failed = with(|r| {
            let failed = r
                .select_failures
                .get(r.select_reads)
                .copied()
                .unwrap_or(false);
            r.select_reads += 1;
            failed
        });
        if failed {
            call("intern.select.error", json!({"scope":scope,"sha":sha}));
            return Err(Error::Store(tl3_store::Error::Invalid(
                "injected intern select failure",
            )));
        }
        let key = (scope.to_owned(), sha.to_owned());
        let existing = with(|r| r.run.pending_intern.get(&key).copied());
        call(
            "intern.select",
            json!({"scope":scope,"sha":sha,"result":existing}),
        );
        if let Some(id) = existing {
            return Ok(InternId(id as i64));
        }
        let (index, failed) = begin_write("intern.insert", json!({"scope":scope,"sha":sha}));
        let result = if failed {
            None
        } else {
            Some(with(|r| {
                let id = r.run.pending_intern.values().copied().max().unwrap_or(0) + 1;
                r.run.pending_intern.insert(key, id);
                id
            }))
        };
        call("intern.result", json!(result));
        end_write(index);
        result
            .map(|id| InternId(id as i64))
            .ok_or(Error::Store(tl3_store::Error::Invalid(
                "injected intern failure",
            )))
    }
}
pub(super) struct Commit {
    pub(super) rows: Rows,
}
pub(super) struct Rows;
impl Rows {
    pub(super) fn push(&mut self, row: RowChange) {
        let value = match row {
            RowChange::Put { table, id, value } => {
                json!({"put":{"table":table,"id":id,"value":value}})
            }
            RowChange::Delete { table, id } => json!({"delete":{"table":table,"id":id}}),
        };
        call("row.push", value.clone());
        with(|r| r.run.staged_rows.push(value));
    }
}
pub(super) fn put(table: &str, id: String, value: Value) -> RowChange {
    RowChange::Put {
        table: table.into(),
        id,
        value,
    }
}
fn persist() -> Result<()> {
    let (index, failed) = begin_write("transaction.commit", Value::Null);
    if !failed {
        with(|r| {
            r.run.persisted_intern = r.run.pending_intern.clone();
            for row in &r.run.staged_rows {
                if let Some(put) = row.get("put") {
                    r.run
                        .persisted_rows
                        .insert(put["id"].as_str().unwrap().into(), put["value"].clone());
                } else {
                    r.run
                        .persisted_rows
                        .remove(row["delete"]["id"].as_str().unwrap());
                }
            }
        });
    }
    end_write(index);
    if failed {
        Err(Error::Store(tl3_store::Error::Invalid(
            "injected commit failure",
        )))
    } else {
        Ok(())
    }
}
fn run(s: &Schedule, new: bool, cut: Option<Cut>) -> Run {
    static HOOK: std::sync::Once = std::sync::Once::new();
    HOOK.call_once(|| {
        let old = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            if !info.payload().is::<Crash>() {
                old(info);
            }
        }));
    });
    ACTIVE.with(|r| {
        *r.borrow_mut() = Some(Recording {
            run: Run {
                calls: vec![],
                results: vec![],
                staged_rows: vec![],
                persisted_rows: s.initial_rows.clone(),
                pending_intern: s.interned.clone(),
                persisted_intern: s.interned.clone(),
                writes: 0,
            },
            failures: s.failures.clone(),
            select_failures: s.select_failures.clone(),
            select_reads: 0,
            cut,
        })
    });
    let result = catch_unwind(AssertUnwindSafe(|| {
        let mut controller = Controller { store: Store };
        for step in &s.steps {
            // A new input transaction sees the preceding transaction's intern dictionary.
            with(|r| {
                r.run.pending_intern = r.run.persisted_intern.clone();
                r.run.staged_rows.clear();
            });
            let mut world = World::new(&[], 1);
            if step.present {
                let mut node = Node::new(RepoId(0));
                node.phase = match step.phase % 4 {
                    0 => Phase::Rebasing,
                    1 => Phase::Making,
                    2 => Phase::Checking,
                    _ => Phase::Closed(tl3_kernel::Outcome::Stopped),
                };
                node.generation = step.current;
                world.nodes.push(node);
            }
            let observation = if step.conflict {
                Observation::RebaseConflict {
                    node: NodeId(step.node),
                    generation: step.observed,
                    head: step.sha.clone(),
                    target: OTHER.into(),
                    files: step.files.clone(),
                }
            } else {
                Observation::Rebased {
                    node: NodeId(step.node),
                    generation: step.observed,
                    sha: step.sha.clone(),
                }
            };
            let mut commit = Commit { rows: Rows };
            let translate = match (new, step.conflict) {
                (false, true) => conflict_reference::translate,
                (true, true) => conflict_current::translate,
                (false, false) => rebased_reference::translate,
                (true, false) => rebased_current::translate,
            };
            match translate(&mut controller, &world, observation, &mut commit) {
                Ok(event) => {
                    call("translation.result", json!(event));
                    match persist() {
                        Ok(()) => with(|r| r.run.results.push(json!({"event":event}))),
                        Err(error) => {
                            with(|r| r.run.results.push(json!({"error":error.to_string()})));
                            break;
                        }
                    }
                }
                Err(error) => {
                    with(|r| r.run.results.push(json!({"error":error.to_string()})));
                    break;
                }
            }
        }
    }));
    match result {
        Ok(()) => (),
        Err(error) if error.is::<Crash>() => with(|r| r.run.results.push(json!({"crash":true}))),
        Err(error) => std::panic::resume_unwind(error),
    }
    ACTIVE.with(|r| r.borrow_mut().take().unwrap().run)
}
pub(super) fn compare(s: &Schedule) -> Run {
    let old = run(s, false, None);
    assert_eq!(old, run(s, true, None), "rebase schedule {s:?}");
    for write in 0..old.writes {
        for after in [false, true] {
            let cut = Some(Cut { write, after });
            let old_cut = run(s, false, cut);
            assert_eq!(
                old_cut,
                run(s, true, cut),
                "rebase crash {cut:?}, schedule {s:?}"
            );
            assert_eq!(old_cut.results.last().unwrap(), &json!({"crash":true}));
        }
    }
    old
}
