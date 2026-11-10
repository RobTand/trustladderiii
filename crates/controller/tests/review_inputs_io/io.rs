use super::*;
#[derive(Clone, Debug)]
pub enum Read {
    Value(Value),
    Io,
    Json,
    Git,
    Invalid,
    Store,
    Worktree(u8),
}
#[derive(Clone, Debug)]
pub struct Schedule {
    pub delivery: Delivery,
    pub snapshot: tl3_store::Snapshot,
    pub session: Option<Value>,
    pub tapes: BTreeMap<String, Vec<Read>>,
}
#[derive(Clone, Debug, PartialEq)]
pub struct Run {
    pub error_variant: Option<String>,
    pub trace: Vec<Value>,
    pub result: std::result::Result<Vec<u8>, String>,
}
struct Recording {
    current: bool,
    forced: bool,
    tapes: BTreeMap<String, Vec<Read>>,
    counts: BTreeMap<String, usize>,
    trace: Vec<Value>,
}
thread_local! { static ACTIVE: RefCell<Option<Recording>> = const { RefCell::new(None) }; }
pub fn read(op: &str, args: Value) -> Result<Value> {
    ACTIVE.with(|r| {
        let mut r = r.borrow_mut();
        let r = r.as_mut().unwrap();
        let count = r.counts.entry(op.into()).or_default();
        let index = *count;
        *count += 1;
        let item = r
            .tapes
            .get(op)
            .and_then(|t| t.get(index))
            .cloned()
            .unwrap_or(Read::Value(Value::Null));
        let result = match item {
            Read::Value(v) => Ok(v),
            Read::Io => Err(Error::Io(std::io::Error::from(
                std::io::ErrorKind::PermissionDenied,
            ))),
            Read::Json => Err(Error::Json(serde_json::from_str::<Value>("{").unwrap_err())),
            Read::Git => Err(Error::Adapter("recorded git failure".into())),
            Read::Invalid => Err(Error::Invalid("recorded invalid evidence")),
            Read::Store => Err(::tl3_store::Error::Invalid("recorded store failure").into()),
            Read::Worktree(kind) => Err(match kind {
                0 => tl3_worktrees::Error::Io,
                1 => tl3_worktrees::Error::Spawn,
                2 => tl3_worktrees::Error::Git {
                    operation: "recorded",
                    code: Some(1),
                },
                3 => tl3_worktrees::Error::Response,
                4 => tl3_worktrees::Error::MissingBranch,
                5 => tl3_worktrees::Error::Network {
                    operation: "recorded",
                    code: None,
                },
                6 => tl3_worktrees::Error::Remote,
                _ => tl3_worktrees::Error::CommitId,
            }
            .into()),
        };
        let outcome = match &result {
            Ok(v) => json!({"ok":v}),
            Err(e) => json!({"error":e.to_string()}),
        };
        r.trace.push(json!({"op":op,"args":args,"outcome":outcome}));
        result
    })
}
pub struct Store;
impl Store {
    pub fn worker_materials(&mut self, node: &str) -> Result<Option<Value>> {
        let extra = ACTIVE.with(|r| { let r=r.borrow(); let r=r.as_ref().unwrap(); r.current && !r.forced && r.counts.get("worker_materials").copied().unwrap_or(0)>0 });
        read(if extra {"issue_materials"} else {"worker_materials"}, json!([node])).map(|v| if v.is_null() { None } else { Some(v) })
    }
    pub fn worker_document(&mut self, node: &str, kind: &str) -> Result<Option<Value>> {
        read(if kind == "plans" {"plan_document"} else {"worker_document"}, json!([node, kind]))
            .map(|v| if v.is_null() { None } else { Some(v) })
    }
}
pub struct Worktrees;
impl Worktrees {
    pub fn open(root: &Path, repository: &Path) -> Result<Self> {
        read("open", json!([root, repository]))?;
        Ok(Self)
    }
    pub fn merge_base(&self, base: &CommitId, head: &CommitId) -> Result<CommitId> {
        Ok(CommitId::new(
            read("merge_base", json!([base, head]))?.as_str().unwrap(),
        )?)
    }
    pub fn diff(&self, base: &CommitId, head: &CommitId) -> Result<Vec<u8>> {
        Ok(serde_json::from_value(read("diff", json!([base, head]))?)?)
    }
    pub fn changed_paths_since_merge_base(
        &self,
        base: &CommitId,
        head: &CommitId,
    ) -> Result<Vec<PathBuf>> {
        Ok(serde_json::from_value(read(
            "changed_paths",
            json!([base, head]),
        )?)?)
    }
    pub fn tree(&self, head: &CommitId) -> Result<String> {
        Ok(read("tree", json!([head]))?.as_str().unwrap().into())
    }
}
pub fn run(s: &Schedule, current: bool) -> Run {
    ACTIVE.with(|r| {
        *r.borrow_mut() = Some(Recording {
            current,
            forced: forced(s),
            tapes: {
                let mut tapes=s.tapes.clone();
                let issue=tapes.get("worker_materials").and_then(|t|t.get(1)).cloned().unwrap_or(Read::Value(Value::Null));
                tapes.entry("issue_materials".into()).or_insert(vec![issue]);
                tapes
            },
            counts: BTreeMap::new(),
            trace: vec![],
        })
    });
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let f = if current {
            current::inputs
        } else {
            reference::inputs
        };
        f(&s.delivery, &mut Store, &s.snapshot, s.session.as_ref())
    }));
    let error_variant = match &result {
        Ok(Err(e)) => Some(format!("{:?}", std::mem::discriminant(e))),
        Err(_) => Some("panic".into()),
        _ => None,
    };
    let result = match result {
        Ok(Ok(v)) => Ok(serde_json::to_vec(&v).unwrap()),
        Ok(Err(e)) => Err(e.to_string()),
        Err(e) => Err(format!(
            "panic:{}",
            e.downcast_ref::<&str>()
                .copied()
                .or_else(|| e.downcast_ref::<String>().map(String::as_str))
                .unwrap_or("unknown")
        )),
    };
    let trace = ACTIVE.with(|r| r.borrow_mut().take().unwrap().trace);
    Run { trace, result, error_variant }
}
pub fn compare(s: &Schedule) -> Run {
    let old = run(s, false);
    let raw = run(s, true);
    if raw.result.is_err() && raw.trace.last().is_some_and(|c| c["op"]=="issue_materials" || c["op"]=="plan_document") {
        // A new read can fail where the legacy builder did not read at all.
        // Its exact preceding trace must still match the reference prefix.
        let prefix: Vec<_> = raw.trace[..raw.trace.len()-1].iter()
            .filter(|c| c["op"]!="issue_materials" && c["op"]!="plan_document")
            .cloned().collect();
        assert_eq!(prefix, old.trace[..prefix.len()]);
        assert!(raw.error_variant.is_some());
        return old;
    }
    let new = project_legacy(s, raw);
    if old != new {
        let dir = std::env::var("TEST_UNDECLARED_OUTPUTS_DIR")
            .unwrap_or_else(|_| std::env::temp_dir().to_string_lossy().into_owned());
        std::fs::write(
            Path::new(&dir).join("review-inputs-failure.txt"),
            format!("schedule={s:?}\nold={old:?}\nnew={new:?}\n"),
        )
        .unwrap();
        panic!("review inputs or I/O differ; full schedule persisted to writable test output");
    }
    old
}


