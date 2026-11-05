use super::*;
use serde::Serialize;
use std::cell::RefCell;
use std::panic::{AssertUnwindSafe, catch_unwind};

#[derive(Clone, Debug)]
pub(super) enum Read {
    Value(Option<Value>),
    Error,
}
#[derive(Clone, Debug)]
pub(super) struct Schedule {
    pub event: Event,
    pub nodes: Vec<tl3_kernel::Node>,
    pub detailed: bool,
    pub rows: Vec<RowChange>,
    pub materials: Vec<Read>,
    pub documents: Vec<Read>,
    pub failed: Vec<(String, String)>,
    pub outside: Vec<String>,
    pub total: usize,
    pub checks: Vec<(String, String, Option<String>)>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(super) enum RowChange {
    Put {
        table: String,
        id: String,
        value: Value,
    },
    Delete {
        table: String,
        id: String,
    },
}
pub(super) fn put(table: &str, id: String, value: Value) -> RowChange {
    RowChange::Put {
        table: table.into(),
        id,
        value,
    }
}
pub(super) struct Rows(pub Vec<RowChange>);
impl Rows {
    pub fn iter(&self) -> std::slice::Iter<'_, RowChange> {
        self.0.iter()
    }
    pub fn push(&mut self, row: RowChange) {
        let index = with(|r| {
            let index = r.writes;
            r.writes += 1;
            r.trace.push(json!(["row.push", row]));
            index
        });
        cut(index, false);
        self.0.push(row);
        cut(index, true);
    }
}
pub(super) struct Commit {
    pub rows: Rows,
}
pub(super) struct Snapshot;
pub(super) struct Store;
pub(super) struct Controller {
    pub store: Store,
}
impl Store {
    pub fn worker_materials(&mut self, node: &str) -> Result<Option<Value>> {
        read("worker_materials", node, None)
    }
    pub fn worker_document(&mut self, node: &str, table: &str) -> Result<Option<Value>> {
        read("worker_document", node, Some(table))
    }
}
impl Controller {
    pub fn priority_reserve(&self, _: &Snapshot, event: Event) -> Result<Event> {
        with(|r| r.trace.push(json!(["priority_reserve", event])));
        Ok(event)
    }
}
#[derive(Default)]
struct Recording {
    trace: Vec<Value>,
    stderr: Vec<u8>,
    writes: usize,
    material_reads: usize,
    document_reads: usize,
    materials: Vec<Read>,
    documents: Vec<Read>,
    cut: Option<(usize, bool)>,
}
thread_local! {static ACTIVE: RefCell<Recording> = RefCell::new(Recording::default());}
fn with<T>(f: impl FnOnce(&mut Recording) -> T) -> T {
    ACTIVE.with(|r| f(&mut r.borrow_mut()))
}
#[derive(Debug)]
struct Crash;
fn cut(index: usize, after: bool) {
    if with(|r| r.cut == Some((index, after))) {
        std::panic::panic_any(Crash);
    }
}
pub(super) fn diagnostic(message: String) {
    with(|r| {
        r.trace.push(json!(["eprintln", message]));
        r.stderr.extend_from_slice(message.as_bytes());
        r.stderr.push(b'\n');
    });
}
fn read(op: &str, node: &str, table: Option<&str>) -> Result<Option<Value>> {
    let value = with(|r| {
        let (tape, at) = if table.is_some() {
            (&r.documents, &mut r.document_reads)
        } else {
            (&r.materials, &mut r.material_reads)
        };
        let value = tape.get(*at).cloned().unwrap_or(Read::Value(None));
        *at += 1;
        let result = match &value {
            Read::Value(v) => json!(["ok", v]),
            Read::Error => json!(["error"]),
        };
        r.trace.push(json!([op, node, table, result]));
        value
    });
    match value {
        Read::Value(v) => Ok(v),
        Read::Error => Err(Error::Invalid(if table.is_some() {
            "document read error"
        } else {
            "material read error"
        })),
    }
}
#[derive(Debug, PartialEq, Eq)]
pub(super) struct Run {
    pub trace: Vec<Value>,
    pub stderr: Vec<u8>,
    pub buffered: Vec<u8>,
    pub persisted: Vec<u8>,
    pub result: String,
    pub writes: usize,
}
fn run(schedule: &Schedule, current: bool, cut_at: Option<(usize, bool)>) -> Run {
    ACTIVE.with(|r| {
        *r.borrow_mut() = Recording {
            materials: schedule.materials.clone(),
            documents: schedule.documents.clone(),
            cut: cut_at,
            ..Recording::default()
        }
    });
    let mut world = World::new(&[1], 1);
    world.nodes = schedule.nodes.clone();
    let mut controller = Controller { store: Store };
    let mut commit = Commit {
        rows: Rows(schedule.rows.clone()),
    };
    let failed = schedule
        .failed
        .iter()
        .map(|(test, message)| findings::FailedTest {
            test: test.clone(),
            message: message.clone(),
        })
        .collect();
    let checks = schedule
        .checks
        .iter()
        .map(|(name, summary, url)| findings::CiFailure {
            name: name.clone(),
            summary: summary.clone(),
            url: url.clone(),
        })
        .collect();
    let result = catch_unwind(AssertUnwindSafe(|| {
        if current {
            // Explicit red-stage new-side stub. The real current copy also compiles.
            if true {
                return Ok(None);
            }
            current::Shell(&mut controller).tail(
                &Snapshot,
                &world,
                schedule.event,
                &mut commit,
                schedule.detailed,
                failed,
                schedule.outside.clone(),
                schedule.total,
                checks,
            )
        } else {
            reference::Shell(&mut controller).tail(
                &Snapshot,
                &world,
                schedule.event,
                &mut commit,
                schedule.detailed,
                failed,
                schedule.outside.clone(),
                schedule.total,
                checks,
            )
        }
    }));
    let result = match result {
        Ok(Ok(value)) => format!("ok:{}", serde_json::to_string(&value).unwrap()),
        Ok(Err(error)) => format!("error:{error}"),
        Err(crash) => {
            assert!(crash.is::<Crash>(), "unexpected panic");
            "crash".into()
        }
    };
    let buffered = serde_json::to_vec(&commit.rows.0).unwrap();
    let persisted = if result.starts_with("ok:") {
        buffered.clone()
    } else {
        serde_json::to_vec(&schedule.rows).unwrap()
    };
    with(|r| Run {
        trace: r.trace.clone(),
        stderr: r.stderr.clone(),
        buffered,
        persisted,
        result,
        writes: r.writes,
    })
}
pub(super) fn compare(schedule: &Schedule) -> Run {
    let old = run(schedule, false, None);
    let new = run(schedule, true, None);
    assert_eq!(old, new, "complete schedule: {schedule:?}");
    for index in 0..old.writes {
        for after in [false, true] {
            assert_eq!(
                run(schedule, false, Some((index, after))),
                run(schedule, true, Some((index, after))),
                "write {index}, after {after}, {schedule:?}"
            );
        }
    }
    old
}
