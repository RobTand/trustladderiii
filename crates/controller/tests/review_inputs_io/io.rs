use super::*;
#[derive(Clone, Debug)]
pub enum Read { Value(Value), Io, Json, Git, Invalid }
#[derive(Clone, Debug)]
pub struct Schedule {
    pub delivery: Delivery,
    pub snapshot: tl3_store::Snapshot,
    pub session: Option<Value>,
    pub tapes: BTreeMap<String, Vec<Read>>,
}
#[derive(Clone, Debug, PartialEq)]
pub struct Run { pub trace: Vec<Value>, pub result: std::result::Result<Vec<u8>, String> }
struct Recording { tapes: BTreeMap<String, Vec<Read>>, counts: BTreeMap<String, usize>, trace: Vec<Value> }
thread_local! { static ACTIVE: RefCell<Option<Recording>> = const { RefCell::new(None) }; }
pub fn read(op: &str, args: Value) -> Result<Value> {
    ACTIVE.with(|r| {
        let mut r = r.borrow_mut(); let r = r.as_mut().unwrap();
        let count = r.counts.entry(op.into()).or_default();
        let index = *count; *count += 1;
        let item = r.tapes.get(op).and_then(|t| t.get(index)).cloned().unwrap_or(Read::Value(Value::Null));
        let result = match item {
            Read::Value(v) => Ok(v),
            Read::Io => Err(Error::Io(std::io::Error::from(std::io::ErrorKind::PermissionDenied))),
            Read::Json => Err(Error::Json(serde_json::from_str::<Value>("{").unwrap_err())),
            Read::Git => Err(Error::Adapter("recorded git failure".into())),
            Read::Invalid => Err(Error::Invalid("recorded invalid evidence")),
        };
        let outcome = match &result { Ok(v) => json!({"ok":v}), Err(e) => json!({"error":e.to_string()}) };
        r.trace.push(json!({"op":op,"args":args,"outcome":outcome}));
        result
    })
}
pub struct Store;
impl Store {
    pub fn worker_materials(&mut self, node: &str) -> Result<Option<Value>> {
        read("worker_materials", json!([node])).map(|v| if v.is_null() { None } else { Some(v) })
    }
    pub fn worker_document(&mut self, node: &str, kind: &str) -> Result<Option<Value>> {
        read("worker_document", json!([node,kind])).map(|v| if v.is_null() { None } else { Some(v) })
    }
}
pub struct Worktrees;
impl Worktrees {
    pub fn open(root: &Path, repository: &Path) -> Result<Self> { read("open", json!([root,repository]))?; Ok(Self) }
    pub fn merge_base(&self, base: &CommitId, head: &CommitId) -> Result<CommitId> {
        Ok(CommitId::new(read("merge_base", json!([base,head]))?.as_str().unwrap())?)
    }
    pub fn diff(&self, base: &CommitId, head: &CommitId) -> Result<Vec<u8>> {
        Ok(serde_json::from_value(read("diff", json!([base,head]))?)?)
    }
    pub fn changed_paths_since_merge_base(&self, base: &CommitId, head: &CommitId) -> Result<Vec<PathBuf>> {
        Ok(serde_json::from_value(read("changed_paths", json!([base,head]))?)?)
    }
    pub fn tree(&self, head: &CommitId) -> Result<String> { Ok(read("tree", json!([head]))?.as_str().unwrap().into()) }
}
pub fn run(s: &Schedule, current: bool) -> Run {
    ACTIVE.with(|r| *r.borrow_mut() = Some(Recording { tapes:s.tapes.clone(), counts:BTreeMap::new(), trace:vec![] }));
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let f = if current { current::inputs } else { reference::inputs };
        f(&s.delivery, &mut Store, &s.snapshot, s.session.as_ref())
    }));
    let result = match result {
        Ok(Ok(v)) => Ok(serde_json::to_vec(&v).unwrap()),
        Ok(Err(e)) => Err(e.to_string()),
        Err(e) => Err(format!("panic:{}", e.downcast_ref::<&str>().copied().or_else(|| e.downcast_ref::<String>().map(String::as_str)).unwrap_or("unknown"))),
    };
    let trace = ACTIVE.with(|r| r.borrow_mut().take().unwrap().trace);
    Run {trace,result}
}
pub fn compare(s: &Schedule) -> Run {
    let old = run(s, false);
    let new = run(s, true);
    if old != new {
        let dir = std::env::var("TEST_UNDECLARED_OUTPUTS_DIR").unwrap_or_else(|_| std::env::temp_dir().to_string_lossy().into_owned());
        std::fs::write(Path::new(&dir).join("review-inputs-failure.txt"), format!("schedule={s:?}\nold={old:?}\nnew={new:?}\n")).unwrap();
        panic!("review inputs or I/O differ; full schedule persisted to writable test output");
    }
    old
}
