//! Recorded boundary for the verbatim old functions and checked current shell.
use super::*;
use ::std::{
    cell::RefCell,
    io,
    panic::{AssertUnwindSafe, catch_unwind},
    time::{Duration, SystemTime},
};
use tl3_github::{
    CheckAnnotation, Comment, CommitStatus, Issue, RequiredChecks, RequiredStatusCheck, WorkflowRun,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(super) enum Profile {
    Plain,
    Flip,
    Label,
    Command,
    Waiting,
    Rerun,
    Body,
    Classic,
    Duplicates,
    Refusal,
    Active,
    Terminal,
    Merged,
    MergeFalse,
    Draft,
    Missing,
}
#[derive(Clone, Copy, Debug)]
pub(super) enum Entry {
    Read,
    Completion,
    Execute,
    Reconcile,
    Waiting,
    Command,
    Rerun,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct Frame {
    pub word: u16,
    pub elapsed_ms: u32,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct Schedule {
    pub profile: Profile,
    pub initial: u8,
    pub frames: Vec<Frame>,
    pub appears_at: usize,
    pub write_results: Vec<u8>,
    #[serde(default)]
    pub overrides: Vec<ReadOverride>,
    #[serde(default)]
    pub file_results: Vec<u8>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct ReadOverride {
    pub op: String,
    pub suffix: String,
    pub nth: usize,
    pub frame: Frame,
}
#[derive(Clone, Debug, PartialEq, Eq)]
struct Call {
    op: String,
    args: String,
    result: Vec<u8>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Run {
    calls: Vec<Call>,
    effects: Vec<Call>,
    files: BTreeMap<String, Vec<u8>>,
    result: Value,
    writes: usize,
}
#[derive(Clone, Copy, Debug)]
struct Cut {
    write: usize,
    after: bool,
}
#[derive(Debug)]
struct Crash;
struct Recording {
    schedule: Schedule,
    calls: Vec<Call>,
    effects: Vec<Call>,
    files: BTreeMap<String, Vec<u8>>,
    reads: usize,
    recovery_reads: usize,
    writes: usize,
    api_writes: usize,
    file_writes: usize,
    now_ms: u64,
    cut: Option<Cut>,
}
thread_local! { static ACTIVE: RefCell<Option<Recording>> = const { RefCell::new(None) }; }
fn with<T>(f: impl FnOnce(&mut Recording) -> T) -> T {
    ACTIVE.with(|v| f(v.borrow_mut().as_mut().expect("recorded I/O scope")))
}
fn read(op: &str, args: &str) -> Frame {
    with(|r| {
        let mut frame = r.schedule.frames.get(r.reads).cloned().unwrap_or(Frame {
            word: 0,
            elapsed_ms: 0,
        });
        if let Some(over) = r.schedule.overrides.iter().find(|over| {
            over.op == op
                && args.ends_with(&over.suffix)
                && r.calls
                    .iter()
                    .filter(|c| c.op == op && c.args.ends_with(&over.suffix))
                    .count()
                    == over.nth
        }) {
            frame = over.frame.clone();
        }
        r.reads += 1;
        r.now_ms = r.now_ms.saturating_add(u64::from(frame.elapsed_ms));
        r.calls.push(Call {
            op: op.into(),
            args: args.into(),
            result: Vec::new(),
        });
        frame
    })
}
fn returned<T: Serialize>(value: &T) {
    with(|r| r.calls.last_mut().unwrap().result = serde_json::to_vec(value).unwrap());
}
fn write(
    op: &str,
    args: &str,
    bytes: &[u8],
    change: impl FnOnce(&mut Recording),
) -> io::Result<()> {
    let mode = with(|r| {
        let mode = r
            .schedule
            .file_results
            .get(r.file_writes)
            .copied()
            .unwrap_or(0);
        r.file_writes += 1;
        mode
    });
    mutation(op, args, bytes, mode == 0, change);
    with(|r| {
        r.calls
            .last_mut()
            .unwrap()
            .result
            .extend_from_slice(&[mode])
    });
    if mode == 0 {
        Ok(())
    } else {
        Err(io::Error::from(match mode {
            1 => io::ErrorKind::PermissionDenied,
            2 => io::ErrorKind::StorageFull,
            _ => io::ErrorKind::Other,
        }))
    }
}
fn mutation(op: &str, args: &str, bytes: &[u8], landed: bool, change: impl FnOnce(&mut Recording)) {
    with(|r| {
        let index = r.writes;
        r.writes += 1;
        let call = Call {
            op: op.into(),
            args: args.into(),
            result: bytes.to_vec(),
        };
        r.calls.push(call.clone());
        if r.cut.is_some_and(|c| c.write == index && !c.after) {
            ::std::panic::panic_any(Crash);
        }
        if landed {
            change(r);
            r.effects.push(call);
        }
        if r.cut.is_some_and(|c| c.write == index && c.after) {
            ::std::panic::panic_any(Crash);
        }
    });
}
pub(super) fn has_store() -> bool {
    with(|r| {
        matches!(r.schedule.profile, Profile::Waiting | Profile::Body)
            || r.schedule.initial & 1 != 0
    })
}
fn profile() -> Profile {
    with(|r| r.schedule.profile)
}
fn initial() -> u8 {
    with(|r| r.schedule.initial)
}
fn recovery_path() -> String {
    let digest = Sha256::digest("mergify-requeue:owner/sandbox:100:head");
    let key = format!("requeue:{digest:x}");
    format!(
        "root/{}.requeue",
        key.bytes().map(|b| format!("{b:02x}")).collect::<String>()
    )
}
fn intent() -> Value {
    json!({"before":null,"last_event":0,"requeue":match (initial() >> 1) % 4 {0=>Some("Ready"),1=>Some("Sending"),2=>Some("Refused"),_=>None}})
}
fn recovery() -> Value {
    json!({"eligible_since_ms":if initial() & 4!=0 {None} else {Some(0u64)},"command": match (initial()>>3)%6 {0=>"Ready",1=>"Sending",2=>"Sent",3=>"Refused",4=>"PermissionDenied",_=>"Rejected"},"command_key":"enqueue-1"})
}
fn init(schedule: Schedule, cut: Option<Cut>) {
    ACTIVE.with(|v| {
        *v.borrow_mut() = Some(Recording {
            schedule,
            calls: vec![],
            effects: vec![],
            files: BTreeMap::new(),
            reads: 0,
            recovery_reads: 0,
            writes: 0,
            api_writes: 0,
            file_writes: 0,
            now_ms: 1_000_000,
            cut,
        })
    });
    with(|r| {
        r.files.insert("root/epoch.fence".into(), b"1".to_vec());
    });
    let p = profile();
    if matches!(p, Profile::Flip | Profile::Command) || initial() & 64 != 0 {
        let value = serde_json::to_vec(&intent()).unwrap();
        with(|r| {
            r.files
                .insert("root/656e71756575652d31.intent".into(), value);
        });
    }
    if p == Profile::Waiting || initial() & 128 != 0 {
        let value = serde_json::to_vec(&recovery()).unwrap();
        with(|r| {
            r.files.insert(recovery_path(), value);
        });
    }
}
fn delivery_value(key: &str) -> Value {
    configured_delivery(
        json!({"key":key,"epoch":1,"output":{"Effect":{"node":0,"seq":0,"kind":"Enqueue","head":null}},"generation":0,"head_sha":"head","repository":{"owner":"owner","name":"sandbox","installation":3},"issue":1,"pull_request":100,"paths":[],"attempt":0,"round":0}),
    )
}
fn run(schedule: &Schedule, entry: Entry, new: bool, cut: Option<Cut>) -> Run {
    static HOOK: ::std::sync::Once = ::std::sync::Once::new();
    HOOK.call_once(|| {
        let original = ::std::panic::take_hook();
        ::std::panic::set_hook(Box::new(move |info| {
            if !info.payload().is::<Crash>() {
                original(info)
            }
        }));
    });
    init(schedule.clone(), cut);
    let d: Delivery = serde_json::from_value(delivery_value("enqueue-1")).unwrap();
    let result = catch_unwind(AssertUnwindSafe(|| {
        if new {
            super::current::run(entry, &d)
        } else {
            super::reference::run(entry, &d)
        }
    }));
    let result = match result {
        Ok(v) => v,
        Err(e) if e.is::<Crash>() => json!("crash"),
        Err(e) => ::std::panic::resume_unwind(e),
    };
    ACTIVE.with(|v| {
        let r = v.borrow_mut().take().unwrap();
        Run {
            calls: r.calls,
            effects: r.effects,
            files: r.files,
            result,
            writes: r.writes,
        }
    })
}
pub(super) fn compare(schedule: &Schedule, entry: Entry) -> ::std::result::Result<usize, String> {
    let old = run(schedule, entry, false, None);
    let new = run(schedule, entry, true, None);
    if old != new {
        return Err(format!(
            "I/O schedule divergence\nschedule={schedule:?}\nentry={entry:?}\nold outcome={}\nnew outcome={}\nold calls={:?}\nnew calls={:?}",
            old.result,
            new.result,
            old.calls
                .iter()
                .map(|c| (&c.op, &c.args))
                .collect::<Vec<_>>(),
            new.calls
                .iter()
                .map(|c| (&c.op, &c.args))
                .collect::<Vec<_>>()
        ));
    }
    let writes = old.writes;
    for write in 0..writes {
        for after in [false, true] {
            let cut = Some(Cut { write, after });
            let old = run(schedule, entry, false, cut);
            let new = run(schedule, entry, true, cut);
            if old != new {
                return Err(format!(
                    "crash boundary divergence: {schedule:?}, {entry:?}, {cut:?}\nold={old:#?}\nnew={new:#?}"
                ));
            }
        }
    }
    Ok(writes * 2 + 1)
}
pub(super) fn require_field(s: &Schedule, e: Entry, key: &str, expected: Value) {
    assert_eq!(run(s, e, false, None).result[key], expected);
}
pub(super) fn require_pull_flag(s: &Schedule, key: &str, expected: bool) {
    assert_eq!(
        run(s, Entry::Read, false, None).result["pull"][key],
        json!(expected)
    );
}
pub(super) fn require_ack(s: &Schedule, e: Entry, expected: &str) {
    assert_eq!(
        run(s, e, false, None).result["found"]["ack"],
        json!(expected)
    );
}
pub(super) fn require_count(s: &Schedule, e: Entry, op: &str, count: usize) {
    assert_eq!(
        run(s, e, false, None)
            .calls
            .iter()
            .filter(|c| c.op == op)
            .count(),
        count
    );
}
pub(super) fn require_error(s: &Schedule, e: Entry) {
    assert!(run(s, e, false, None).result.get("error").is_some());
}
pub(super) fn require_ops(s: &Schedule, entry: Entry, ops: &[&str]) {
    let trace = run(s, entry, false, None);
    for op in ops {
        assert!(
            trace.calls.iter().any(|call| call.op == *op),
            "mandatory branch missed {op}: {trace:#?}"
        );
    }
}
pub(super) fn reconciliation(result: Result<Reconciliation>) -> Value {
    match result {
        Ok(Reconciliation::Found(c)) => json!({"found":c}),
        Ok(Reconciliation::Pending) => json!("Pending"),
        Ok(Reconciliation::Absent) => json!("Absent"),
        Err(e) => json!({"error":e.to_string()}),
    }
}
pub(super) fn completed(result: Result<Completion>) -> Value {
    match result {
        Ok(c) => json!({"completion":c}),
        Err(e) => json!({"error":e.to_string()}),
    }
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PathBuf(String);
pub(crate) type Path = PathBuf;
impl PathBuf {
    pub fn from(s: &str) -> Self {
        Self(s.into())
    }
    pub fn to_owned(&self) -> Self {
        self.clone()
    }
    pub fn join(&self, s: impl AsRef<str>) -> Self {
        Self(format!("{}/{}", self.0, s.as_ref()))
    }
    pub fn with_extension(&self, ext: &str) -> Self {
        Self(format!(
            "{}.{ext}",
            self.0.rsplit_once('.').map_or(self.0.as_str(), |(s, _)| s)
        ))
    }
    pub fn exists(&self) -> bool {
        let f = read("file.exists", &self.0);
        let yes = with(|r| r.files.contains_key(&self.0)) ^ (f.word & 1 != 0);
        returned(&yes);
        yes
    }
}
pub(super) mod mock_std {
    pub use ::std::*;
    pub mod path {
        pub(crate) use super::super::{Path, PathBuf};
    }
    pub mod fs {
        use super::super::*;
        pub fn read(path: impl ::std::borrow::Borrow<PathBuf>) -> io::Result<Vec<u8>> {
            let path = path.borrow();
            let f = super::super::read("file.read", &path.0);
            if f.word & 1024 != 0 {
                returned(&"PermissionDenied");
                return Err(io::Error::from(io::ErrorKind::PermissionDenied));
            }
            if f.word & 2048 != 0 {
                returned(&"Other");
                return Err(io::Error::from(io::ErrorKind::Other));
            }
            if path.0.ends_with(".requeue") {
                let appear = with(|r| {
                    let n = r.recovery_reads;
                    r.recovery_reads += 1;
                    r.schedule.profile == Profile::Flip && n >= r.schedule.appears_at
                });
                if appear {
                    let value = serde_json::to_vec(&recovery()).unwrap();
                    with(|r| {
                        r.files.insert(path.0.clone(), value);
                    });
                }
            }
            if f.word & 1 != 0 {
                with(|r| {
                    r.files.remove(&path.0);
                });
            }
            if f.word & 2 != 0 && path.0.ends_with(".requeue") {
                let value = serde_json::to_vec(&recovery()).unwrap();
                with(|r| {
                    r.files.insert(path.0.clone(), value);
                });
            }
            if f.word & 2 != 0 {
                let value = if path.0.ends_with(".intent") {
                    Some(
                        json!({"before":null,"last_event":f.word>>3,"requeue":match (f.word>>2)%4 {0=>Some("Ready"),1=>Some("Sending"),2=>Some("Refused"),_=>None}}),
                    )
                } else if path.0.ends_with(".reruns") {
                    Some(
                        json!([{"run":run_value(&f),"accepted":f.word&4!=0,"waiting_for_completion":f.word&8!=0}]),
                    )
                } else if path.0.ends_with(".json") {
                    Some(
                        json!({"key":"enqueue-1","deliveries":0,"epochs":[],"completion":Completion::confirmed(vec![])}),
                    )
                } else if path.0.ends_with(".fence") {
                    Some(json!(i64::from(f.word >> 2) % 3))
                } else {
                    None
                };
                if let Some(value) = value {
                    with(|r| {
                        r.files
                            .insert(path.0.clone(), serde_json::to_vec(&value).unwrap());
                    });
                }
            }
            let value = if f.word & 4096 != 0 {
                Some(b"invalid json".to_vec())
            } else {
                with(|r| r.files.get(&path.0).cloned())
            };
            returned(&value);
            value.ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))
        }
        pub fn rename(from: PathBuf, to: &PathBuf) -> io::Result<()> {
            write("file.rename", &format!("{}->{}", from.0, to.0), &[], |r| {
                let b = r.files.remove(&from.0).unwrap();
                r.files.insert(to.0.clone(), b);
            })
        }
        pub struct File(PathBuf);
        impl File {
            pub fn create(path: &PathBuf) -> io::Result<Self> {
                write("file.create", &path.0, &[], |r| {
                    r.files.insert(path.0.clone(), vec![]);
                })?;
                Ok(Self(path.clone()))
            }
            pub fn open(path: &PathBuf) -> io::Result<Self> {
                let f = super::super::read("file.open", &path.0);
                returned(&(f.word & 1 == 0));
                if f.word & 1 != 0 {
                    return Err(io::Error::from(io::ErrorKind::PermissionDenied));
                }
                Ok(Self(path.clone()))
            }
            pub fn sync_all(&self) -> io::Result<()> {
                write("file.fsync", &self.0.0, &[], |_| {})
            }
        }
        impl io::Write for File {
            fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
                write("file.write", &self.0.0, bytes, |r| {
                    r.files.get_mut(&self.0.0).unwrap().extend(bytes);
                })?;
                Ok(bytes.len())
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
    }
}
#[derive(Clone)]
pub(crate) struct Row {
    pub table: &'static str,
    pub id: String,
    pub value: Value,
    pub version: Version,
}
#[derive(Clone)]
pub(crate) struct Snapshot {
    pub rows: Vec<Row>,
    pub repaired: bool,
}
impl Snapshot {
    pub fn held(&self) -> bool {
        self.rows[0].value["held"] == true
    }
}
pub(super) struct Archive {
    pub payload: Value,
}
pub(crate) struct Store;
impl Store {
    pub fn open(path: &PathBuf) -> Result<Self> {
        let f = read("store.open", &path.0);
        returned(&(f.word & 1 == 0));
        if f.word & 1 != 0 {
            return Err(io::Error::from(io::ErrorKind::PermissionDenied).into());
        }
        Ok(Self)
    }
    pub fn load(&self) -> Result<Snapshot> {
        let f = read("store.snapshot", "");
        let repaired = (profile() == Profile::Waiting) ^ (f.word & 4 != 0);
        returned(&(repaired, f.word & 8 != 0, f.word & 16 != 0));
        Ok(configured_snapshot(Snapshot {
            rows: vec![Row {
                table: "nodes",
                id: "0".into(),
                value: json!({"held":f.word&8!=0,"paused":f.word&16!=0}),
                version: Version(0),
            }],
            repaired,
        }))
    }
    pub fn effects(&self) -> Result<Vec<Archive>> {
        let f = read("store.effects", "");
        let payload = delivery_value("historical");
        let values = if f.word & 1 != 0 {
            vec![payload]
        } else {
            vec![]
        };
        returned(&values);
        Ok(values
            .into_iter()
            .map(|payload| Archive { payload })
            .collect())
    }
}
pub(super) struct Runtime;
impl Runtime {
    pub fn block_on<T>(&self, future: impl ::std::future::Future<Output = T>) -> T {
        let mut future = ::std::pin::pin!(future);
        let mut cx = ::std::task::Context::from_waker(::std::task::Waker::noop());
        match future.as_mut().poll(&mut cx) {
            ::std::task::Poll::Ready(v) => v,
            _ => panic!("recorded calls must be synchronous"),
        }
    }
}
pub(super) struct Clock;
impl Clock {
    pub fn now(&self) -> SystemTime {
        read("clock.now", "");
        let t = with(|r| r.now_ms);
        returned(&t);
        UNIX_EPOCH + Duration::from_millis(t)
    }
}
#[derive(Clone)]
pub(super) struct GitHub(pub ::std::sync::Arc<GitHubInner>);
pub(super) struct GitHubInner {
    pub slug: String,
    pub comments: tokio::sync::Mutex<()>,
}
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct Message(String);
impl From<&str> for Message {
    fn from(s: &str) -> Self {
        Self(s.into())
    }
}
impl From<String> for Message {
    fn from(s: String) -> Self {
        Self(s)
    }
}
pub(super) struct Mergify {
    pub(super) github: GitHub,
}
impl Mergify {
    pub fn new(github: GitHub) -> Self {
        Self { github }
    }
}
fn github_frame(op: &str, args: &str) -> ::std::result::Result<Frame, ::tl3_github::Error> {
    let f = read(op, args);
    if f.word & 256 != 0 {
        let status = match (f.word >> 5) % 4 {
            0 => 403,
            1 => 404,
            2 => 422,
            _ => 500,
        };
        returned(&json!({"error": status}));
        return Err(::tl3_github::Error::Http { status });
    };
    Ok(f)
}
fn pull(f: &Frame) -> PullRequest {
    configured_pull(serde_json::from_value(json!({"number":100,"user":{"id":1,"login":"factory[bot]"},"state":if f.word&1!=0 {"closed"}else{"open"},"draft":f.word&8!=0 || profile() == Profile::Draft,"merged_at":if f.word&4!=0 || profile() == Profile::Merged {Some("2026-11-01T00:00:00Z")}else{None},"mergeable":!(f.word&512!=0 || profile()==Profile::MergeFalse),"mergeable_state":if f.word&512!=0 || profile()==Profile::MergeFalse {"dirty"}else{"clean"},"body":if profile()==Profile::Body {""} else {"Change type: fix"},"merge_commit_sha":if profile()==Profile::Merged {Some("merge")}else{None},"head":{"sha":if f.word&2!=0 {"other"}else{"head"},"ref":"topic","repo":null},"base":{"sha":"base","ref":"main","repo":null},"updated_at":"2026-11-01T00:00:00Z"})).unwrap())
}
fn check(f: &Frame, queue: bool) -> CheckRun {
    let summary = include_str!("../../../mergify/tests/fixtures/waiting/checks.json");
    let mut checks: Value = serde_json::from_str(summary).unwrap();
    // The recorded fixture's queue evidence is valid; each read can replace it.
    let value = checks
        .as_array_mut()
        .unwrap()
        .iter()
        .find(|c| c["name"] == tl3_mergify::QUEUE_CHECK)
        .unwrap()
        .clone();
    let mut c: CheckRun = serde_json::from_value(value).unwrap();
    c.head_sha = "head".into();
    c.id = 10 + u64::from(f.word >> 5);
    if queue {
        c.output.as_mut().unwrap().summary=Some("<span itemprop=\"mergify.v1.eyJ2ZXJzaW9uIjogMSwgInN0YXRlIjogIndhaXRpbmciLCAicXVldWVfcnVsZV9uYW1lIjogbnVsbCwgInF1ZXVlZF9hdCI6IG51bGwsICJzcGVjdWxhdGl2ZV9jaGVja19wciI6IG51bGwsICJyZXF1aXJlZF9jb25kaXRpb25zIjogW119\"></span>".into());
    }

    if queue {
        let state = match (f.word >> 10) % 8 {
            1 => "queued",
            2 => "checking",
            3 => "dequeued",
            4 => "dequeued",
            5 => "dequeued",
            6 => "unknown",
            _ => match profile() {
                Profile::Active => "queued",
                Profile::Terminal => "dequeued",
                _ => "waiting",
            },
        };
        let reason = match (f.word >> 10) % 8 {
            4 => "CHECKS_FAILED",
            5 => "PR_MANUALLY_DEQUEUED",
            _ => "OTHER",
        };
        if state != "waiting" {
            let payload =
                json!({"version":1,"state":state,"dequeue_reason":{"code":reason}}).to_string();
            // Encode without a new dependency; serde-produced bytes go to Mergify's real parser.
            let encoded = base64(&payload);
            c.output.as_mut().unwrap().summary =
                Some(format!("<span itemprop=\"mergify.v1.{encoded}\"></span>"));
        }
    }
    if !queue {
        c.name = "CI".into();
        c.app = Some(tl3_github::CheckApp {
            id: 1,
            slug: "github-actions".into(),
        });
        c.status = "completed".into();
        c.conclusion = Some("failure".into());
        c.check_suite = Some(tl3_github::CheckSuiteId { id: 1 });
    }
    if f.word & 16 != 0 {
        c.status = "in_progress".into();
    }
    c
}
impl GitHub {
    pub fn new() -> Self {
        Self(::std::sync::Arc::new(GitHubInner {
            slug: "factory".into(),
            comments: tokio::sync::Mutex::new(()),
        }))
    }
    pub fn marker(key: &str) -> String {
        ::tl3_github::GitHub::marker(key)
    }
    pub fn app_slug(&self) -> &str {
        "factory"
    }
    pub async fn pull_request(
        &self,
        repo: &str,
        n: u64,
    ) -> ::std::result::Result<PullRequest, ::tl3_github::Error> {
        let f = github_frame("github.pull", &format!("{repo}#{n}"))?;
        let p = pull(&f);
        returned(&p);
        Ok(p)
    }
    pub async fn issue(
        &self,
        repo: &str,
        n: u64,
    ) -> ::std::result::Result<Issue, ::tl3_github::Error> {
        let f = github_frame("github.issue", &format!("{repo}#{n}"))?;
        let queued = matches!(
            profile(),
            Profile::Flip | Profile::Command | Profile::Waiting
        ) ^ (f.word & 1 != 0);
        let i:Issue=serde_json::from_value(json!({"number":n,"title":"issue","state":"open","updated_at":"2026-11-01T00:00:00Z","labels":if queued {json!([{"name":QUEUE_LABEL},{"name":"dequeued"}])}else{json!([])}})).unwrap();
        returned(&i);
        Ok(i)
    }
    pub async fn issue_events(
        &self,
        repo: &str,
        n: u64,
    ) -> ::std::result::Result<Vec<IssueActivity>, ::tl3_github::Error> {
        let f = github_frame("github.events", &format!("{repo}#{n}"))?;
        let e = if f.word & 1 != 0 {
            vec![serde_json::from_value(json!({"id":2,"event":"unlabeled","actor":{"id":7,"login":"person"},"created_at":"2026-11-01T00:00:00Z","label":{"name":QUEUE_LABEL}})).unwrap()]
        } else if matches!(profile(), Profile::Flip | Profile::Command) {
            vec![serde_json::from_value(json!({"id":1,"event":"labeled","actor":null,"created_at":"2026-11-01T00:00:00Z","label":{"name":"queued"}})).unwrap(),serde_json::from_value(json!({"id":2,"event":"labeled","actor":null,"created_at":"2026-11-01T00:00:00Z","label":{"name":"dequeued"}})).unwrap()]
        } else {
            vec![]
        };
        returned(&e);
        Ok(e)
    }
    pub async fn comments(
        &self,
        repo: &str,
        n: u64,
    ) -> ::std::result::Result<Vec<Comment>, ::tl3_github::Error> {
        let f = github_frame("github.comments", &format!("{repo}#{n}"))?;
        let key = if f.word & 8 != 0 {
            "mergify-requeue:owner/sandbox:100:head"
        } else {
            "enqueue-1"
        };
        let mut c =
            if f.word & 1 != 0 || matches!(profile(), Profile::Duplicates | Profile::Refusal) {
                vec![comment(
                    1,
                    key,
                    "factory[bot]",
                    &format!("@mergifyio queue\n{}", Self::marker(key)),
                )]
            } else {
                vec![]
            };
        if (f.word & 2 != 0 || profile() == Profile::Duplicates) && !c.is_empty() {
            c.push(comment(2, key, "factory[bot]", &c[0].body));
        }
        if f.word & 4 != 0 || profile() == Profile::Refusal {
            let mut reply = comment(3, key, "mergify[bot]", "> `queue`\n\n## Command disallowed");
            reply.user.id = 37929162;
            c.push(reply);
        }
        if f.word & 16 != 0 {
            c.push(comment(4, key, "person", &Self::marker(key)));
        }
        returned(&c);
        Ok(c)
    }
    pub async fn check_runs(
        &self,
        repo: &str,
        head: &str,
    ) -> ::std::result::Result<Vec<CheckRun>, ::tl3_github::Error> {
        let f = github_frame("github.checks", &format!("{repo}@{head}"))?;
        let mut c = vec![check(&f, profile() != Profile::Rerun)];
        if f.word & 512 != 0 {
            c.push(check(&f, false));
        }
        returned(&c);
        Ok(c)
    }
    pub async fn required_status_checks(
        &self,
        repo: &str,
        branch: &str,
    ) -> ::std::result::Result<RequiredChecks, ::tl3_github::Error> {
        let f = github_frame("github.requirements", &format!("{repo}@{branch}"))?;
        let result = RequiredChecks {
            checks: if matches!(
                profile(),
                Profile::Rerun | Profile::Classic | Profile::Missing
            ) ^ (f.word & 1 != 0)
            {
                vec![RequiredStatusCheck {
                    context: "CI".into(),
                    integration_id: if profile() == Profile::Classic || f.word & 2 != 0 {
                        None
                    } else {
                        Some(1)
                    },
                }]
            } else {
                vec![]
            },
            unavailable: vec![],
        };
        returned(
            &json!({"checks":result.checks.iter().map(|c|json!({"context":c.context,"integration_id":c.integration_id})).collect::<Vec<_>>(),"unavailable":result.unavailable}),
        );
        Ok(result)
    }
    pub async fn commit_statuses(
        &self,
        repo: &str,
        head: &str,
    ) -> ::std::result::Result<Vec<CommitStatus>, ::tl3_github::Error> {
        let f = github_frame("github.statuses", &format!("{repo}@{head}"))?;
        let values = if profile() == Profile::Classic || f.word & 1 != 0 {
            vec![
                json!({"id":1,"context":"CI","state":if f.word&2!=0 {"success"}else{"failure"},"updated_at":null}),
            ]
        } else {
            vec![]
        };
        returned(&values);
        Ok(values
            .into_iter()
            .map(|value| serde_json::from_value(value).unwrap())
            .collect())
    }
    pub async fn check_annotations(
        &self,
        repo: &str,
        id: u64,
    ) -> ::std::result::Result<Vec<CheckAnnotation>, ::tl3_github::Error> {
        let f = github_frame("github.annotations", &format!("{repo}/{id}"))?;
        let result = if f.word & 1 == 0 {
            vec![CheckAnnotation {
                annotation_level: "failure".into(),
                message: "The runner has lost communication with the server".into(),
            }]
        } else {
            vec![]
        };
        returned(
            &result
                .iter()
                .map(|a| json!({"annotation_level":a.annotation_level,"message":a.message}))
                .collect::<Vec<_>>(),
        );
        Ok(result)
    }
    pub async fn workflow_runs(
        &self,
        repo: &str,
        head: &str,
    ) -> ::std::result::Result<Vec<WorkflowRun>, ::tl3_github::Error> {
        let f = github_frame("github.runs", &format!("{repo}@{head}"))?;
        let result = vec![run_value(&f)];
        returned(&result);
        Ok(result)
    }
    pub async fn workflow_run(
        &self,
        repo: &str,
        id: u64,
    ) -> ::std::result::Result<WorkflowRun, ::tl3_github::Error> {
        let f = github_frame("github.run", &format!("{repo}/{id}"))?;
        let result = run_value(&f);
        returned(&result);
        Ok(result)
    }
    pub async fn add_label(
        &self,
        repo: &str,
        n: u64,
        label: &str,
    ) -> ::std::result::Result<(), ::tl3_github::Error> {
        api_write("github.add_label", &format!("{repo}#{n}"), label.as_bytes())
    }

    pub async fn request<T: serde::de::DeserializeOwned>(
        &self,
        method: reqwest::Method,
        path: String,
        _: Vec<(String, String)>,
        body: Option<Value>,
    ) -> ::std::result::Result<T, ::tl3_github::Error> {
        let payload = body.unwrap_or(Value::Null);
        let op = if method == reqwest::Method::DELETE {
            "github.delete_comment"
        } else {
            "github.command"
        };
        api_write(op, &path, &serde_json::to_vec(&payload).unwrap())?;
        let value = if method == reqwest::Method::DELETE {
            Value::Null
        } else {
            serde_json::to_value(comment(
                50,
                "",
                "factory[bot]",
                payload["body"].as_str().unwrap(),
            ))
            .unwrap()
        };
        Ok(serde_json::from_value(value).unwrap())
    }
    pub async fn update_pull_request_body(
        &self,
        repo: &str,
        n: u64,
        body: &str,
    ) -> ::std::result::Result<PullRequest, ::tl3_github::Error> {
        api_write("github.patch_body", &format!("{repo}#{n}"), body.as_bytes())?;
        let mut pull = pull(&Frame {
            word: 0,
            elapsed_ms: 0,
        });
        pull.body = Some(body.into());
        Ok(pull)
    }
    pub async fn rerun_failed_jobs(
        &self,
        repo: &str,
        n: u64,
    ) -> ::std::result::Result<(), ::tl3_github::Error> {
        api_write("github.rerun_failed", &format!("{repo}/{n}"), &[])
    }
    pub async fn check_suites(
        &self,
        repo: &str,
        head: &str,
    ) -> ::std::result::Result<Vec<::tl3_github::CheckSuite>, ::tl3_github::Error> {
        let f = github_frame("github.suites", &format!("{repo}@{head}"))?;
        let result=vec![serde_json::from_value(json!({"id":1,"head_sha":head,"status":if f.word&1!=0 {"in_progress"}else{"completed"},"conclusion":"success"})).unwrap()];
        returned(&result);
        Ok(result)
    }
    pub async fn job_log_tail(
        &self,
        repo: &str,
        id: u64,
    ) -> ::std::result::Result<::tl3_github::JobLogTail, ::tl3_github::Error> {
        let f = github_frame("github.job_log", &format!("{repo}/{id}"))?;
        let text = if f.word & 1 != 0 {
            "no test output"
        } else {
            "FAILED tests/test_queue.py::case"
        }
        .to_owned();
        returned(&(text.as_str(), f.word & 2 != 0));
        Ok(::tl3_github::JobLogTail {
            text,
            truncated: f.word & 2 != 0,
        })
    }
    pub async fn pull_request_paths(
        &self,
        repo: &str,
        n: u64,
    ) -> ::std::result::Result<Option<Vec<String>>, ::tl3_github::Error> {
        let f = github_frame("github.paths", &format!("{repo}#{n}"))?;
        let result = if f.word & 1 != 0 {
            None
        } else {
            Some(vec!["src/main.rs".to_owned()])
        };
        returned(&result);
        Ok(result)
    }
    pub async fn rerun_workflow_run(
        &self,
        repo: &str,
        id: u64,
    ) -> ::std::result::Result<(), ::tl3_github::Error> {
        api_write("github.rerun", &format!("{repo}/{id}"), &[])
    }
}
fn run_value(f: &Frame) -> WorkflowRun {
    WorkflowRun {
        id: 1,
        check_suite_id: 1,
        head_sha: if f.word & 2 != 0 { "other" } else { "head" }.into(),
        run_attempt: 1 + u64::from(f.word >> 5),
        status: if f.word & 1 != 0 {
            "in_progress"
        } else {
            "completed"
        }
        .into(),
        conclusion: Some("failure".into()),
        event: "pull_request".into(),
        path: ".github/workflows/gate.yml".into(),
    }
}
fn api_write(op: &str, args: &str, bytes: &[u8]) -> ::std::result::Result<(), ::tl3_github::Error> {
    let mode = with(|r| {
        let mode = r
            .schedule
            .write_results
            .get(r.api_writes)
            .copied()
            .unwrap_or(0);
        r.api_writes += 1;
        mode
    });
    mutation(
        op,
        args,
        &serde_json::to_vec(&(bytes, mode)).unwrap(),
        matches!(mode, 0 | 4),
        |_| {},
    );
    match mode {
        0 => Ok(()),
        1 => Err(::tl3_github::Error::Http { status: 403 }),
        2 => Err(::tl3_github::Error::Http { status: 422 }),
        3 | 4 => Err(::tl3_github::Error::Transport),
        5 => Err(::tl3_github::Error::ActionsForbidden(
            ::tl3_github::ActionsRefusal::RunInProgress,
        )),
        _ => Err(::tl3_github::Error::RateLimited {
            retry_after: Duration::from_secs(1),
        }),
    }
}

fn comment(id: u64, _: &str, login: &str, body: &str) -> Comment {
    serde_json::from_value(json!({"id":id,"body":body,"user":{"id":1,"login":login},"created_at":"2026-11-01T00:00:00Z","updated_at":"2026-11-01T00:00:00Z","issue_url":"unused"})).unwrap()
}
fn base64(s: &str) -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for b in s.as_bytes().chunks(3) {
        let n = (u32::from(b[0]) << 16)
            | (u32::from(*b.get(1).unwrap_or(&0)) << 8)
            | u32::from(*b.get(2).unwrap_or(&0));
        for shift in [18, 12, 6, 0] {
            out.push(ALPHABET[((n >> shift) & 63) as usize] as char);
        }
        if b.len() < 3 {
            out.pop();
            out.push('=');
        }
        if b.len() < 2 {
            out.pop();
            out.pop();
            out.push('=');
            out.push('=');
        }
    }
    out
}

// A1 r1 fixtures extend returned facts; the I/O recorder and crash machinery
// remain shared and unchanged. Thread-local fixtures reset even on a panic.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct Version(pub i64);
impl Version {
    pub fn get(self) -> i64 {
        self.0
    }
}
#[derive(Clone, Default)]
pub(super) struct A1Inputs {
    pub rows: Vec<Row>,
    pub delivery: Option<Delivery>,
    pub body: Option<String>,
}
thread_local! { static A1_INPUTS: RefCell<A1Inputs> = RefCell::new(A1Inputs::default()); }
pub(super) fn with_inputs<T>(inputs: A1Inputs, f: impl FnOnce() -> T) -> T {
    struct Restore(A1Inputs);
    impl Drop for Restore {
        fn drop(&mut self) {
            A1_INPUTS.with(|v| *v.borrow_mut() = self.0.clone());
        }
    }
    let _restore = Restore(A1_INPUTS.with(|v| v.replace(inputs)));
    f()
}
fn configured_snapshot(mut snapshot: Snapshot) -> Snapshot {
    A1_INPUTS.with(|v| snapshot.rows.extend(v.borrow().rows.clone()));
    snapshot
}
fn configured_delivery(default: Value) -> Value {
    A1_INPUTS.with(|v| {
        v.borrow()
            .delivery
            .as_ref()
            .map_or(default, |d| serde_json::to_value(d).unwrap())
    })
}
fn configured_pull(mut pull: PullRequest) -> PullRequest {
    A1_INPUTS.with(|v| {
        if let Some(body) = &v.borrow().body {
            pull.body = Some(body.clone());
        }
    });
    pull
}
pub(super) fn fixture_delivery() -> Delivery {
    serde_json::from_value(delivery_value("enqueue-1")).unwrap()
}
pub(super) fn projection(name: &str) {
    with(|r| {
        r.calls.push(Call {
            op: format!("projection.{name}"),
            args: String::new(),
            result: vec![],
        })
    });
}
pub(super) fn require_no_writes(s: &Schedule, e: Entry) {
    assert_eq!(run(s, e, false, None).writes, 0);
}

// B1 reachability checks inspect the reference trace as well as comparing both shells.
pub(super) fn require_recovery(s: &Schedule, entry: Entry, command: &str, timer: Option<u64>) {
    let trace = run(s, entry, false, None);
    let bytes = trace.files.get(&recovery_path()).expect("recovery receipt");
    let saved: Value = serde_json::from_slice(bytes).unwrap();
    assert_eq!(saved["command"], json!(command), "{trace:#?}");
    assert_eq!(saved["eligible_since_ms"], json!(timer), "{trace:#?}");
}
pub(super) fn require_no_op(s: &Schedule, entry: Entry, op: &str) {
    let trace = run(s, entry, false, None);
    assert!(!trace.calls.iter().any(|c| c.op == op), "{trace:#?}");
}
pub(super) fn require_command_fence(s: &Schedule) {
    let trace = run(s, Entry::Command, false, None);
    let fence = trace
        .calls
        .iter()
        .position(|c| c.op == "file.read" && c.args.ends_with("epoch.fence"))
        .unwrap();
    let intent = trace
        .calls
        .iter()
        .position(|c| {
            c.op == "file.write" && String::from_utf8_lossy(&c.result).contains("Sending")
        })
        .unwrap();
    let post = trace
        .calls
        .iter()
        .position(|c| c.op == "github.command")
        .unwrap();
    assert!(fence < intent && intent < post, "{trace:#?}");
}

// ctrl-core C1: direct evaluate inputs share the existing recorded leaf boundary.
pub(super) fn c1_pull() -> PullRequest {
    pull(&Frame {
        word: 0,
        elapsed_ms: 0,
    })
}
pub(super) fn c1_check(
    id: u64,
    name: &str,
    app: Option<(u64, &str)>,
    conclusion: Option<&str>,
    current: bool,
) -> CheckRun {
    let mut value = check(
        &Frame {
            word: 0,
            elapsed_ms: 0,
        },
        false,
    );
    value.id = id;
    value.name = name.into();
    value.app = app.map(|(id, slug)| tl3_github::CheckApp {
        id,
        slug: slug.into(),
    });
    value.conclusion = conclusion.map(str::to_owned);
    if !current {
        value.head_sha = "other".into();
    }
    value
}
pub(super) fn c1_evaluate(s: &Schedule, execute: impl Fn(bool) -> Value) -> Value {
    let record = |new| {
        init(s.clone(), None);
        let result = execute(new);
        ACTIVE.with(|active| {
            let r = active.borrow_mut().take().unwrap();
            Run {
                calls: r.calls,
                effects: r.effects,
                files: r.files,
                result,
                writes: r.writes,
            }
        })
    };
    let old = record(false);
    let new = record(true);
    assert_eq!(old, new, "C1 evaluate divergence: {s:?}");
    assert_eq!(old.writes, 0, "evaluate must have no write/crash boundary");
    new.result
}
pub(super) fn c1_require_order(s: &Schedule, e: Entry, ops: &[&str]) {
    let trace = run(s, e, false, None);
    let mut next = 0;
    for call in &trace.calls {
        if next < ops.len() && call.op == ops[next] {
            next += 1;
        }
    }
    assert_eq!(
        next,
        ops.len(),
        "mandatory ordered calls missed: {trace:#?}"
    );
}
pub(super) fn c1_require_error(s: &Schedule, e: Entry, expected: &str) {
    assert_eq!(run(s, e, false, None).result["error"], json!(expected));
}
pub(super) fn c1_require_read_count(s: &Schedule, e: Entry, suffix: &str, expected: usize) {
    let trace = run(s, e, false, None);
    assert_eq!(
        trace
            .calls
            .iter()
            .filter(|call| call.op == "file.read" && call.args.ends_with(suffix))
            .count(),
        expected
    );
}

// ctrl-core C1 r1: payload tapes extend the leaf boundary without changing #760's tapes.
pub(super) mod c1_payloads {
    use super::*;

    #[derive(Clone, Copy, Debug, Serialize)]
    pub(in crate::mergify_io) enum Outcome {
        Accepted,
        Http408,
        AppAccess,
        RunExpired,
        OtherRefusal,
    }
    #[derive(Clone, Debug)]
    pub(in crate::mergify_io) struct Payloads {
        pub schedule: Schedule,
        pub statuses: Vec<Vec<Value>>,
        pub runs: Vec<Vec<WorkflowRun>>,
        pub outcome: Outcome,
    }
    thread_local! {
        static PAYLOAD: RefCell<Option<(Payloads, usize, usize)>> = const { RefCell::new(None) };
    }
    #[derive(Clone)]
    pub(in crate::mergify_io) struct GitHub(super::GitHub);
    impl ::std::ops::Deref for GitHub {
        type Target = super::GitHub;
        fn deref(&self) -> &Self::Target {
            &self.0
        }
    }
    impl GitHub {
        pub fn new() -> Self {
            Self(super::GitHub::new())
        }
        pub async fn commit_statuses(
            &self,
            repo: &str,
            head: &str,
        ) -> ::std::result::Result<Vec<CommitStatus>, ::tl3_github::Error> {
            github_frame("github.statuses", &format!("{repo}@{head}"))?;
            let values = PAYLOAD.with(|p| {
                let mut p = p.borrow_mut();
                let (payload, next, _) = p.as_mut().unwrap();
                let values = payload.statuses.get(*next).cloned().unwrap_or_default();
                *next += 1;
                values
            });
            returned(&values);
            Ok(values
                .into_iter()
                .map(|v| serde_json::from_value(v).unwrap())
                .collect())
        }
        pub async fn workflow_runs(
            &self,
            repo: &str,
            head: &str,
        ) -> ::std::result::Result<Vec<WorkflowRun>, ::tl3_github::Error> {
            github_frame("github.runs", &format!("{repo}@{head}"))?;
            let values = PAYLOAD.with(|p| {
                let mut p = p.borrow_mut();
                let (payload, _, next) = p.as_mut().unwrap();
                let values = payload.runs.get(*next).cloned().unwrap_or_default();
                *next += 1;
                values
            });
            returned(&values);
            Ok(values)
        }
        pub async fn rerun_workflow_run(
            &self,
            repo: &str,
            id: u64,
        ) -> ::std::result::Result<(), ::tl3_github::Error> {
            let outcome = PAYLOAD.with(|p| p.borrow().as_ref().unwrap().0.outcome);
            with(|r| r.api_writes += 1);
            mutation(
                "github.rerun",
                &format!("{repo}/{id}"),
                &serde_json::to_vec(&outcome).unwrap(),
                matches!(outcome, Outcome::Accepted),
                |_| {},
            );
            use ::tl3_github::{ActionsRefusal, Error};
            match outcome {
                Outcome::Accepted => Ok(()),
                Outcome::Http408 => Err(Error::Http { status: 408 }),
                Outcome::AppAccess => Err(Error::ActionsForbidden(ActionsRefusal::AppAccess)),
                Outcome::RunExpired => Err(Error::ActionsForbidden(ActionsRefusal::RunExpired)),
                Outcome::OtherRefusal => Err(Error::ActionsForbidden(ActionsRefusal::Other)),
            }
        }
    }
    #[derive(Debug)]
    pub(in crate::mergify_io) struct Side {
        pub result: Value,
        pub receipt: Option<Value>,
        pub posts: Vec<String>,
    }
    pub(in crate::mergify_io) struct Report {
        pub sides: [Side; 2],
        pub comparisons: usize,
        pub receipt_crashes: usize,
    }
    fn receipt(run: &Run) -> Option<Value> {
        run.files
            .iter()
            .find(|(path, _)| path.ends_with(".reruns"))
            .map(|(_, bytes)| serde_json::from_slice(bytes).unwrap())
    }
    fn record(
        p: &Payloads,
        new: bool,
        cut: Option<Cut>,
        execute: &impl Fn(bool, &Delivery) -> Value,
    ) -> Run {
        static HOOK: ::std::sync::Once = ::std::sync::Once::new();
        HOOK.call_once(|| {
            let original = ::std::panic::take_hook();
            ::std::panic::set_hook(Box::new(move |info| {
                if !info.payload().is::<Crash>() {
                    original(info);
                }
            }));
        });
        init(p.schedule.clone(), cut);
        PAYLOAD.with(|active| *active.borrow_mut() = Some((p.clone(), 0, 0)));
        let delivery = serde_json::from_value(delivery_value("enqueue-1")).unwrap();
        let result = match catch_unwind(AssertUnwindSafe(|| execute(new, &delivery))) {
            Ok(result) => result,
            Err(error) if error.is::<Crash>() => json!("crash"),
            Err(error) => ::std::panic::resume_unwind(error),
        };
        PAYLOAD.with(|active| *active.borrow_mut() = None);
        ACTIVE.with(|active| {
            let r = active.borrow_mut().take().unwrap();
            Run {
                calls: r.calls,
                effects: r.effects,
                files: r.files,
                result,
                writes: r.writes,
            }
        })
    }
    pub(in crate::mergify_io) fn compare(
        p: &Payloads,
        execute: impl Fn(bool, &Delivery) -> Value,
    ) -> Report {
        let old = record(p, false, None, &execute);
        let new = record(p, true, None, &execute);
        assert_eq!(old, new, "C1 payload divergence: {p:?}");
        let mut receipt_crashes = 0;
        let mut post_seen = false;
        let mutations: Vec<_> = old
            .calls
            .iter()
            .filter(|c| {
                matches!(
                    c.op.as_str(),
                    "file.create" | "file.write" | "file.fsync" | "file.rename" | "github.rerun"
                )
            })
            .collect();
        assert_eq!(mutations.len(), old.writes);
        for (write, call) in mutations.iter().enumerate() {
            if call.op == "github.rerun" {
                post_seen = true;
            }
            for after in [false, true] {
                let cut = Some(Cut { write, after });
                let a = record(p, false, cut, &execute);
                let b = record(p, true, cut, &execute);
                assert_eq!(a, b, "C1 payload crash divergence: {p:?}, {cut:?}");
                if post_seen && call.op == "file.rename" && call.args.ends_with(".reruns") {
                    assert_eq!(a.result, json!("crash"));
                    if after {
                        assert_eq!(
                            receipt(&a),
                            receipt(&old),
                            "receipt rename must publish final bytes"
                        );
                    } else {
                        let before = receipt(&a).expect("intent was persisted before POST");
                        assert_eq!(before[0]["accepted"], false);
                        assert_eq!(before[0]["waiting_for_completion"], false);
                    }
                    receipt_crashes += 1;
                }
            }
        }
        let side = |r: &Run| Side {
            result: r.result.clone(),
            receipt: receipt(r),
            posts: r
                .calls
                .iter()
                .filter(|c| c.op == "github.rerun")
                .map(|c| c.args.clone())
                .collect(),
        };
        Report {
            sides: [side(&old), side(&new)],
            comparisons: 1 + old.writes * 2,
            receipt_crashes,
        }
    }
    pub(in crate::mergify_io) fn workflow(
        id: u64,
        event: &str,
        conclusion: &str,
        attempt: u64,
    ) -> WorkflowRun {
        let mut run = run_value(&Frame {
            word: 0,
            elapsed_ms: 0,
        });
        run.id = id;
        run.event = event.into();
        run.conclusion = Some(conclusion.into());
        run.run_attempt = attempt;
        run
    }
}

// ctrl-core A2: configurable entry runner; the real old/current shells remain separate.
pub(super) fn a2_compare(s: &Schedule, execute: impl Fn(bool, &Delivery) -> Value) -> Value {
    // Install the existing crash-only panic hook before exercising custom entries.
    let _ = run(s, Entry::Read, false, None);
    let record = |new, cut| {
        init(s.clone(), cut);
        let d = serde_json::from_value(delivery_value("enqueue-1")).unwrap();
        let result = match catch_unwind(AssertUnwindSafe(|| execute(new, &d))) {
            Ok(value) => value,
            Err(error) if error.is::<Crash>() => json!("crash"),
            Err(error) => ::std::panic::resume_unwind(error),
        };
        ACTIVE.with(|active| {
            let r = active.borrow_mut().take().unwrap();
            Run {
                calls: r.calls,
                effects: r.effects,
                files: r.files,
                result,
                writes: r.writes,
            }
        })
    };
    let old = record(false, None);
    let new = record(true, None);
    assert_eq!(old, new, "A2 differential divergence: {s:?}");
    for write in 0..old.writes {
        for after in [false, true] {
            let cut = Some(Cut { write, after });
            assert_eq!(
                record(false, cut),
                record(true, cut),
                "A2 crash divergence: {s:?}, {cut:?}"
            );
        }
    }
    json!({
        "result":new.result,
        "calls":new.calls.iter().map(|c|json!({"op":c.op,"args":c.args})).collect::<Vec<_>>(),
        "files":new.files.iter().map(|(p,b)|(p.clone(),String::from_utf8_lossy(b).into_owned())).collect::<BTreeMap<_,_>>(),
        "comparisons":1+2*new.writes,
    })
}

// A2 initial durable state is installed before any shell operation, on both sides.
pub(super) fn a2_seed_heal(same_head: bool, reran: bool) {
    let key = "body-heal:owner/sandbox#100";
    let path = format!(
        "root/{}.body-heal",
        key.bytes().map(|b| format!("{b:02x}")).collect::<String>()
    );
    with(|r| {
        r.files.insert(
            path,
            serde_json::to_vec(&json!({"head":if same_head {"head"} else {"old"},"reran":reran}))
                .unwrap(),
        );
    });
}
