use super::*;
use std::{
    cell::RefCell,
    collections::BTreeMap,
    panic::{AssertUnwindSafe, catch_unwind},
};
#[derive(Clone, Debug)]
pub(super) struct Schedule {
    pub repos: Vec<Repository>,
    pub observations: Vec<Observation>,
    pub rows: Vec<WorldRow>,
    pub reads: Vec<u8>,
    pub commit_failures: Vec<bool>,
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct Run {
    pub calls: Vec<Value>,
    pub staged: Vec<Value>,
    pub bytes: BTreeMap<String, Vec<u8>>,
    pub results: Vec<Value>,
    pub writes: usize,
    pub reads: usize,
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
    tape: Vec<u8>,
    failures: Vec<bool>,
    commits: usize,
    cut: Option<Cut>,
}
thread_local! {static ACTIVE:RefCell<Option<Recording>>=const {RefCell::new(None)};}
fn with<T>(f: impl FnOnce(&mut Recording) -> T) -> T {
    ACTIVE.with(|r| f(r.borrow_mut().as_mut().unwrap()))
}
fn call(op: &str, args: Value) {
    with(|r| r.run.calls.push(json!({"op":op,"args":args})));
}
fn write(op: &str, args: Value, apply: impl FnOnce(&mut Recording)) {
    with(|r| {
        let index = r.run.writes;
        r.run.writes += 1;
        r.run.calls.push(json!({"op":op,"args":args}));
        if r.cut.is_some_and(|c| c.write == index && !c.after) {
            std::panic::panic_any(Crash);
        }
        apply(r);
        if r.cut.is_some_and(|c| c.write == index && c.after) {
            std::panic::panic_any(Crash);
        }
    });
}
pub(super) struct Controller {
    pub bootstrap: Bootstrap,
    pub store: Store,
}
pub(super) struct Store;
impl Store {
    fn comment_retired(&mut self, id: &str) -> Result<bool> {
        let reply = with(|r| {
            let v = r.tape.get(r.run.reads).copied().unwrap_or(0) % 4;
            r.run.reads += 1;
            v
        });
        call("store.comment_retired", json!({"id":id,"reply":reply}));
        match reply {
            0 => Ok(false),
            1 => Ok(true),
            2 => Err(Error::Store(tl3_store::Error::Invalid("query failed"))),
            _ => Err(Error::Store(tl3_store::Error::Invalid("connection lost"))),
        }
    }
}
pub(super) struct Commit {
    pub rows: Rows,
}
pub(super) struct Rows;
impl Rows {
    pub fn push(&mut self, row: RowChange) {
        let row = match row {
            RowChange::Put { table, id, value } => json!({"table":table,"id":id,"value":value}),
            RowChange::Delete { .. } => unreachable!(),
        };
        write("rows.push", row.clone(), |r| r.run.staged.push(row));
    }
}
pub(super) fn put(table: &str, id: String, value: Value) -> RowChange {
    RowChange::Put {
        table: table.into(),
        id,
        value,
    }
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
pub(super) fn observation(
    author: u64,
    repository: Repository,
    id: u64,
    body: String,
) -> Observation {
    Observation::GitHubComment {
        repository,
        issue: 73,
        comment: Comment {
            id,
            body,
            user: Actor {
                id: author,
                login: "synthetic".into(),
            },
            created_at: "2026-11-05T00:00:00Z".parse().unwrap(),
            updated_at: "2026-11-05T00:00:00Z".parse().unwrap(),
            issue_url: "https://github.com/Owner/Repo/issues/73".into(),
        },
    }
}
pub(super) fn repo() -> Repository {
    Repository {
        owner: "Owner".into(),
        name: "Repo".into(),
        installation: 3,
    }
}
pub(super) fn stable() -> Schedule {
    Schedule {
        repos: vec![repo()],
        observations: vec![observation(42, repo(), 7, "Please explain".into())],
        rows: vec![],
        reads: vec![0],
        commit_failures: vec![],
    }
}
fn run(s: &Schedule, new: bool, cut: Option<Cut>) -> Run {
    ACTIVE.with(|r| {
        *r.borrow_mut() = Some(Recording {
            run: Run::default(),
            tape: s.reads.clone(),
            failures: s.commit_failures.clone(),
            commits: 0,
            cut,
        })
    });
    let result = catch_unwind(AssertUnwindSafe(|| {
        let mut controller = Controller {
            bootstrap: Bootstrap {
                repos: s.repos.clone(),
                rob: 42,
                operations: None,
                slots: 1,
                pool: 1,
                fail_fast: false,
            },
            store: Store,
        };
        let mut snapshot = Snapshot {
            revision: Revision::ZERO,
            epoch: Epoch::ZERO,
            rows: s.rows.clone(),
            pending_effects: vec![],
            credit: vec![],
            interned: vec![],
        };
        for observation in &s.observations {
            with(|r| r.run.staged.clear());
            let mut commit = Commit { rows: Rows };
            let result = if new {
                // Red: the new implementation entry is deliberately absent. The complete
                // current arm still compiles and its identity guard remains active.
                let _ = (&mut controller, &snapshot, observation, &mut commit);
                Ok::<Option<Event>, Error>(None)
            } else {
                reference::translate(&mut controller, &snapshot, observation.clone(), &mut commit)
            };
            match result {
                Err(e) => with(|r| r.run.results.push(json!({"error":e.to_string()}))),
                Ok(event) => {
                    let failed = with(|r| {
                        let v = r.failures.get(r.commits).copied().unwrap_or(false);
                        r.commits += 1;
                        v
                    });
                    // The arm buffers rows. This explicit test boundary models atomic outer
                    // commit/rollback, not the production transaction writer or translate tail.
                    write("harness.commit", json!({"failed":failed}), |r| {
                        if !failed {
                            for row in &r.run.staged {
                                r.run.bytes.insert(
                                    format!(
                                        "{}/{}",
                                        row["table"].as_str().unwrap(),
                                        row["id"].as_str().unwrap()
                                    ),
                                    serde_json::to_vec(&row["value"]).unwrap(),
                                );
                            }
                        }
                    });
                    if failed {
                        with(|r| r.run.results.push(json!({"error":"commit failed"})));
                    } else {
                        for change in with(|r| r.run.staged.clone()) {
                            let table = change["table"].as_str().unwrap();
                            let id = change["id"].as_str().unwrap();
                            snapshot.rows.retain(|r| r.table != table || r.id != id);
                            snapshot.rows.push(row(table, id, change["value"].clone()));
                        }
                        with(|r| r.run.results.push(json!({"event":event})));
                    }
                }
            }
        }
    }));
    match result {
        Ok(()) => {}
        Err(e) if e.is::<Crash>() => with(|r| r.run.results.push(json!({"crash":true}))),
        Err(e) => std::panic::resume_unwind(e),
    }
    ACTIVE.with(|r| r.borrow_mut().take().unwrap().run)
}
pub(super) fn compare(s: &Schedule) -> (Run, usize) {
    let old = run(s, false, None);
    let new = run(s, true, None);
    assert_eq!(old, new, "schedule {s:?}");
    for write in 0..old.writes {
        for after in [false, true] {
            let cut = Some(Cut { write, after });
            assert_eq!(
                run(s, false, cut),
                run(s, true, cut),
                "cut {cut:?}, schedule {s:?}"
            );
        }
    }
    let count = 1 + 2 * old.writes;
    (old, count)
}
