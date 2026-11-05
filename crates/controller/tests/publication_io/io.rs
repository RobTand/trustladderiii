//! One I/O recorder for both shells, the real atomic writer, and GitHub leaf calls.
use super::*;
use ::std::{
    cell::RefCell,
    io,
    panic::{AssertUnwindSafe, catch_unwind},
    sync::Arc,
    time::Duration,
};
use tl3_github::{
    CheckApp, CheckOutput, CheckRun, Comment, Issue, IssueActivity, NewCheckRun, NewPullRequest,
    PullRequest, UpdateCheckRun,
};
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub(super) enum Entry {
    Reconcile,
    Publish,
    Execute,
    Repair,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(super) enum Scenario {
    Approval,
    Duplicates,
    OpenPr,
    ClosePr,
    Correction,
    Restart,
    Console,
    Clarify,
    Replacement,
    Incident,
    WrongIncident,
    RestartDropped,
    SummaryMerged,
    SummaryDuplicate,
    Plain,
    EvidencePermanent,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct Override {
    pub op: String,
    pub nth: usize,
    pub word: u16,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct Schedule {
    pub scenario: Scenario,
    pub reads: Vec<u16>,
    pub api_results: Vec<u8>,
    pub file_results: Vec<u8>,
    pub overrides: Vec<Override>,
    pub fence_epoch: u8,
    pub owner_epoch: u8,
    pub effect_epoch: u8,
    pub initial_attempt: Option<u8>,
    pub initial_comments: u8,
}
impl Schedule {
    pub fn stable(scenario: Scenario) -> Self {
        Self {
            scenario,
            reads: vec![],
            api_results: vec![],
            file_results: vec![],
            overrides: vec![],
            fence_epoch: 1,
            owner_epoch: 1,
            effect_epoch: 1,
            initial_attempt: None,
            initial_comments: 0,
        }
    }
    pub fn over(&mut self, op: &str, nth: usize, word: u16) {
        self.overrides.push(Override {
            op: op.into(),
            nth,
            word,
        });
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Call {
    pub op: String,
    pub args: String,
    pub response: Vec<u8>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Run {
    pub calls: Vec<Call>,
    pub effects: Vec<Call>,
    pub files: BTreeMap<String, Vec<u8>>,
    pub upstream: BTreeMap<String, Value>,
    pub result: Value,
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
    upstream: BTreeMap<String, Value>,
    reads: usize,
    writes: usize,
    file_writes: usize,
    api_writes: usize,
    cut: Option<Cut>,
}
thread_local! { static ACTIVE: RefCell<Option<Recording>> = const { RefCell::new(None) }; }
fn with<T>(f: impl FnOnce(&mut Recording) -> T) -> T {
    ACTIVE.with(|r| f(r.borrow_mut().as_mut().expect("recorded I/O scope")))
}
fn read(op: &str, args: &str) -> u16 {
    with(|r| {
        let nth = r.calls.iter().filter(|c| c.op == op).count();
        let word = r
            .schedule
            .overrides
            .iter()
            .find(|o| o.op == op && o.nth == nth)
            .map(|o| o.word)
            .unwrap_or_else(|| r.schedule.reads.get(r.reads).copied().unwrap_or(0));
        r.reads += 1;
        r.calls.push(Call {
            op: op.into(),
            args: args.into(),
            response: vec![],
        });
        word
    })
}
fn returned(value: impl Serialize) {
    with(|r| r.calls.last_mut().unwrap().response = serde_json::to_vec(&value).unwrap());
}
fn mutation(op: &str, args: &str, bytes: &[u8], landed: bool, change: impl FnOnce(&mut Recording)) {
    with(|r| {
        let index = r.writes;
        r.writes += 1;
        let call = Call {
            op: op.into(),
            args: args.into(),
            response: bytes.to_vec(),
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
fn file_write(
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
            .response
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
fn api_write(op: &str, args: &str, body: &Value, change: impl FnOnce(&mut Recording)) -> u8 {
    let mode = with(|r| {
        let m = r
            .schedule
            .api_results
            .get(r.api_writes)
            .copied()
            .unwrap_or(0);
        r.api_writes += 1;
        m
    });
    mutation(
        op,
        args,
        &serde_json::to_vec(&(body, mode)).unwrap(),
        matches!(mode, 0 | 5 | 10 | 11),
        change,
    );
    mode
}
fn api_error(mode: u8) -> Option<tl3_github::Error> {
    use tl3_github::Error::*;
    match mode {
        0 | 11 => None,
        1 => Some(Http { status: 403 }),
        2 | 9 => Some(Http { status: 404 }),
        3 => Some(Http { status: 422 }),
        4 => Some(RateLimited {
            retry_after: Duration::from_secs(17),
        }),
        5..=7 => Some(Transport),
        8 => Some(Http { status: 500 }),
        _ => Some(Response),
    }
}
fn gh_read(op: &str, args: &str) -> ::std::result::Result<u16, tl3_github::Error> {
    let w = read(op, args);
    let err = match w >> 12 {
        1 => Some(tl3_github::Error::Http { status: 404 }),
        2 => Some(tl3_github::Error::Http { status: 403 }),
        3 => Some(tl3_github::Error::Http { status: 422 }),
        4 => Some(tl3_github::Error::RateLimited {
            retry_after: Duration::from_secs(17),
        }),
        5 => Some(tl3_github::Error::Transport),
        6 => Some(tl3_github::Error::Response),
        7 => Some(tl3_github::Error::Http { status: 500 }),
        _ => None,
    };
    if let Some(e) = err {
        returned(e.to_string());
        Err(e)
    } else {
        Ok(w)
    }
}
pub(super) fn owner_epoch() -> i64 {
    with(|r| i64::from(r.schedule.owner_epoch))
}
pub(super) fn operations() -> Option<tl3_config::OperationsIssue> {
    Some(tl3_config::OperationsIssue {
        owner: "OWNER".into(),
        name: "Sandbox".into(),
        installation_id: 3,
        number: 12,
    })
}
fn scenario() -> Scenario {
    with(|r| r.schedule.scenario)
}
fn attempt_path() -> String {
    let key = format!("publication-1:{}", "a".repeat(40));
    format!(
        "root/{}.approval",
        key.bytes().map(|b| format!("{b:02x}")).collect::<String>()
    )
}
fn delivery() -> Delivery {
    let s = scenario();
    let output = match s {
        Scenario::Approval | Scenario::Duplicates => {
            json!({"Effect":{"node":0,"seq":0,"kind":"PostApproval","head":null}})
        }
        Scenario::OpenPr | Scenario::EvidencePermanent => {
            json!({"Effect":{"node":0,"seq":0,"kind":"OpenPr","head":null}})
        }
        Scenario::ClosePr => json!({"Effect":{"node":0,"seq":0,"kind":"ClosePr","head":null}}),
        Scenario::Incident | Scenario::WrongIncident => {
            json!({"Notice":{"node":0,"notice":{"Incident":{"needs_rob":true}}}})
        }
        Scenario::SummaryMerged => json!({"Notice":{"node":0,"notice":{"Summary":"Merged"}}}),
        Scenario::SummaryDuplicate => json!({"Notice":{"node":0,"notice":{"Summary":"Duplicate"}}}),
        _ => {
            json!({"Comment":{"node":0,"kind":match s {Scenario::Clarify=>"Clarify",Scenario::Replacement|Scenario::RestartDropped=>"ReplacementDropped",_=>"Classify"}}})
        }
    };
    let steering = match s {
        Scenario::Correction => {
            json!({"FileRequest":{"text":"correction","comment":7,"correction":true,"revert":true,"since":"2026-11-01T00:00:00Z","comment_issue":12}})
        }
        Scenario::Restart => {
            json!({"Restart":{"comment_key":"restart-key","text":"restart","issue":null}})
        }
        Scenario::Console => json!({"ConsoleNote":{"text":"The request was paused."}}),
        Scenario::Clarify => {
            json!({"ClarifyComment":{"question":"Which change?","excerpt":"make it faster"}})
        }
        Scenario::Replacement => {
            json!({"ReplacementDropped":{"text":"change it","outcome":"Merged"}})
        }
        Scenario::RestartDropped => json!({"RestartDropped":{"text":"restart"}}),
        _ => Value::Null,
    };
    serde_json::from_value(json!({"key":"publication-1","epoch":with(|r|r.schedule.effect_epoch),"output":output,"generation":0,"head_sha":"a".repeat(40),"repository":{"owner":"owner","name":"sandbox","installation":3},"issue":if s==Scenario::WrongIncident {13}else{12},"pull_request":100,"paths":[],"attempt":0,"round":0,"steering":steering,"request_text":if s==Scenario::EvidencePermanent {Some("publish proof with red/green")}else{None},"question_context":{"issue_title":"Original request","request":"[Original request](https://github.com/owner/requests/issues/7)","incident_id":"0.1"}})).unwrap()
}
fn init(schedule: Schedule, cut: Option<Cut>) {
    let mut files = BTreeMap::new();
    files.insert(
        "root/epoch.fence".into(),
        serde_json::to_vec(&schedule.fence_epoch).unwrap(),
    );
    let mut upstream = BTreeMap::new();
    if schedule.scenario == Scenario::Duplicates {
        for id in 1..=3 {
            upstream.insert(
                format!("check:{id}"),
                serde_json::to_value(check(id, false)).unwrap(),
            );
        }
    }
    for id in 0..schedule.initial_comments {
        upstream.insert(
            format!("comment:{id}"),
            serde_json::to_value(comment(
                u64::from(id),
                &format!("A message\n\n{}", GitHub::marker("publication-1")),
            ))
            .unwrap(),
        );
    }
    ACTIVE.with(|r| {
        *r.borrow_mut() = Some(Recording {
            schedule,
            calls: vec![],
            effects: vec![],
            files,
            upstream,
            reads: 0,
            writes: 0,
            file_writes: 0,
            api_writes: 0,
            cut,
        })
    });
    if let Some(state) = with(|r| r.schedule.initial_attempt) {
        let path = attempt_path();
        with(|r| {
            r.files.insert(
                path,
                serde_json::to_vec(
                    [
                        "Preflight",
                        "Dispatching",
                        "Observed",
                        "Uncertain",
                        "Refused",
                    ][usize::from(state) % 5],
                )
                .unwrap(),
            );
        });
    }
}
fn run(s: &Schedule, e: Entry, new: bool, cut: Option<Cut>) -> Run {
    static HOOK: ::std::sync::Once = ::std::sync::Once::new();
    HOOK.call_once(|| {
        let original = ::std::panic::take_hook();
        ::std::panic::set_hook(Box::new(move |i| {
            if !i.payload().is::<Crash>() {
                original(i)
            }
        }));
    });
    init(s.clone(), cut);
    let d = delivery();
    let result = match catch_unwind(AssertUnwindSafe(|| {
        if new {
            super::current::run(e, &d)
        } else {
            super::reference::run(e, &d)
        }
    })) {
        Ok(v) => v,
        Err(e) if e.is::<Crash>() => json!("crash"),
        Err(e) => ::std::panic::resume_unwind(e),
    };
    ACTIVE.with(|r| {
        let r = r.borrow_mut().take().unwrap();
        Run {
            calls: r.calls,
            effects: r.effects,
            files: r.files,
            upstream: r.upstream,
            result,
            writes: r.writes,
        }
    })
}
pub(super) fn observe(s: &Schedule, e: Entry) -> Run {
    run(s, e, false, None)
}
pub(super) fn compare(s: &Schedule, e: Entry) -> ::std::result::Result<usize, String> {
    let old = run(s, e, false, None);
    let new = run(s, e, true, None);
    if old != new {
        return Err(format!(
            "I/O schedule divergence: {s:?} {e:?}\nold={old:#?}\nnew={new:#?}"
        ));
    }
    for write in 0..old.writes {
        for after in [false, true] {
            let cut = Some(Cut { write, after });
            let old = run(s, e, false, cut);
            let new = run(s, e, true, cut);
            if old != new {
                return Err(format!(
                    "Crash divergence: {s:?} {e:?} {cut:?}\nold={old:#?}\nnew={new:#?}"
                ));
            }
        }
    }
    Ok(1 + 2 * old.writes)
}
pub(super) fn reconciliation(r: Result<Reconciliation>) -> Value {
    match r {
        Ok(Reconciliation::Found(c)) => json!({"found":c}),
        Ok(Reconciliation::Absent) => json!("Absent"),
        Ok(Reconciliation::Pending) => json!("Pending"),
        Err(e) => json!({"error":e.to_string()}),
    }
}
pub(super) fn completed(r: Result<Completion>) -> Value {
    match r {
        Ok(c) => json!({"completion":c}),
        Err(e) => json!({"error":e.to_string()}),
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PathBuf(String);
pub(crate) type Path = PathBuf;
impl PathBuf {
    pub fn from(p: &str) -> Self {
        Self(p.into())
    }
    pub fn to_owned(&self) -> Self {
        self.clone()
    }
    pub fn join(&self, p: impl AsRef<str>) -> Self {
        Self(format!("{}/{}", self.0, p.as_ref()))
    }
    pub fn parent(&self) -> Option<Self> {
        self.0.rsplit_once('/').map(|(p, _)| Self(p.into()))
    }
    pub fn with_extension(&self, ext: &str) -> Self {
        Self(format!(
            "{}.{ext}",
            self.0.rsplit_once('.').map_or(self.0.as_str(), |(p, _)| p)
        ))
    }
    pub fn exists(&self) -> bool {
        let w = read("file.exists", &self.0);
        let v = with(|r| r.files.contains_key(&self.0)) ^ (w & 1 != 0);
        returned(v);
        v
    }
}
pub(super) mod mock_std {
    pub use ::std::*;
    pub mod path {
        pub(crate) use super::super::{Path, PathBuf};
    }
    pub mod fs {
        use super::super::*;
        pub fn read(p: impl ::std::borrow::Borrow<PathBuf>) -> io::Result<Vec<u8>> {
            let p = p.borrow();
            let w = super::super::read("file.read", &p.0);
            let value = if w & 1024 != 0 {
                Err(io::ErrorKind::PermissionDenied)
            } else if w & 2048 != 0 {
                Err(io::ErrorKind::Other)
            } else if w & 4096 != 0 {
                Ok(b"invalid json".to_vec())
            } else if w & 1 != 0 {
                Err(io::ErrorKind::NotFound)
            } else if w & 2 != 0 && p.0.ends_with(".fence") {
                Ok(serde_json::to_vec(&((w >> 2) % 4)).unwrap())
            } else if w & 2 != 0 && p.0.ends_with(".approval") {
                Ok(serde_json::to_vec(
                    [
                        "Preflight",
                        "Dispatching",
                        "Observed",
                        "Uncertain",
                        "Refused",
                    ][usize::from(w >> 2) % 5],
                )
                .unwrap())
            } else {
                with(|r| r.files.get(&p.0).cloned()).ok_or(io::ErrorKind::NotFound)
            };
            returned(value.as_ref().map_err(|e| format!("{e:?}")));
            value.map_err(io::Error::from)
        }
        pub fn rename(from: PathBuf, to: &PathBuf) -> io::Result<()> {
            file_write("file.rename", &format!("{}->{}", from.0, to.0), &[], |r| {
                let b = r.files.remove(&from.0).unwrap();
                r.files.insert(to.0.clone(), b);
            })
        }
        pub fn remove_file(p: &PathBuf) -> io::Result<()> {
            file_write("file.remove", &p.0, &[], |r| {
                r.files.remove(&p.0);
            })
        }
        pub struct File(PathBuf);
        impl File {
            pub fn create(p: &PathBuf) -> io::Result<Self> {
                file_write("file.create", &p.0, &[], |r| {
                    r.files.insert(p.0.clone(), vec![]);
                })?;
                Ok(Self(p.clone()))
            }
            pub fn open(p: impl ::std::borrow::Borrow<PathBuf>) -> io::Result<Self> {
                let p = p.borrow();
                let w = super::super::read("file.open", &p.0);
                returned(w & 1 == 0);
                if w & 1 == 0 {
                    Ok(Self(p.clone()))
                } else {
                    Err(io::ErrorKind::PermissionDenied.into())
                }
            }
            pub fn sync_all(&self) -> io::Result<()> {
                file_write("file.fsync", &self.0.0, &[], |_| {})
            }
        }
        impl io::Write for File {
            fn write(&mut self, b: &[u8]) -> io::Result<usize> {
                file_write("file.write", &self.0.0, b, |r| {
                    r.files.get_mut(&self.0.0).unwrap().extend_from_slice(b)
                })?;
                Ok(b.len())
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
    }
}
pub(super) struct Runtime;
impl Runtime {
    pub fn block_on<T>(&self, f: impl ::std::future::Future<Output = T>) -> T {
        let mut f = ::std::pin::pin!(f);
        let mut cx = ::std::task::Context::from_waker(::std::task::Waker::noop());
        match f.as_mut().poll(&mut cx) {
            ::std::task::Poll::Ready(v) => v,
            _ => panic!("recorded call yielded"),
        }
    }
}
pub(super) struct RecordedWriter;
impl Writer for RecordedWriter {
    fn draft(&mut self, key: &str, input: &WriteRequest) -> Result<WriterReply> {
        let w = read(
            "writer.draft",
            &json!({"key":key,"input":input}).to_string(),
        );
        if w & 1 != 0 {
            returned("rejected");
            return Err(Error::MessageRejected);
        }
        if w & 2 != 0 {
            returned("failed");
            return Err(Error::Invalid("writer unavailable"));
        }
        let reply = WriterReply::Text(TextDraft {
            text: "Could you explain what you mean by \"make it faster\"?".into(),
        });
        returned(&reply);
        Ok(reply)
    }
}
fn check(id: u64, approved: bool) -> CheckRun {
    serde_json::from_value(json!({"id":id,"name":"TL3 approval","head_sha":"a".repeat(40),"external_id":format!("publication-1:{}","a".repeat(40)),"app":{"id":9,"slug":"factory"},"status":"completed","conclusion":if approved {"success"}else{"failure"},"output":{"title":"TL3 approval","summary":if approved {"This head passed TL3's required checks and independent review."}else{"old output"},"text":""}})).unwrap()
}
fn comment(id: u64, body: &str) -> Comment {
    serde_json::from_value(json!({"id":id,"body":body,"user":{"id":9,"login":"factory[bot]"},"created_at":"2026-11-01T00:00:00Z","updated_at":"2026-11-01T00:00:00Z","issue_url":"https://api.github.com/repos/owner/sandbox/issues/12"})).unwrap()
}
fn pull(number: u64, w: u16) -> PullRequest {
    serde_json::from_value(json!({"number":number,"user":{"id":9,"login":"factory[bot]"},"state":if w&1!=0 {"closed"}else{"open"},"draft":w&8!=0,"merged_at":if w&2!=0 {Some("2026-11-01T00:00:00Z")}else{None},"mergeable":w&4==0,"body":if w&16!=0 {""}else{"Change type: fix"},"head":{"sha":"a".repeat(40),"ref":if w&32!=0 {"foreign"}else{"topic"},"repo":{"id":1,"owner":{"id":1,"login":"owner"},"name":"sandbox","full_name":"owner/sandbox"}},"base":{"sha":"base","ref":"main","repo":null},"updated_at":"2026-11-01T00:00:00Z"})).unwrap()
}
#[derive(Clone)]
pub(super) struct GitHub(Arc<GitHubInner>);
struct GitHubInner {
    slug: String,
    comments: tokio::sync::Mutex<()>,
}
impl GitHub {
    pub fn new() -> Self {
        Self(Arc::new(GitHubInner {
            slug: "factory".into(),
            comments: tokio::sync::Mutex::new(()),
        }))
    }
    pub fn app_id(&self) -> u64 {
        9
    }
    pub fn app_slug(&self) -> &str {
        "factory"
    }
    pub fn marker(k: &str) -> String {
        tl3_github::GitHub::marker(k)
    }
    pub async fn check_runs(
        &self,
        repo: &str,
        head: &str,
    ) -> ::std::result::Result<Vec<CheckRun>, tl3_github::Error> {
        let w = gh_read("github.check_runs", &format!("{repo}@{head}"))?;
        let mut cs = with(|r| {
            r.upstream
                .iter()
                .filter(|(k, _)| k.starts_with("check:"))
                .map(|(_, v)| serde_json::from_value(v.clone()).unwrap())
                .collect::<Vec<CheckRun>>()
        });
        if w & 1 != 0 {
            cs.clear();
        }
        if w & 2 != 0 {
            cs.push(check(100, false));
        }
        if w & 4 != 0 {
            cs.push(check(101, true));
        }
        if w & 8 != 0 {
            let mut c = check(102, false);
            c.app.as_mut().unwrap().id = 77;
            cs.push(c);
        }
        returned(&cs);
        Ok(cs)
    }
    pub async fn post_check_run_tracked(
        &self,
        repo: &str,
        new: &NewCheckRun,
        output: Option<&CheckOutput>,
    ) -> ::std::result::Result<CheckRun, tl3_github::PostFailure> {
        let body = json!({"check":new,"output":output});
        let c = check(200, true);
        let mode = api_write("github.post_check", repo, &body, |r| {
            r.upstream
                .insert("check:200".into(), serde_json::to_value(&c).unwrap());
        });
        if let Some(e) = api_error(mode) {
            return Err(tl3_github::PostFailure {
                error: e,
                sent: !matches!(mode, 6 | 9),
            });
        }
        Ok(if mode == 11 { check(200, false) } else { c })
    }
    pub async fn update_check_run(
        &self,
        repo: &str,
        id: u64,
        update: &UpdateCheckRun,
    ) -> ::std::result::Result<CheckRun, tl3_github::Error> {
        let c = check(id, true);
        let mode = api_write(
            "github.patch_check",
            &format!("{repo}#{id}"),
            &json!(update),
            |r| {
                r.upstream.insert(format!("check:{id}"), json!(c));
            },
        );
        match api_error(mode) {
            Some(e) => Err(e),
            None => Ok(if mode == 11 { check(id, false) } else { c }),
        }
    }
    pub async fn issue(
        &self,
        repo: &str,
        n: u64,
    ) -> ::std::result::Result<Issue, tl3_github::Error> {
        let w = gh_read("github.issue", &format!("{repo}#{n}"))?;
        let i=serde_json::from_value(json!({"number":n,"title":"Current issue title","body":"Current issue text","state":if w&1!=0 {"closed"}else{"open"},"updated_at":"2026-11-01T00:00:00Z","labels":[]})).unwrap();
        returned(&i);
        Ok(i)
    }
    pub async fn reconcile_issue(
        &self,
        repo: &str,
        key: &str,
        since: &str,
    ) -> ::std::result::Result<Option<Issue>, tl3_github::Error> {
        let w = gh_read("github.reconcile_issue", &format!("{repo}/{key}/{since}"))?;
        let i = if w & 1 != 0 {
            Some(serde_json::from_value(json!({"number":99,"title":"Correction","state":"open","updated_at":"2026-11-01T00:00:00Z","labels":[]})).unwrap())
        } else {
            None
        };
        returned(&i);
        Ok(i)
    }
    pub async fn issue_events(
        &self,
        repo: &str,
        n: u64,
    ) -> ::std::result::Result<Vec<IssueActivity>, tl3_github::Error> {
        let w = gh_read("github.events", &format!("{repo}#{n}"))?;
        let es = if w & 1 == 0 {
            vec![serde_json::from_value(json!({"id":1,"event":"closed","actor":{"id":9,"login":"factory[bot]"},"created_at":"2026-11-01T00:00:00Z"})).unwrap()]
        } else {
            vec![]
        };
        returned(&es);
        Ok(es)
    }
    pub async fn pull_requests_for_branch(
        &self,
        repo: &str,
        owner: &str,
        branch: &str,
    ) -> ::std::result::Result<Vec<PullRequest>, tl3_github::Error> {
        let w = gh_read("github.branch_prs", &format!("{repo}/{owner}/{branch}"))?;
        let ps = if w & 64 != 0 {
            vec![]
        } else {
            vec![pull(100, w)]
        };
        returned(&ps);
        Ok(ps)
    }
    pub async fn pull_request(
        &self,
        repo: &str,
        n: u64,
    ) -> ::std::result::Result<PullRequest, tl3_github::Error> {
        let w = gh_read("github.pull", &format!("{repo}#{n}"))?;
        let p = pull(n, w);
        returned(&p);
        Ok(p)
    }
    pub async fn close_issue(
        &self,
        repo: &str,
        n: u64,
        reason: CloseReason,
    ) -> ::std::result::Result<(), tl3_github::Error> {
        let mode = api_write(
            "github.close_issue",
            &format!("{repo}#{n}"),
            &json!(reason),
            |r| {
                r.upstream.insert(format!("issue:{n}"), json!("closed"));
            },
        );
        api_error(mode).map_or(Ok(()), Err)
    }
    pub async fn reopen_issue(
        &self,
        repo: &str,
        n: u64,
    ) -> ::std::result::Result<Issue, tl3_github::Error> {
        let mode = api_write(
            "github.reopen_issue",
            &format!("{repo}#{n}"),
            &json!(null),
            |r| {
                r.upstream.insert(format!("issue:{n}"), json!("open"));
            },
        );
        if let Some(e) = api_error(mode) {
            return Err(e);
        }
        Ok(serde_json::from_value(json!({"number":n,"title":"Reopened title","body":"request","state":"open","updated_at":"2026-11-01T00:00:00Z","labels":[]})).unwrap())
    }
    pub async fn create_request_issue(
        &self,
        repo: &str,
        key: &str,
        since: &str,
        title: &str,
        body: &str,
    ) -> ::std::result::Result<Issue, tl3_github::Error> {
        let mode = api_write(
            "github.create_issue",
            &format!("{repo}/{key}/{since}"),
            &json!({"title":title,"body":body}),
            |r| {
                r.upstream
                    .insert("issue:99".into(), json!({"title":title,"body":body}));
            },
        );
        if let Some(e) = api_error(mode) {
            return Err(e);
        }
        Ok(serde_json::from_value(json!({"number":99,"title":title,"body":body,"state":"open","updated_at":"2026-11-01T00:00:00Z","labels":[]})).unwrap())
    }
    pub async fn close_pull_request(
        &self,
        repo: &str,
        n: u64,
    ) -> ::std::result::Result<PullRequest, tl3_github::Error> {
        let mode = api_write(
            "github.close_pr",
            &format!("{repo}#{n}"),
            &json!(null),
            |r| {
                r.upstream.insert(format!("pull:{n}"), json!("closed"));
            },
        );
        if let Some(e) = api_error(mode) {
            return Err(e);
        }
        Ok(pull(n, 1))
    }
    pub async fn reopen_pull_request(
        &self,
        repo: &str,
        n: u64,
    ) -> ::std::result::Result<PullRequest, tl3_github::Error> {
        let mode = api_write(
            "github.reopen_pr",
            &format!("{repo}#{n}"),
            &json!(null),
            |r| {
                r.upstream.insert(format!("pull:{n}"), json!("open"));
            },
        );
        if let Some(e) = api_error(mode) {
            return Err(e);
        }
        Ok(pull(n, 0))
    }
    pub async fn update_pull_request_body(
        &self,
        repo: &str,
        n: u64,
        body: &str,
    ) -> ::std::result::Result<PullRequest, tl3_github::Error> {
        let mode = api_write(
            "github.patch_body",
            &format!("{repo}#{n}"),
            &json!(body),
            |r| {
                r.upstream.insert(format!("body:{n}"), json!(body));
            },
        );
        if let Some(e) = api_error(mode) {
            return Err(e);
        }
        let mut p = pull(n, 0);
        p.body = Some(body.into());
        Ok(p)
    }
    pub async fn open_pull_request(
        &self,
        repo: &str,
        new: &NewPullRequest,
    ) -> ::std::result::Result<PullRequest, tl3_github::Error> {
        let mode = api_write("github.open_pr", repo, &json!(new), |r| {
            r.upstream.insert("pull:100".into(), json!(new));
        });
        if let Some(e) = api_error(mode) {
            return Err(e);
        }
        let mut p = pull(100, 0);
        p.body = Some(new.body.clone());
        Ok(p)
    }
    pub async fn comments(
        &self,
        repo: &str,
        n: u64,
    ) -> ::std::result::Result<Vec<Comment>, tl3_github::Error> {
        let w = gh_read("github.comments", &format!("{repo}#{n}"))?;
        let mut cs = with(|r| {
            r.upstream
                .iter()
                .filter(|(k, _)| k.starts_with("comment:"))
                .map(|(_, v)| serde_json::from_value(v.clone()).unwrap())
                .collect::<Vec<Comment>>()
        });
        if w & 1 != 0 {
            cs.push(comment(
                100,
                &format!("A message\n\n{}", Self::marker("publication-1")),
            ));
        }
        returned(&cs);
        Ok(cs)
    }
    async fn request<T: serde::de::DeserializeOwned>(
        &self,
        method: reqwest::Method,
        path: String,
        _query: Vec<(String, String)>,
        body: Option<Value>,
    ) -> ::std::result::Result<T, tl3_github::Error> {
        let id = 300;
        let c = comment(
            id,
            body.as_ref().and_then(|b| b["body"].as_str()).unwrap_or(""),
        );
        let op = if method == reqwest::Method::DELETE {
            "github.delete_comment"
        } else {
            "github.post_comment"
        };
        let mode = api_write(op, &path, &json!(body), |r| {
            if method == reqwest::Method::DELETE {
                let id = path.rsplit('/').next().unwrap();
                r.upstream.remove(&format!("comment:{id}"));
            } else {
                r.upstream.insert(format!("comment:{id}"), json!(c));
            }
        });
        if let Some(e) = api_error(mode) {
            return Err(e);
        }
        serde_json::from_value(if method == reqwest::Method::DELETE {
            json!(null)
        } else {
            json!(c)
        })
        .map_err(|_| tl3_github::Error::Response)
    }
}
mod comments {
    use super::*;
    include!("comments.rs");
}
