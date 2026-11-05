use super::*;
use std::{
    cell::RefCell,
    panic::{AssertUnwindSafe, catch_unwind},
};

// Test-only red switch. The production arms and both compiled copies stay pinned.
const CURRENT_STUB: bool = true;

#[derive(Clone, Debug)]
pub(super) struct Schedule {
    pub rows: Vec<(String, String, Value)>,
    pub observations: Vec<Observation>,
    pub clocks: Vec<u64>,
    pub write_results: Vec<u8>,
}
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Run {
    pub trace: Vec<Value>,
    pub buffered: Vec<Value>,
    pub files: BTreeMap<String, Vec<u8>>,
    pub results: Vec<Value>,
    pub writes: usize,
}
struct Recording {
    clocks: Vec<u64>,
    samples: usize,
    trace: Vec<Value>,
}
#[derive(Debug)]
struct Crash;
thread_local! { static ACTIVE: RefCell<Option<Recording>> = const { RefCell::new(None) }; }
fn with<T>(f: impl FnOnce(&mut Recording) -> T) -> T {
    ACTIVE.with(|active| f(active.borrow_mut().as_mut().unwrap()))
}
pub(super) fn now() -> SystemTime {
    let ms = with(|r| {
        let ms = r.clocks.get(r.samples).copied().unwrap_or(10000);
        r.samples += 1;
        r.trace.push(json!(["clock", ms]));
        ms
    });
    UNIX_EPOCH + Duration::from_millis(ms)
}
fn rows(commit: &Commit) -> Vec<Value> {
    commit
        .rows
        .iter()
        .map(|r| match r {
            RowChange::Put { table, id, value } => json!(["put", table, id, value]),
            RowChange::Delete { table, id } => json!(["delete", table, id]),
        })
        .collect()
}
fn snapshot(files: &BTreeMap<String, Vec<u8>>, revision: usize) -> Snapshot {
    Snapshot {
        revision: tl3_store::Revision::try_from(revision as i64).unwrap(),
        epoch: tl3_store::Epoch::try_from(1).unwrap(),
        rows: files
            .iter()
            .map(|(path, bytes)| {
                let (table, id) = path.split_once('/').unwrap();
                WorldRow {
                    table: table.into(),
                    id: id.into(),
                    value: serde_json::from_slice(bytes).unwrap(),
                    version: tl3_store::Revision::ZERO,
                    arrival: 0,
                }
            })
            .collect(),
        pending_effects: vec![],
        credit: vec![],
        interned: vec![],
    }
}
fn run(s: &Schedule, current: bool, cut: Option<(usize, bool)>) -> Run {
    ACTIVE.with(|active| {
        *active.borrow_mut() = Some(Recording {
            clocks: s.clocks.clone(),
            samples: 0,
            trace: vec![],
        })
    });
    let mut run = Run {
        trace: vec![],
        buffered: vec![],
        results: vec![],
        writes: 0,
        files: s
            .rows
            .iter()
            .map(|(table, id, v)| (format!("{table}/{id}"), serde_json::to_vec(v).unwrap()))
            .collect(),
    };
    let result = catch_unwind(AssertUnwindSafe(|| {
        for (index, observation) in s.observations.iter().enumerate() {
            let snapshot = snapshot(&run.files, index);
            let mut commit = Commit::new(format!("g2d-{index}"), json!(observation), &snapshot);
            with(|r| r.trace.push(json!(["translate", observation])));
            let result = if current && CURRENT_STUB {
                Err(Error::Invalid("G2d red current arm stub"))
            } else if current {
                current::run(&snapshot, observation.clone(), &mut commit)
            } else {
                reference::run(&snapshot, observation.clone(), &mut commit)
            };
            run.buffered = rows(&commit);
            for row in &run.buffered {
                with(|r| r.trace.push(json!(["row.push", row])));
            }
            match result {
                Err(error) => {
                    run.results.push(json!(["error", error.to_string()]));
                    return;
                }
                Ok(event) => {
                    assert!(
                        event.is_none(),
                        "Progress/Quota must not yield a kernel event"
                    );
                    run.results.push(json!(["ok", null]));
                }
            }
            // This is the unchanged outer transaction boundary, not translate's tail.
            // Row pushes above are buffered; only this write can persist them.
            let write = run.writes;
            run.writes += 1;
            let mode = s.write_results.get(write).copied().unwrap_or(0);
            with(|r| r.trace.push(json!(["store.commit", run.buffered, mode])));
            if cut == Some((write, false)) {
                std::panic::panic_any(Crash);
            }
            if mode == 0 {
                for row in &commit.rows {
                    match row {
                        RowChange::Put { table, id, value } => {
                            run.files.insert(
                                format!("{table}/{id}"),
                                serde_json::to_vec(value).unwrap(),
                            );
                        }
                        RowChange::Delete { table, id } => {
                            run.files.remove(&format!("{table}/{id}"));
                        }
                    }
                }
            }
            if cut == Some((write, true)) {
                std::panic::panic_any(Crash);
            }
            if mode != 0 {
                run.results.push(json!(["commit.error", mode]));
                return;
            }
        }
    }));
    if let Err(error) = result {
        if !error.is::<Crash>() {
            std::panic::resume_unwind(error);
        }
        run.results.push(json!(["crash", cut]));
    }
    run.trace = ACTIVE.with(|active| active.borrow_mut().take().unwrap().trace);
    run
}
pub(super) fn compare(s: &Schedule) -> Run {
    let old = run(s, false, None);
    let new = run(s, true, None);
    assert!(
        old == new,
        "complete schedule {s:?}; old results {:?}, new results {:?}; old trace {:?}, new trace {:?}",
        old.results,
        new.results,
        old.trace,
        new.trace
    );
    for index in 0..old.writes {
        for after in [false, true] {
            assert_eq!(
                run(s, false, Some((index, after))),
                run(s, true, Some((index, after))),
                "crash at write {index}, after={after}, schedule {s:?}"
            );
        }
    }
    old
}
pub(super) fn crash(s: &Schedule, after: bool) -> Run {
    let old = run(s, false, Some((0, after)));
    assert_eq!(old, run(s, true, Some((0, after))));
    old
}
