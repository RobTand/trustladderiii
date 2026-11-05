//! One boundary shared by verbatim old and checked new refresh bodies.
use super::*;
use std::{
    cell::RefCell,
    marker::PhantomData,
    panic::{AssertUnwindSafe, catch_unwind},
};

#[derive(Clone, Debug)]
pub(super) struct Schedule {
    pub frames: Vec<u8>,
    pub chunks: Vec<Vec<u8>>,
    pub initial_attempt: bool,
    pub initial_offset: u64,
    pub recorded: u64,
    pub previous_wait: bool,
}
impl Default for Schedule {
    fn default() -> Self {
        Self {
            frames: vec![],
            chunks: vec![super::line(0, false)],
            initial_attempt: true,
            initial_offset: 0,
            recorded: 0,
            previous_wait: false,
        }
    }
}
#[derive(Debug)]
struct Crash;
#[derive(Clone, Copy, Debug)]
struct Cut {
    write: usize,
    after: bool,
}
#[derive(Clone, Debug, PartialEq)]
struct Outcome {
    trace: Vec<Value>,
    sends: Vec<Value>,
    persisted: Value,
    files: BTreeMap<PathBuf, Vec<u8>>,
    result: Vec<Value>,
    offset: u64,
    ended: Vec<PathBuf>,
    active: Vec<(String, i64)>,
    writes: usize,
}
struct Recording {
    schedule: Schedule,
    trace: Vec<Value>,
    sends: Vec<Value>,
    persisted: Value,
    files: BTreeMap<PathBuf, Vec<u8>>,
    reads: usize,
    writes: usize,
    cut: Option<Cut>,
    call: usize,
}
thread_local! {static RECORDING:RefCell<Option<Recording>>=const {RefCell::new(None)};}
fn with<T>(f: impl FnOnce(&mut Recording) -> T) -> T {
    RECORDING.with(|r| f(r.borrow_mut().as_mut().unwrap()))
}
fn record(op: &str, args: Value, result: Value) {
    with(|r| r.trace.push(json!({"op":op,"args":args,"result":result})));
}
fn sample(op: &str, args: Value) -> u8 {
    let word = with(|r| {
        let v = r.schedule.frames.get(r.reads).copied().unwrap_or(0);
        r.reads += 1;
        v
    });
    record(op, args, json!(word));
    word
}
fn io_error(word: u8) -> std::io::Result<()> {
    match word {
        250 => Err(std::io::ErrorKind::PermissionDenied.into()),
        251 => Err(std::io::ErrorKind::Other.into()),
        _ => Ok(()),
    }
}
pub(super) struct Sender<T>(pub PhantomData<T>);
pub(super) fn send(_sender: &Sender<Input>, input: Input) -> Result<()> {
    let value = serde_json::to_value(input).unwrap();
    let word = sample("send", value.clone());
    if word == 250 {
        return Err(Error::Invalid("worker ingress receiver closed"));
    }
    with(|r| r.sends.push(value));
    Ok(())
}
pub(super) struct Run {
    pub root: PathBuf,
    pub delivery: Delivery,
    pub attempt: Option<(i64, PathBuf)>,
    pub watches: Vec<i32>,
    pub offset: u64,
    pub processes: BTreeMap<&'static str, u8>,
}
pub(super) struct Active {
    values: Vec<(String, i64)>,
}
impl Active {
    pub fn add(&mut self, d: &Delivery, n: i64) {
        self.values.push((d.key.clone(), n));
        record("active.add", json!([d.key, n]), Value::Null);
    }
}
pub(super) struct WorkerOutputSource {
    pub watches: BTreeMap<i32, PathBuf>,
    pub ended: Vec<PathBuf>,
    pub active: Active,
}
impl WorkerOutputSource {
    pub fn watch(&self, path: &std::path::Path) -> Result<i32> {
        let word = sample("watch", json!(path));
        io_error(word)?;
        Ok(7)
    }
}
pub(super) fn latest(root: &std::path::Path) -> Result<Option<(i64, PathBuf)>> {
    let word = sample("latest.stat", json!(root));
    io_error(word)?;
    Ok((word != 1).then(|| (1, root.join("attempt-00000001"))))
}
pub(super) fn read(path: &std::path::Path) -> Result<Option<Value>> {
    let word = sample("claim.read", json!(path));
    io_error(word)?;
    Ok(match word {
        1 => None,
        2 => Some(json!({"cancelled":true})),
        3 => Some(json!({"cancelled":"true"})),
        4 => return Err(Error::Invalid("invalid claim")),
        _ => Some(json!({"cancelled":false})),
    })
}
pub(super) mod process {
    use super::*;
    pub fn pidfd(path: &std::path::Path) -> Result<Option<u8>> {
        let word = sample("pidfd", json!(path));
        io_error(word)?;
        Ok((word != 1).then_some(word))
    }
}
pub(super) struct File;
impl File {
    pub fn open(path: PathBuf) -> std::io::Result<Self> {
        let word = sample("stdout.open", json!(path));
        io_error(word)?;
        if word == 1 {
            return Err(std::io::ErrorKind::NotFound.into());
        }
        Ok(Self)
    }
}
pub(super) struct BufReader {
    position: usize,
}
impl BufReader {
    pub fn new(_: File) -> Self {
        Self { position: 0 }
    }
    pub fn seek(&mut self, from: SeekFrom) -> std::io::Result<u64> {
        let SeekFrom::Start(offset) = from else {
            unreachable!()
        };
        let word = sample("stdout.seek", json!(offset));
        io_error(word)?;
        self.position = offset as usize;
        Ok(offset)
    }
    pub fn read_until(&mut self, _delimiter: u8, buffer: &mut Vec<u8>) -> std::io::Result<usize> {
        let word = sample("stdout.read_until", json!(self.position));
        io_error(word)?;
        let bytes = with(|r| {
            r.files
                .get(&PathBuf::from("run/attempt-00000001/stdout.jsonl"))
                .cloned()
                .unwrap_or_default()
        });
        let tail = bytes.get(self.position..).unwrap_or_default();
        let n = tail
            .iter()
            .position(|b| *b == b'\n')
            .map_or(tail.len(), |p| p + 1);
        buffer.extend_from_slice(&tail[..n]);
        self.position += n;
        record("stdout.bytes", json!(n), json!(buffer));
        Ok(n)
    }
}
pub(super) struct Store;
impl Store {
    pub fn worker_reconcilable(&mut self, key: &str) -> Result<bool> {
        let word = sample("store.worker_reconcilable", json!(key));
        io_error(word)?;
        Ok(word != 1)
    }
    pub fn worker_progress(
        &mut self,
        node: Option<&str>,
    ) -> Result<Vec<tl3_store::WorkerProgress>> {
        let word = sample("store.worker_progress", json!(node));
        io_error(word)?;
        if word == 1 {
            return Ok(vec![]);
        }
        let value = with(|r| r.persisted.clone());
        Ok(vec![tl3_store::WorkerProgress {
            key: if word == 2 {
                "different"
            } else {
                "worker-events"
            }
            .into(),
            node: "0".into(),
            epoch: 1,
            attempt: if word == 3 { 2 } else { 1 },
            output_offset: value["offset"].as_i64().unwrap_or(0),
            unit: "model_turn".into(),
            external_wait: serde_json::from_value(value["wait"].clone()).unwrap(),
            started_ms: 0,
            last_progress_ms: None,
        }])
    }
    pub fn record_worker_progress(
        &mut self,
        key: &str,
        attempt: i64,
        offset: i64,
        unit: &str,
        wait: Option<&tl3_store::ExternalWait>,
    ) -> Result<()> {
        let value = json!({"key":key,"attempt":attempt,"offset":offset,"unit":unit,"wait":wait});
        let index = with(|r| {
            let n = r.writes;
            r.writes += 1;
            n
        });
        let word = sample("store.record_worker_progress", value.clone());
        if with(|r| r.cut.is_some_and(|c| c.write == index && !c.after)) {
            std::panic::panic_any(Crash);
        }
        if io_error(word).is_ok() {
            with(|r| r.persisted = value);
        }
        if with(|r| r.cut.is_some_and(|c| c.write == index && c.after)) {
            std::panic::panic_any(Crash);
        }
        io_error(word)?;
        Ok(())
    }
}
pub(super) fn inspect_recorded(
    path: &std::path::Path,
    d: &Delivery,
    _store: &mut Store,
) -> Result<Reconciliation> {
    let word = sample("inspect_recorded", json!([path, d.key]));
    io_error(word)?;
    Ok(match word {
        1 => Reconciliation::Absent,
        2 => Reconciliation::Found(Box::new(crate::executor::Completion::confirmed(vec![
            Observation::Progress {
                key: d.key.clone(),
                checkpoint: "ended".into(),
            },
        ]))),
        _ => Reconciliation::Pending,
    })
}
fn execute(schedule: &Schedule, new: bool, cut: Option<Cut>) -> Outcome {
    RECORDING.with(|r|*r.borrow_mut()=Some(Recording{schedule:schedule.clone(),trace:vec![],sends:vec![],
        persisted:json!({"offset":schedule.recorded,"wait":if schedule.previous_wait {json!({"upstream":"prismabuild","key":"a".repeat(64)})}else{Value::Null}}),
        files:BTreeMap::new(),reads:0,writes:0,cut,call:0}));
    let mut source = WorkerOutputSource {
        watches: BTreeMap::new(),
        ended: vec![],
        active: Active { values: vec![] },
    };
    let mut run = Run {
        root: PathBuf::from("run"),
        delivery: super::delivery(false),
        attempt: schedule
            .initial_attempt
            .then(|| (1, PathBuf::from("run/attempt-00000001"))),
        watches: vec![],
        offset: schedule.initial_offset,
        processes: BTreeMap::new(),
    };
    let mut results = vec![];
    for chunk in &schedule.chunks {
        with(|r| {
            r.files
                .entry(PathBuf::from("run/attempt-00000001/stdout.jsonl"))
                .or_default()
                .extend(chunk)
        });
        let result = catch_unwind(AssertUnwindSafe(|| {
            if new {
                super::current::refresh_run(&mut source, &mut run)
            } else {
                super::reference::refresh_run(&mut source, &mut run)
            }
        }));
        match result {
            Ok(Ok(v)) => results.push(json!(v)),
            Ok(Err(e)) => {
                results.push(json!({"error":e.to_string()}));
                break;
            }
            Err(e) if e.is::<Crash>() => {
                results.push(json!("crash"));
                break;
            }
            Err(e) => std::panic::resume_unwind(e),
        }
    }
    RECORDING.with(|r| {
        let r = r.borrow_mut().take().unwrap();
        Outcome {
            trace: r.trace,
            sends: r.sends,
            persisted: r.persisted,
            files: r.files,
            result: results,
            offset: run.offset,
            ended: source.ended,
            active: source.active.values,
            writes: r.writes,
        }
    })
}
pub(super) fn compare(schedule: &Schedule) -> Outcome {
    let old = execute(schedule, false, None);
    assert_eq!(old, execute(schedule, true, None), "schedule {schedule:?}");
    for write in 0..old.writes {
        for after in [false, true] {
            let cut = Some(Cut { write, after });
            assert_eq!(
                execute(schedule, false, cut),
                execute(schedule, true, cut),
                "schedule {schedule:?}, crash {cut:?}"
            );
        }
    }
    old
}
pub(super) fn schedules() {
    use proptest::prelude::*;
    super::runner(128)
        .run(
            &(
                proptest::collection::vec(any::<u8>(), 0..60),
                proptest::collection::vec(any::<u8>(), 1..20),
                any::<bool>(),
                0u64..300,
                any::<bool>(),
            ),
            |(frames, kinds, initial_attempt, recorded, previous_wait)| {
                let chunks = kinds.into_iter().map(|k| super::line(k, false)).collect();
                compare(&Schedule {
                    frames,
                    chunks,
                    initial_attempt,
                    recorded,
                    previous_wait,
                    initial_offset: 0,
                });
                Ok(())
            },
        )
        .unwrap();
}
pub(super) fn mandatory() {
    let stable = Schedule::default();
    let result = compare(&stable);
    assert_eq!(result.writes, 1);
    assert_eq!(result.sends.len(), 1);
    let mut changed = Schedule {
        chunks: vec![
            super::line(2, false),
            super::line(2, false),
            super::line(1, false),
        ],
        ..stable.clone()
    };
    let result = compare(&changed);
    assert_eq!(result.writes, 3);
    assert_eq!(result.sends.len(), 2);
    assert!(result.sends[0].get("Look").is_some());
    changed.previous_wait = true;
    assert_eq!(compare(&changed).sends.len(), 1);
    assert_eq!(
        compare(&Schedule {
            recorded: 10000,
            ..stable.clone()
        })
        .writes,
        0
    );
    for (op, nth) in [
        ("store.worker_reconcilable", 0),
        ("latest.stat", 1),
        ("claim.read", 0),
        ("claim.read", 1),
        ("stdout.open", 0),
        ("store.worker_progress", 0),
        ("inspect_recorded", 0),
    ] {
        for value in [1, 2, 3, 250, 251] {
            let mut s = Schedule {
                initial_attempt: false,
                ..stable.clone()
            };
            let base = execute(&s, false, None);
            let index = base
                .trace
                .iter()
                .filter(|v| v["op"] != "stdout.bytes" && v["op"] != "active.add")
                .enumerate()
                .filter(|(_, v)| v["op"] == op)
                .nth(nth)
                .map(|(n, _)| n);
            if let Some(index) = index {
                s.frames = vec![0; index + 1];
                s.frames[index] = value;
                let r = compare(&s);
                assert!(
                    r.trace
                        .iter()
                        .any(|v| v["op"] == op && v["result"] == value)
                );
            }
        }
    }
    let complete = super::line(0, false);
    let split = complete.len() / 2;
    let r = compare(&Schedule {
        chunks: vec![complete[..split].to_vec(), complete[split..].to_vec()],
        ..stable
    });
    assert_eq!(r.writes, 1);
}
