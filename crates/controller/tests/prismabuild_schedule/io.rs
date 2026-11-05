//! One scripted interface used by both byte-identical source replays.
#![allow(dead_code)]
pub use super::decide;
pub use super::tl3_store;
pub use executor::{Completion, Delivery, Observation, Output as Out};
pub use serde::{Deserialize, Serialize};
use std::{cell::RefCell, collections::BTreeSet};
pub use std::{collections::BTreeMap, ffi::OsString, io::Write};
pub use tl3_controller::adapters::prismabuild::CheckRun;
pub use tl3_controller::{Error, Result, executor};
pub use tl3_kernel::{EffectKind, NodeId};
pub use tl3_prismabuild::{ActionKey, ActionState, Demand, ProgressPhase, RecordRead, Tool};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct State {
    pub calls: Vec<String>,
    pub files: BTreeMap<String, Vec<u8>>,
    pub synced: BTreeMap<String, Vec<u8>>,
    pub persisted: BTreeMap<String, Vec<u8>>,
    pub reads: usize,
    pub writes: usize,
    pub script: Vec<u8>,
    pub crash: usize,
}
thread_local! { pub static IO: RefCell<State> = RefCell::new(State {
    calls: vec![], files: BTreeMap::new(), synced: BTreeMap::new(),
    persisted: BTreeMap::new(), reads: 0, writes: 0, script: vec![0], crash: usize::MAX,
}); }
#[derive(Debug)]
pub struct Crash;
pub fn call(name: impl Into<String>) {
    IO.with(|io| io.borrow_mut().calls.push(name.into()));
}
pub fn sample(name: impl Into<String>) -> u8 {
    call(name);
    IO.with(|io| {
        let mut io = io.borrow_mut();
        let value = io.script[io.reads % io.script.len()];
        io.reads += 1;
        value
    })
}
pub fn mutation(name: impl Into<String>, change: impl FnOnce(&mut State)) {
    call(name);
    IO.with(|io| {
        let mut io = io.borrow_mut();
        // Before and after each write are distinct crash points.
        if io.crash == io.writes * 2 {
            std::panic::panic_any(Crash);
        }
        change(&mut io);
        io.writes += 1;
        if io.crash == io.writes * 2 - 1 {
            std::panic::panic_any(Crash);
        }
    });
}
fn missing() -> std::io::Error {
    std::io::ErrorKind::NotFound.into()
}
fn fault() -> std::io::Error {
    std::io::ErrorKind::PermissionDenied.into()
}

