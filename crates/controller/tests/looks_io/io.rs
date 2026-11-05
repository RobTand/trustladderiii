use super::*;
use std::{
    cell::RefCell,
    panic::{AssertUnwindSafe, catch_unwind},
};
#[derive(Clone, Copy, Debug)]
pub(super) enum Entry {
    Failed(u8),
    State,
    Alive(bool),
    Probe,
    Stall,
    Progress(u8),
    Quota(u8),
}
#[derive(Clone, Debug, Default)]
pub(super) struct Schedule {
    pub d2: bool,
    pub announced: Option<u64>,
    pub since: Option<u64>,
    pub noticed: bool,
    pub waiting: bool,
    pub wait_ended: bool,
    pub held: bool,
    pub checks: bool,
    pub not_found: bool,
    pub recent_progress: bool,
    pub frames: Vec<(u8, u32)>,
    pub overrides: Vec<(String, usize, u8)>,
    pub writes: Vec<u8>,
    pub entries: Vec<Entry>,
    pub failures: u8,
    pub incidents: u8,
    pub counted: Option<u64>,
    pub incident: bool,
    pub needs_rob: bool,
    pub refused: bool,
    pub maintenance: bool,
    pub postmortem: bool,
    pub notice: bool,
    pub worker: bool,
    pub quota: bool,
    pub stopping: bool,
    pub progress: bool,
}
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Run {
    pub trace: Vec<(String, Value)>,
    pub files: BTreeMap<String, Vec<u8>>,
    pub result: Value,
    pub writes: usize,
}
#[derive(Debug)]
struct Crash;
pub(super) struct Recording {
    schedule: Schedule,
    pub files: BTreeMap<String, Vec<u8>>,
    trace: Vec<(String, Value)>,
    reads: usize,
    writes: usize,
    now: u64,
    revision: i64,
    cut: Option<(usize, bool)>,
}
thread_local! { static ACTIVE: RefCell<Option<Recording>> = const { RefCell::new(None) }; }
fn with<T>(f: impl FnOnce(&mut Recording) -> T) -> T {
    ACTIVE.with(|r| f(r.borrow_mut().as_mut().unwrap()))
}
pub(super) fn record(op: &str, value: Value) {
    with(|r| r.trace.push((op.into(), value)));
}
pub(super) fn read(op: &str, args: Value) -> Result<u8> {
    let word = with(|r| {
        let (word, delta) = r.schedule.frames.get(r.reads).copied().unwrap_or((0, 1000));
        let nth = r.trace.iter().filter(|(name, _)| name == op).count();
        let word = r
            .schedule
            .overrides
            .iter()
            .find(|(name, n, _)| name == op && *n == nth)
            .map_or(word, |(_, _, word)| *word);
        r.reads += 1;
        r.now = r.now.saturating_add(u64::from(delta));
        r.trace.push((op.into(), json!([args, word, r.now])));
        word
    });
    if word >= 240 {
        Err(error(word - 240))
    } else {
        Ok(word)
    }
}
pub(super) fn now() -> u64 {
    // The clock itself cannot fail. Its value can change independently per sample.
    let _ = read("clock", Value::Null);
    with(|r| r.now)
}
pub(super) fn error(code: u8) -> Error {
    match code % 12 {
        0 => Error::Invalid("unavailable"),
        1 => Error::Quota {
            retry_after: Duration::from_millis(1500),
        },
        2 => Error::Permanent {
            class: "queue_command_refused",
            source: "refused".into(),
        },
        3 => Error::Io(std::io::ErrorKind::PermissionDenied.into()),
        4 => Error::Io(std::io::ErrorKind::ConnectionReset.into()),
        5 => Error::Adapter(Box::new(tl3_mergify::Error::ChangedDuringRead)),
        6 => Error::Adapter(Box::new(tl3_github::Error::Http { status: 404 })),
        7 => Error::Adapter(Box::new(tl3_github::Error::Http { status: 500 })),
        8 => Error::Store(tl3_store::Error::OwnershipLost),
        9 => Error::StoredJson(serde_json::from_str::<Value>("invalid").unwrap_err()),
        10 => Error::Adapter(Box::new(tl3_mergify::Error::HeadChanged)),
        _ => Error::Adapter(Box::new(tl3_github::Error::Transport)),
    }
}
pub(super) fn write(op: &str, args: Value, change: impl FnOnce(&mut Recording)) -> Result<()> {
    let failed = with(|r| {
        let index = r.writes;
        r.writes += 1;
        let mode = r.schedule.writes.get(index).copied().unwrap_or(0);
        r.trace.push((op.into(), json!([args, mode])));
        if r.cut == Some((index, false)) {
            std::panic::panic_any(Crash);
        }
        if mode == 0 {
            change(r);
        }
        if r.cut == Some((index, true)) {
            std::panic::panic_any(Crash);
        }
        mode != 0
    });
    if failed {
        Err(Error::Store(tl3_store::Error::EffectConflict))
    } else {
        Ok(())
    }
}
pub(super) struct Store;
impl Store {
    pub fn load(&self) -> Result<Snapshot> {
        let word = read("store.load", Value::Null)?;
        let mut s = snapshot();
        for row in &mut s.rows {
            if row.table == "incidents" {
                if word & 1 != 0 {
                    row.value["open"] = json!(false);
                }
                if word & 2 != 0 {
                    row.value["needs_rob"] = json!(true);
                }
                if word & 4 != 0 {
                    row.value["effect"] = json!("other-effect");
                }
                if word & 8 != 0 {
                    row.value["signature"]["class"] = json!("queue_command_refused");
                }
            }
        }
        if word & 16 != 0 {
            s.rows.retain(|r| r.table != "incidents");
        }
        record(
            "store.loaded",
            json!(
                s.rows
                    .iter()
                    .map(|r| json!([r.table, r.id, r.value]))
                    .collect::<Vec<_>>()
            ),
        );
        Ok(s)
    }
    pub fn effect(&self, key: &str) -> Result<Option<Effect>> {
        let word = read("store.effect", json!(key))?;
        // An allowance's delivery always has an effect. Vary state and errors,
        // while preserving that store invariant required by look_state's expects.
        Ok(Some(Effect {
            key: key.into(),
            epoch: tl3_store::Epoch::try_from(1).unwrap(),
            kind: "mergify".into(),
            payload: json!(delivery(key)),
            state: match word % 4 {
                1 => EffectState::Confirmed,
                2 => EffectState::Cancelling,
                3 => EffectState::Failed,
                _ => EffectState::Uncertain,
            },
            created_revision: tl3_store::Revision::ZERO,
            updated_revision: tl3_store::Revision::ZERO,
        }))
    }
    pub fn commit(&mut self, c: &Commit) -> Result<()> {
        write("store.commit", json!(format!("{c:?}")), |r| {
            for row in &c.rows {
                match row {
                    RowChange::Put { table, id, value } => {
                        r.files
                            .insert(format!("{table}/{id}"), serde_json::to_vec(value).unwrap());
                    }
                    RowChange::Delete { table, id } => {
                        r.files.remove(&format!("{table}/{id}"));
                    }
                }
            }
            for e in &c.effects {
                r.files.insert(
                    format!("effects/{}", e.key),
                    serde_json::to_vec(&e.payload).unwrap(),
                );
            }
            r.revision += 1;
            r.files
                .insert("revision".into(), r.revision.to_string().into_bytes());
        })
    }
}
fn snapshot() -> Snapshot {
    with(|r| Snapshot {
        revision: tl3_store::Revision::try_from(r.revision).unwrap(),
        epoch: tl3_store::Epoch::try_from(1).unwrap(),
        rows: r
            .files
            .iter()
            .filter_map(|(path, bytes)| {
                let (table, id) = path.split_once('/')?;
                if table == "effects" {
                    return None;
                }
                Some(tl3_store::WorldRow {
                    table: table.into(),
                    id: id.into(),
                    value: serde_json::from_slice(bytes).unwrap(),
                    version: tl3_store::Revision::ZERO,
                    arrival: 0,
                })
            })
            .collect(),
        pending_effects: vec![],
        credit: vec![],
        interned: vec![],
    })
}
pub(super) struct Adapters;
pub(super) struct Adapter;
impl Adapters {
    pub fn adapter(&self, d: &Delivery) -> Result<Adapter> {
        read("adapter", json!(d.key))?;
        Ok(Adapter)
    }
    pub fn get(&self, u: Upstream) -> Result<Adapter> {
        read("adapter.get", json!(u))?;
        Ok(Adapter)
    }
}
impl Adapter {
    pub fn probe(&self, d: &Delivery) -> Result<bool> {
        Ok(read("probe", json!(d.key))? & 1 == 0)
    }
    pub fn look(&self, d: &Delivery) -> Result<Look> {
        Ok(match read("look", json!(d.key))? % 12 {
            0 => Look::Pending,
            1 => Look::Healthy,
            2 => Look::Alive {
                progress: "moving".into(),
            },
            3 => Look::Absent,
            4 => Look::Quota {
                reset_ms: now().saturating_add(1000),
            },
            5 => Look::Found(Box::new(completion(0))),
            6 => Look::Alive {
                progress: adapters::mergify::REQUEUE_PROGRESS_PREFIX.into(),
            },
            7 | 8 => Look::Waiting {
                wait: tl3_store::ExternalWait {
                    upstream: if with(|r| r.schedule.worker) {
                        "prismabuild"
                    } else {
                        "other"
                    }
                    .into(),
                    key: "outside-job".into(),
                },
                checkout: "checkout".into(),
            },
            9 => Look::Found(Box::new(completion(1))),
            10 => Look::Found(Box::new(completion(2))),
            _ => Look::Found(Box::new(completion(3))),
        })
    }
    pub fn stop_stalled(&self, d: &Delivery) -> Result<Completion> {
        read("stop_stalled", json!(d.key))?;
        Ok(Completion::confirmed(vec![]))
    }
    pub fn app_login(&self, installation: u64) -> Option<String> {
        let _ = read("app_login", json!(installation));
        Some("factory[bot]".into())
    }
    pub fn quota_resume_ms(&self, _: &Delivery, announced: u64, now: u64) -> Result<u64> {
        let word = read("quota_resume", json!(announced))?;
        Ok(match word % 3 {
            1 => now,
            2 => now.saturating_add(120_000),
            _ => announced,
        })
    }
    pub fn resume_after_quota(&self, _: &Delivery) -> Result<Option<Completion>> {
        let word = read("resume_quota", Value::Null)?;
        Ok((word % 4 != 1).then(|| completion(word % 4)))
    }
    pub fn watch_wait(&self, d: &Delivery, wait: Option<&tl3_store::ExternalWait>) -> Result<()> {
        read("watch_wait.read", json!(wait))?;
        write("watch_wait", json!([d.key, wait]), |r| {
            r.files
                .insert("external-watch".into(), serde_json::to_vec(&wait).unwrap());
        })
    }
    pub fn wait_record(&self, _: &str, _: &std::path::Path) -> Result<executor::WaitRecord> {
        let word = read("wait_record", Value::Null)?;
        Ok(if word & 1 != 0 {
            executor::WaitRecord::Alive
        } else {
            executor::WaitRecord::Ended
        })
    }
}
fn completion(code: u8) -> Completion {
    let mut c = Completion::confirmed(if code >= 2 {
        vec![Observation::Kernel(Event::Fact {
            node: NodeId(0),
            fact: Fact::UpstreamStalled,
        })]
    } else {
        vec![]
    });
    if code & 1 != 0 {
        c.progress = Some(executor::ProgressToken::Mergify(
            tl3_controller::adapters::mergify::Progress {
                merged_at: Some("checkpoint".into()),
                merge_commit_sha: None,
                queue_check: None,
            },
        ));
    }
    c
}
pub(super) fn held() -> bool {
    with(|r| r.schedule.held)
}
pub(super) fn checks() -> bool {
    with(|r| r.schedule.checks)
}
fn initial(s: &Schedule) -> AllowanceState {
    let mut d = delivery("effect-1");
    if s.worker {
        d.output = Out::Effect {
            node: NodeId(0),
            seq: 0,
            kind: tl3_kernel::EffectKind::StartRun(tl3_kernel::Role::Make),
            head: None,
        };
    }
    if s.postmortem {
        d.output = Out::Comment {
            node: NodeId(0),
            kind: executor::CommentKind::Postmortem,
        };
    }
    if s.notice {
        d.output = Out::Notice {
            node: NodeId(0),
            notice: tl3_kernel::Notice::Incident { needs_rob: false },
        };
    }
    let mut a = AllowanceState::new(d, 0, 1000);
    a.read_failures = s.failures;
    a.read_incidents = s.incidents;
    a.counted_failure_ms = s.counted;
    a.maintenance = s.maintenance;
    a.stopping = s.stopping;
    if s.incident {
        a.incident = Some("1.read".into());
    }
    a.quota_ms = s.announced.or(s.quota.then_some(1));
    a.unserved_since_ms = s.since;
    a.ladder_out_noticed = s.noticed;
    a.wait_ended = s.wait_ended;
    if s.waiting {
        a.external_wait = Some(tl3_store::ExternalWait {
            upstream: "prismabuild".into(),
            key: "outside-job".into(),
        });
    }
    if s.not_found {
        a.failure = Some("not_found".into());
    }
    if s.recent_progress {
        a.progress_ms = 1_000_000;
    }

    if s.progress {
        a.progress = Some(adapters::mergify::REQUEUE_PROGRESS_PREFIX.into());
    }
    a
}
pub(super) fn run(s: &Schedule, new: bool, cut: Option<(usize, bool)>) -> Run {
    static HOOK: std::sync::Once = std::sync::Once::new();
    HOOK.call_once(|| {
        let old = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |i| {
            if !i.payload().is::<Crash>() {
                old(i);
            }
        }));
    });
    let mut state = initial(s);
    let mut files = BTreeMap::new();
    files.insert(
        "allowances/effect-1".into(),
        serde_json::to_vec(&state).unwrap(),
    );
    files.insert(
        "watches/effect-1".into(),
        serde_json::to_vec(&json!({"key":"effect-1"})).unwrap(),
    );
    if s.incident {
        files.insert("incidents/1.read".into(), serde_json::to_vec(&json!({"id":"1.read","node":0,"open":true,"needs_rob":s.needs_rob,"blocking":false,"summary":"outage","signature":{"upstream":"Mergify","class":if s.refused {"queue_command_refused"} else {"transport"}},"effect":"effect-1"})).unwrap());
    }
    ACTIVE.with(|r| {
        *r.borrow_mut() = Some(Recording {
            schedule: s.clone(),
            files,
            trace: vec![],
            reads: 0,
            writes: 0,
            now: 1_000_000,
            revision: 1,
            cut,
        })
    });
    let result = catch_unwind(AssertUnwindSafe(|| {
        for &entry in &s.entries {
            let snap = snapshot();
            let result = if new {
                super::current::run(entry, &mut state, &snap)
            } else {
                super::reference::run(entry, &mut state, &snap)
            };
            if let Err(e) = result {
                return json!({"error":e.to_string(),"state":state});
            }
        }
        json!({"ok":true,"state":state})
    }));
    let result = match result {
        Ok(r) => r,
        Err(e) if e.is::<Crash>() => json!("crash"),
        Err(e) => std::panic::resume_unwind(e),
    };
    ACTIVE.with(|r| {
        let r = r.borrow_mut().take().unwrap();
        Run {
            trace: r.trace,
            files: r.files,
            result,
            writes: r.writes,
        }
    })
}
pub(super) fn compare(s: &Schedule) -> Run {
    let old = run(s, false, None);
    assert_eq!(old, run(s, true, None), "schedule={s:?}");
    for at in 0..old.writes {
        for after in [false, true] {
            assert_eq!(
                run(s, false, Some((at, after))),
                run(s, true, Some((at, after))),
                "schedule={s:?}; cut={at}:{after}"
            );
        }
    }
    old
}