fn forced(s: &Schedule) -> bool {
    s.snapshot.rows.iter().find(|r|r.table=="check_reports")
        .and_then(|r|r.value["results"].as_array()).into_iter().flatten()
        .any(|r|["forcing","proof","declared_symptom"].iter().any(|k|r.get(*k).is_some()))
}

// Compare the unchanged extraction behavior. C-4-only reads and fields have
// their own mandatory assertions, including injected errors, above this projection.
fn project_legacy(s: &Schedule, mut run: Run) -> Run {
    let scrub = |v: &mut Value| {
        v.as_object_mut().unwrap().remove("issue_truncated");
        v.as_object_mut().unwrap().remove("footprint_additions");
        if let Some(rules)=v["standing_rules"].as_array_mut() { rules.retain(|r|r["id"]!=ISSUE_RULE); }
        if !forced(s) { v.as_object_mut().unwrap().remove("issue"); }
        else if v.get("issue").is_none()
            && let Some(Read::Value(materials))=s.tapes.get("worker_materials").and_then(|t|t.get(1))
            && let Some(issue)=materials.get("issue")
            && (!issue["title"].is_string() || !issue["body"].is_string()) {
            // The old implementation turned malformed fields into empty strings.
            v["issue"]=json!({"title":issue["title"].as_str().unwrap_or("").chars().take(1000).collect::<String>(),"body":issue["body"].as_str().unwrap_or("").chars().take(16000).collect::<String>()});
        }
    };
    run.trace.retain(|c|c["op"]!="issue_materials" && c["op"]!="plan_document");
    for c in &mut run.trace { if c["op"]=="validate_inputs" { scrub(&mut c["args"][1]); } }
    if let Ok(bytes)=&mut run.result { let mut v:Value=serde_json::from_slice(bytes).unwrap(); scrub(&mut v); *bytes=serde_json::to_vec(&v).unwrap(); }
    run
}
