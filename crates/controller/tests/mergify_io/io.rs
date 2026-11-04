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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Profile {
    Plain,
    Flip,
    Label,
    Command,
    Waiting,
    Rerun,
}
#[derive(Clone, Copy, Debug)]
pub(super) enum Entry {
    Completion,
    Execute,
    Reconcile,
    Waiting,
    Command,
    Rerun,
}
#[derive(Clone, Debug)]
pub(super) struct Frame {
    pub word: u16,
    pub elapsed_ms: u32,
}
#[derive(Clone, Debug)]
pub(super) struct Schedule {
    pub profile: Profile,
    pub initial: u8,
    pub frames: Vec<Frame>,
    pub appears_at: usize,
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
    now_ms: u64,
    cut: Option<Cut>,
}
thread_local! { static ACTIVE: RefCell<Option<Recording>> = const { RefCell::new(None) }; }
fn with<T>(f: impl FnOnce(&mut Recording) -> T) -> T {
    ACTIVE.with(|v| f(v.borrow_mut().as_mut().expect("recorded I/O scope")))
}
fn read(op: &str, args: &str) -> Frame {
    with(|r| {
        let frame = r.schedule.frames.get(r.reads).cloned().unwrap_or(Frame {
            word: 0,
            elapsed_ms: 0,
        });
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
fn write(op: &str, args: &str, bytes: &[u8], change: impl FnOnce(&mut Recording)) {
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
        change(r);
        r.effects.push(call);
        if r.cut.is_some_and(|c| c.write == index && c.after) {
            ::std::panic::panic_any(Crash);
        }
    });
}
pub(super) fn has_store() -> bool {
    with(|r| r.schedule.profile == Profile::Waiting || r.schedule.initial & 1 != 0)
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
    json!({"before":null,"last_event":0,"requeue":if initial() & 2!=0 {"Sending"} else {"Ready"}})
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
    let d: Delivery=serde_json::from_value(json!({"key":"enqueue-1","epoch":1,"output":{"Effect":{"node":0,"seq":0,"kind":"Enqueue","head":null}},"generation":0,"head_sha":"head","repository":{"owner":"owner","name":"sandbox","installation":3},"issue":1,"pull_request":100,"paths":[],"attempt":0,"round":0})).unwrap();
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
    let writes = old.effects.len();
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
            let value = with(|r| r.files.get(&path.0).cloned());
            returned(&value);
            value.ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))
        }
        pub fn rename(from: PathBuf, to: &PathBuf) -> io::Result<()> {
            write("file.rename", &format!("{}->{}", from.0, to.0), &[], |r| {
                let b = r.files.remove(&from.0).unwrap();
                r.files.insert(to.0.clone(), b);
            });
            Ok(())
        }
        pub struct File(PathBuf);
        impl File {
            pub fn create(path: &PathBuf) -> io::Result<Self> {
                write("file.create", &path.0, &[], |r| {
                    r.files.insert(path.0.clone(), vec![]);
                });
                Ok(Self(path.clone()))
            }
            pub fn open(path: &PathBuf) -> io::Result<Self> {
                super::super::read("file.open", &path.0);
                Ok(Self(path.clone()))
            }
            pub fn sync_all(&self) -> io::Result<()> {
                write("file.fsync", &self.0.0, &[], |_| {});
                Ok(())
            }
        }
        impl io::Write for File {
            fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
                write("file.write", &self.0.0, bytes, |r| {
                    r.files.get_mut(&self.0.0).unwrap().extend(bytes);
                });
                Ok(bytes.len())
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
    }
}
#[derive(Clone)]
pub(super) struct Row {
    pub table: &'static str,
    pub id: String,
    pub value: Value,
}
#[derive(Clone)]
pub(super) struct Snapshot {
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
        read("store.open", &path.0);
        Ok(Self)
    }
    pub fn load(&self) -> Result<Snapshot> {
        let f = read("store.snapshot", "");
        let repaired = (profile() == Profile::Waiting) ^ (f.word & 4 != 0);
        Ok(Snapshot {
            rows: vec![Row {
                table: "nodes",
                id: "0".into(),
                value: json!({"held":f.word&8!=0,"paused":f.word&16!=0}),
            }],
            repaired,
        })
    }
    pub fn effects(&self) -> Result<Vec<Archive>> {
        read("store.effects", "");
        Ok(vec![])
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
pub(super) struct GitHub;
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
        return Err(::tl3_github::Error::Http {
            status: match (f.word >> 5) % 4 {
                0 => 403,
                1 => 404,
                2 => 422,
                _ => 500,
            },
        });
    };
    Ok(f)
}
fn pull(f: &Frame) -> PullRequest {
    serde_json::from_value(json!({"number":100,"user":{"id":1,"login":"factory[bot]"},"state":if f.word&1!=0 {"closed"}else{"open"},"draft":f.word&8!=0,"merged_at":if f.word&4!=0 {Some("2026-11-01T00:00:00Z")}else{None},"mergeable":true,"mergeable_state":"clean","head":{"sha":if f.word&2!=0 {"other"}else{"head"},"ref":"topic","repo":null},"base":{"sha":"base","ref":"main","repo":null},"updated_at":"2026-11-01T00:00:00Z"})).unwrap()
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
        let c = if f.word & 1 != 0 {
            vec![serde_json::from_value(json!({"id":1,"body":format!("@mergifyio queue\n{}",Self::marker("enqueue-1")),"user":{"id":1,"login":"factory[bot]"},"created_at":"2026-11-01T00:00:00Z","updated_at":"2026-11-01T00:00:00Z","issue_url":"unused"})).unwrap()]
        } else {
            vec![]
        };
        returned(&c);
        Ok(c)
    }
    pub async fn reconcile_comments(
        &self,
        repo: &str,
        n: u64,
        key: &str,
    ) -> ::std::result::Result<Option<Comment>, ::tl3_github::Error> {
        Ok(self
            .comments(repo, n)
            .await?
            .into_iter()
            .find(|c| c.body.contains(&Self::marker(key))))
    }
    pub async fn check_runs(
        &self,
        repo: &str,
        head: &str,
    ) -> ::std::result::Result<Vec<CheckRun>, ::tl3_github::Error> {
        let f = github_frame("github.checks", &format!("{repo}@{head}"))?;
        let c = vec![check(&f, profile() != Profile::Rerun)];
        returned(&c);
        Ok(c)
    }
    pub async fn required_status_checks(
        &self,
        repo: &str,
        branch: &str,
    ) -> ::std::result::Result<RequiredChecks, ::tl3_github::Error> {
        let f = github_frame("github.requirements", &format!("{repo}@{branch}"))?;
        Ok(RequiredChecks {
            checks: if (profile() == Profile::Rerun) ^ (f.word & 1 != 0) {
                vec![RequiredStatusCheck {
                    context: "CI".into(),
                    integration_id: Some(1),
                }]
            } else {
                vec![]
            },
            unavailable: vec![],
        })
    }
    pub async fn commit_statuses(
        &self,
        repo: &str,
        head: &str,
    ) -> ::std::result::Result<Vec<CommitStatus>, ::tl3_github::Error> {
        github_frame("github.statuses", &format!("{repo}@{head}"))?;
        Ok(vec![])
    }
    pub async fn check_annotations(
        &self,
        repo: &str,
        id: u64,
    ) -> ::std::result::Result<Vec<CheckAnnotation>, ::tl3_github::Error> {
        let f = github_frame("github.annotations", &format!("{repo}/{id}"))?;
        Ok(if f.word & 1 == 0 {
            vec![CheckAnnotation {
                annotation_level: "failure".into(),
                message: "The runner has lost communication with the server".into(),
            }]
        } else {
            vec![]
        })
    }
    pub async fn workflow_runs(
        &self,
        repo: &str,
        head: &str,
    ) -> ::std::result::Result<Vec<WorkflowRun>, ::tl3_github::Error> {
        let f = github_frame("github.runs", &format!("{repo}@{head}"))?;
        Ok(vec![run_value(&f)])
    }
    pub async fn workflow_run(
        &self,
        repo: &str,
        id: u64,
    ) -> ::std::result::Result<WorkflowRun, ::tl3_github::Error> {
        let f = github_frame("github.run", &format!("{repo}/{id}"))?;
        Ok(run_value(&f))
    }
    pub async fn add_label(
        &self,
        repo: &str,
        n: u64,
        label: &str,
    ) -> ::std::result::Result<(), ::tl3_github::Error> {
        api_write("github.add_label", &format!("{repo}#{n}"), label.as_bytes())
    }
    pub async fn post_command_comment(
        &self,
        repo: &str,
        n: u64,
        key: &str,
        body: &str,
    ) -> ::std::result::Result<(), ::tl3_github::Error> {
        api_write(
            "github.command",
            &format!("{repo}#{n}/{key}"),
            body.as_bytes(),
        )
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
    write(op, args, bytes, |_| {});
    Ok(())
}
pub(super) fn read_queue(
    github: &GitHub,
    d: &Delivery,
) -> Result<(tl3_mergify::Observation, Vec<Observation>)> {
    let repo = full_name(&d.repository);
    let state = Runtime
        .block_on(Mergify::new(github.clone()).read(&repo, 100, "head"))
        .map_err(adapter_error)?;
    Ok((state, vec![]))
}