#[derive(Clone, Debug)]
pub struct VPath {
    path: std::path::PathBuf,
    parent: Option<Box<VPath>>,
}
pub type Path = VPath;
pub type PathBuf = VPath;
impl VPath {
    pub fn new(path: impl Into<std::path::PathBuf>) -> Self {
        let path = path.into();
        let parent = path
            .parent()
            .filter(|p| *p != path)
            .map(|p| Box::new(Self::new(p)));
        Self { path, parent }
    }
    pub fn join(&self, child: impl AsRef<std::path::Path>) -> Self {
        Self::new(self.path.join(child))
    }
    pub fn with_extension(&self, ext: &str) -> Self {
        Self::new(self.path.with_extension(ext))
    }
    pub fn parent(&self) -> Option<&Self> {
        self.parent.as_deref()
    }
    pub fn extension(&self) -> Option<&std::ffi::OsStr> {
        self.path.extension()
    }
    pub fn display(&self) -> std::path::Display<'_> {
        self.path.display()
    }
    pub fn name(&self) -> String {
        self.path.to_string_lossy().into_owned()
    }
    pub fn exists(&self) -> bool {
        self.try_exists().unwrap_or(false)
    }
    pub fn try_exists(&self) -> std::io::Result<bool> {
        let n = sample(format!("exists:{}", self.name()));
        if n == 254 {
            return Err(fault());
        }
        if n == 253 {
            return Ok(false);
        }
        Ok(IO.with(|io| io.borrow().files.contains_key(&self.name())))
    }
}
impl AsRef<VPath> for VPath {
    fn as_ref(&self) -> &VPath {
        self
    }
}
#[derive(Clone)]
pub struct Entry(VPath);
impl Entry {
    pub fn path(&self) -> VPath {
        call(format!("entry.path:{}", self.0.name()));
        self.0.clone()
    }
    pub fn file_type(&self) -> std::io::Result<FileType> {
        if sample(format!("entry.type:{}", self.0.name())) == 254 {
            return Err(fault());
        }
        Ok(FileType(self.0.extension().is_none()))
    }
    pub fn file_name(&self) -> OsString {
        call(format!("entry.name:{}", self.0.name()));
        self.0.path.file_name().unwrap().to_owned()
    }
}
pub struct FileType(bool);
impl FileType {
    pub fn is_dir(&self) -> bool {
        self.0
    }
}
pub mod fs {
    use super::*;
    pub fn read(path: impl AsRef<Path>) -> std::io::Result<Vec<u8>> {
        let path = path.as_ref().name();
        let n = sample(format!("read:{path}"));
        match n {
            254 => Err(fault()),
            253 => Err(missing()),
            252 => Ok(b"invalid json".to_vec()),
            251 if path.ends_with("epoch.fence") => Ok(b"99".to_vec()),
            _ => IO.with(|io| io.borrow().files.get(&path).cloned().ok_or_else(missing)),
        }
    }
    pub fn create_dir_all(path: impl AsRef<Path>) -> std::io::Result<()> {
        let path = path.as_ref().name();
        mutation(format!("mkdir:{path}"), |io| {
            io.files.entry(path).or_default();
        });
        Ok(())
    }
    pub fn remove_file(path: impl AsRef<Path>) -> std::io::Result<()> {
        remove(path, false)
    }
    pub fn remove_dir_all(path: impl AsRef<Path>) -> std::io::Result<()> {
        remove(path, true)
    }
    fn remove(path: impl AsRef<Path>, recursive: bool) -> std::io::Result<()> {
        let path = path.as_ref().name();
        let exists = IO.with(|io| {
            io.borrow()
                .files
                .keys()
                .any(|k| k == &path || recursive && k.starts_with(&format!("{path}/")))
        });
        if !exists {
            call(format!("remove:{recursive}:{path}"));
            return Err(missing());
        }
        mutation(format!("remove:{recursive}:{path}"), |io| {
            io.files
                .retain(|k, _| !(k == &path || recursive && k.starts_with(&format!("{path}/"))));
            io.synced
                .retain(|k, _| !(k == &path || recursive && k.starts_with(&format!("{path}/"))));
        });
        Ok(())
    }
    pub fn rename(from: impl AsRef<Path>, to: impl AsRef<Path>) -> std::io::Result<()> {
        let from = from.as_ref().name();
        let to = to.as_ref().name();
        mutation(format!("rename:{from}:{to}"), |io| {
            if let Some(bytes) = io.files.remove(&from) {
                io.files.insert(to.clone(), bytes);
            }
            if let Some(bytes) = io.synced.remove(&from) {
                io.synced.insert(to, bytes);
            }
        });
        Ok(())
    }
    pub fn read_dir(
        path: impl AsRef<Path>,
    ) -> std::io::Result<std::vec::IntoIter<std::io::Result<Entry>>> {
        let path = path.as_ref().clone();
        if sample(format!("readdir:{}", path.name())) == 254 {
            return Err(fault());
        }
        let children = IO.with(|io| {
            io.borrow()
                .files
                .keys()
                .filter_map(|name| {
                    let p = VPath::new(name);
                    (p.path.parent() == Some(path.path.as_path())).then_some(name.clone())
                })
                .collect::<BTreeSet<_>>()
        });
        Ok(children
            .into_iter()
            .map(|name| Ok(Entry(VPath::new(name))))
            .collect::<Vec<_>>()
            .into_iter())
    }
}
pub struct File(VPath);
impl File {
    pub fn create(path: impl AsRef<Path>) -> std::io::Result<Self> {
        let path = path.as_ref().clone();
        mutation(format!("create:{}", path.name()), |io| {
            io.files.insert(path.name(), vec![]);
        });
        Ok(Self(path))
    }
    pub fn open(path: impl AsRef<Path>) -> std::io::Result<Self> {
        let path = path.as_ref().clone();
        let n = sample(format!("open:{}", path.name()));
        if n == 254 {
            return Err(fault());
        }
        if !IO.with(|io| io.borrow().files.contains_key(&path.name())) {
            return Err(missing());
        }
        Ok(Self(path))
    }
    pub fn sync_all(&self) -> std::io::Result<()> {
        mutation(format!("fsync:{}", self.0.name()), |io| {
            if self.0.extension().is_none() {
                let prefix = format!("{}/", self.0.name());
                io.persisted.retain(|k, _| !k.starts_with(&prefix));
                io.persisted.extend(
                    io.synced
                        .iter()
                        .filter(|(k, _)| k.starts_with(&prefix))
                        .map(|(k, v)| (k.clone(), v.clone())),
                );
            } else if let Some(bytes) = io.files.get(&self.0.name()) {
                io.synced.insert(self.0.name(), bytes.clone());
            }
        });
        Ok(())
    }
}
impl Write for File {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        mutation(format!("write:{}:{bytes:?}", self.0.name()), |io| {
            io.files
                .get_mut(&self.0.name())
                .unwrap()
                .extend_from_slice(bytes)
        });
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        call("flush");
        Ok(())
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Job {
    pub action: ActionKey,
    pub delivery: Delivery,
    pub run: CheckRun,
    pub changed_paths: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected_tests: Option<u32>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub always_tests: Vec<String>,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct WaitSubscription {
    pub delivery: Delivery,
    pub wait: tl3_store::ExternalWait,
}
pub struct Settings {
    pub records: VPath,
    pub result_tool: Tool,
    pub python: Vec<OsString>,
    pub image: String,
    pub demand: Demand,
    pub quiet: std::time::Duration,
    pub priority: i32,
}
pub struct Lock;
impl Lock {
    pub fn lock(&self) -> Result<Guard> {
        call("lock");
        Ok(Guard)
    }
}
pub struct Guard;
impl Drop for Guard {
    fn drop(&mut self) {
        call("unlock");
    }
}
pub struct Keys;
impl Keys {
    pub fn send(&self, node: NodeId) -> Result<()> {
        call(format!("notify:{}", node.0));
        Ok(())
    }
}
pub struct Runtime;
impl Runtime {
    pub fn block_on<T>(&self, result: T) -> T {
        result
    }
}
pub struct Client;
pub fn adapter_error(error: tl3_prismabuild::Error) -> Error {
    Error::Adapter(Box::new(error))
}
pub fn decision_record(record: &RecordRead) -> decide::Record {
    match record {
        RecordRead::Absent => decide::Record::Absent,
        RecordRead::Moving => decide::Record::Moving,
        _ if record.is_live() => decide::Record::Live,
        _ => decide::Record::Ended,
    }
}
impl Client {
    pub fn read(&self, key: &ActionKey) -> std::result::Result<RecordRead, tl3_prismabuild::Error> {
        let n = sample(format!("pb.read:{key}"));
        if n == 254 {
            return Err(tl3_prismabuild::Error::Tool { code: Some(1) });
        }
        // This fake encodes decide.rs's reader_linearizability precondition:
        // a publication present throughout a read must return non-Absent.
        // queue.rs does not establish it today: two claim/rollback cycles can
        // miss both ready/claimed probe pairs. See t730-cc-pb-reader-race.
        let published = IO.with(|io| io.borrow().persisted.contains_key(&format!("pb:{key}")));
        let state = match n % 8 {
            0 if !published => return Ok(RecordRead::Absent),
            0 => ActionState::Queued,
            1 => return Ok(RecordRead::Moving),
            2 => ActionState::Queued,
            3 => ActionState::Claimed,
            4 => ActionState::Finished { returncode: 0 },
            5 => ActionState::Failed {
                status: "failed".into(),
                returncode: Some(1),
            },
            6 => ActionState::LeaseLost,
            _ => ActionState::Withdrawn,
        };
        Ok(RecordRead::Present(Box::new(
            tl3_prismabuild::ActionRecord {
                key: key.clone(),
                snapshot_parent: None,
                state,
                published_unix: 1.0,
                attempts: vec![],
                lease: None,
                returncode: None,
                termination_reason: None,
                stderr_tail: String::new(),
                progress_observation: None,
            },
        )))
    }
    pub fn publish(&self, key: &ActionKey) -> std::result::Result<(), tl3_prismabuild::Error> {
        mutation(format!("pb.publish:{key}"), |io| {
            io.persisted
                .insert(format!("pb:{key}"), b"published".to_vec());
        });
        Ok(())
    }
    pub fn withdraw(
        &self,
        key: &ActionKey,
        reason: &str,
    ) -> std::result::Result<(), tl3_prismabuild::Error> {
        mutation(format!("pb.withdraw:{key}:{reason}"), |io| {
            io.persisted
                .insert(format!("pb:{key}"), b"withdrawn".to_vec());
        });
        Ok(())
    }
    pub fn seal_at_priority(
        &self,
        action: &SubmitAction,
        priority: i32,
    ) -> std::result::Result<ActionKey, tl3_prismabuild::Error> {
        mutation(format!("pb.seal:{priority}:{:?}", action.command), |io| {
            io.persisted.insert("seal".into(), b"sealed".to_vec());
        });
        Ok("a".repeat(64).parse().unwrap())
    }
}
pub mod results {
    use super::*;
    pub fn ending(
        _tool: &Tool,
        job: &Job,
        withdrawal: bool,
        _record: &RecordRead,
    ) -> Result<Option<Completion>> {
        match sample(format!("pb.ending:{}:{withdrawal}", job.action)) % 4 {
            0 => Ok(None),
            1 => Ok(Some(Completion::confirmed(vec![]))),
            2 => Err(Error::Invalid("unreadable PrismaBuild ending")),
            _ => Err(std::io::Error::from(std::io::ErrorKind::PermissionDenied).into()),
        }
    }
}
pub struct Images;
pub struct AdditionalTests {
    pub selected: Option<u32>,
    pub always: Vec<String>,
}
impl Images {
    pub fn for_repository<'a>(
        &self,
        _repo: &tl3_controller::Repository,
        image: &'a str,
    ) -> &'a str {
        call("image");
        image
    }
    pub fn additional_tests(&self, _repo: &tl3_controller::Repository) -> AdditionalTests {
        call("additional-tests");
        AdditionalTests {
            selected: None,
            always: vec![],
        }
    }
    pub fn test_demand(
        &self,
        _repo: &tl3_controller::Repository,
    ) -> Option<tl3_config::TestDemand> {
        call("demand");
        None
    }
}
pub struct Harness;
impl Harness {
    pub fn path(&mut self) -> Result<VPath> {
        sample("harness.path");
        Ok(VPath::new("/harness"))
    }
}
pub struct HarnessLock;
impl HarnessLock {
    pub fn lock(&self) -> Result<Harness> {
        call("harness.lock");
        Ok(Harness)
    }
}
pub mod checks {
    use super::*;
    pub fn descended_base(delivery: &Delivery) -> Result<CheckRun> {
        sample("git.descended-base");
        delivery
            .checks
            .clone()
            .ok_or(Error::Invalid("missing check plan"))
    }
    pub fn stage(
        _input: &Path,
        _harness: &Path,
        _delivery: &Delivery,
        _image: &str,
        _paths: &[String],
        _tests: &AdditionalTests,
    ) -> Result<Vec<String>> {
        mutation("git.stage", |io| {
            io.files.insert("/records/0/input".into(), vec![]);
        });
        Ok(vec!["src/a.rs".into()])
    }
}
pub fn validate_plan_size(plan: &[tl3_controller::adapters::prismabuild::Check]) -> Result<()> {
    let n = sample(format!("validate-plan:{}", plan.len()));
    match n {
        254 => Err(fault().into()),
        253 => Err(Error::Invalid("check plan too large")),
        _ => Ok(()),
    }
}
pub struct Adapter {
    pub client: Client,
    pub settings: Settings,
    pub runtime: Runtime,
    pub keys: Keys,
    pub records_lock: Lock,
    pub images: Images,
    pub harness: HarnessLock,
}
impl Adapter {
    pub fn fixture() -> Self {
        Self {
            client: Client,
            settings: Settings {
                records: VPath::new("/records"),
                result_tool: Tool {
                    program: "unused".into(),
                    args: vec![],
                },
                python: vec!["python3".into()],
                image: "image".into(),
                demand: Demand {
                    cpus: std::num::NonZeroU32::new(1).unwrap(),
                    mem_gb: 1,
                    gpu: false,
                },
                quiet: std::time::Duration::from_secs(1),
                priority: -10,
            },
            runtime: Runtime,
            keys: Keys,
            records_lock: Lock,
            images: Images,
            harness: HarnessLock,
        }
    }
    pub fn protected_paths(&self, _repo: &tl3_controller::Repository) -> Vec<String> {
        call("protected-paths");
        vec![]
    }
}

pub struct SubmitAction {
    pub checkout: VPath,
    pub command: Vec<OsString>,
    pub demand: Demand,
    pub container_image: String,
    pub progress: Vec<ProgressPhase>,
    pub progress_cycle: bool,
    pub env: BTreeMap<String, String>,
}

#[derive(Debug)]
pub enum Reconciliation {
    Found(Box<Completion>),
    Absent,
    Pending,
}
