//! Recorded filesystem, transactional store and Hindsight leaf boundary.
use super::*;
use ::std::{
    cell::RefCell,
    collections::BTreeMap,
    io::Cursor,
    panic::{AssertUnwindSafe, catch_unwind},
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub(super) struct Schedule {
    pub shape: u8,
    pub rows: u8,
    pub curations: u8,
    pub capacity: u8,
    pub reads: Vec<u8>,
    pub writes: Vec<u8>,
    pub http: Vec<u8>,
    pub held: bool,
    pub key_error: bool,
    pub repeat: bool,
}
#[derive(Clone, Copy, Debug)]
pub(super) enum Entry {
    Queue,
    Flush,
    QueueFlush,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Run {
    pub trace: Vec<(String, Value, Value)>,
    pub effects: Vec<(String, Value)>,
    pub rows: BTreeMap<String, Value>,
    pub remote: BTreeMap<String, Value>,
    pub files: BTreeMap<String, Vec<u8>>,
    pub result: String,
    pub writes: usize,
}
struct Recording {
    s: Schedule,
    run: Run,
    read: usize,
    write: usize,
    http: usize,
    cut: Option<(usize, bool)>,
}
#[derive(Debug)]
struct Crash;
thread_local! { static ACTIVE: RefCell<Option<Recording>> = const { RefCell::new(None) }; }
fn with<T>(f: impl FnOnce(&mut Recording) -> T) -> T {
    ACTIVE.with(|r| f(r.borrow_mut().as_mut().expect("recorded scope")))
}
fn returned(value: Value) {
    with(|r| r.run.trace.last_mut().unwrap().2 = value);
}
pub(super) fn sample(op: &str, args: Value) -> Result<u8> {
    let mode = with(|r| {
        let mode = r.s.reads.get(r.read).copied().unwrap_or(0);
        r.read += 1;
        r.run.trace.push((op.into(), args, json!(mode)));
        mode
    });
    if mode == 1 {
        Err(Error::Invalid("recorded boundary failure"))
    } else {
        Ok(mode)
    }
}
fn mutate(op: &str, args: Value, change: impl FnOnce(&mut Recording)) -> Result<()> {
    let mode = with(|r| {
        let index = r.run.writes;
        r.run.writes += 1;
        let mode = r.s.writes.get(r.write).copied().unwrap_or(0);
        r.write += 1;
        r.run.trace.push((op.into(), args.clone(), json!(mode)));
        if r.cut == Some((index, false)) {
            ::std::panic::panic_any(Crash);
        }
        if mode != 1 {
            change(r);
            r.run.effects.push((op.into(), args));
        }
        if r.cut == Some((index, true)) {
            ::std::panic::panic_any(Crash);
        }
        mode
    });
    if mode != 0 {
        Err(Error::Invalid("recorded store failure"))
    } else {
        Ok(())
    }
}
fn outbox(sequence: i64, id: &str, parked: Value) -> Value {
    json!({"sequence":sequence,"id":id,"item":{"content":format!("record {id}")},"parked":parked})
}
fn curation(sequence: i64, state: &str, verdict: &str) -> Value {
    json!({"sequence":sequence,"memory_id":format!("memory-{sequence}"),"state":state,"verdict":verdict,"reason":"verified evidence", "delivered":false,"status":null})
}
fn initial(s: &Schedule) -> Run {
    let mut rows = BTreeMap::new();
    for n in 0..s.rows {
        rows.insert(
            format!("out:{n}"),
            outbox(n as i64 + 1, &format!("op-{n}"), Value::Null),
        );
    }
    for n in 0..s.capacity {
        rows.insert(
            format!("cap:{n}"),
            outbox(100 + n as i64, &format!("cap-{n}"), json!("capacity")),
        );
    }
    for n in 0..s.curations {
        rows.insert(
            format!("cur:{n}"),
            curation(n as i64 + 1, "invalidated", "accepted"),
        );
    }
    if s.shape == 23 {
        rows.insert("cur:hidden".into(), curation(200, "hidden", "accepted"));
        rows.insert(
            "cur:rejected".into(),
            curation(201, "invalidated", "rejected"),
        );
        rows.insert("expired".into(), json!({"parked":"http","expired":true}));
        rows.insert("recent".into(), json!({"parked":"http","expired":false}));
    }
    let role = match s.shape % 5 {
        0 => "maker",
        1 => "ingest",
        2 => "planner",
        3 => "audit",
        _ => "reviewer",
    };
    let reply = match s.shape {
        3 | 7 => Value::Null,
        10 => json!({"head":"a".repeat(39)}),
        15 => json!({"head":"z".repeat(40)}),
        _ => json!({"head":"a".repeat(40),"memory_flags":[],"conventions":[]}),
    };
    let evidence = json!({"path":"evidence.rs","start_line":1,"end_line":1,"quote":"named_function returns false"});
    let mut reply = reply;
    if s.shape == 12 {
        reply["conventions"] = json!([
            {"text":"named_function returns false","evidence":evidence},
            {"text":"","evidence":{}},
            {"text":"missing evidence","evidence":{}},
            {"text":"fourth ignored","evidence":evidence}]);
    }
    if s.shape == 11 || s.shape == 17 {
        reply["memory_ids"] = json!(["memory-1"]);
        reply["memory_flags"] = json!([
            {"id":"memory-1","reason":"contradiction","claim":"named_function returns true","evidence":evidence},
            {"id":"bad id","reason":"bad"},
            {"id":"memory-2","reason":"not supplied","claim":"missing"},
            {"id":"memory-1","reason":"fourth ignored"}]);
    }
    let mut request = json!({"memory":{"repo":"owner/repo","request":"req-1","epoch":3,"issue":1},"role":role,"operation":{"generation":4},"model":{"name":"model"},"inputs":{"tests":["private infrastructure"],"hindsight":"discard","forecast":{},"memories":[{"id":"memory-1","kind":"lesson","repo":"owner/repo","content":"named_function returns true in evidence.rs"}],"memory_note":"discard","repo":{"materials":"discard","name":"repo"},"brief":"do work"}});
    if s.shape == 1 {
        request.as_object_mut().unwrap().remove("memory");
    }
    if s.shape == 8 {
        request["memory"] = Value::Null;
    }
    if s.shape == 18 {
        request["memory"]["issue"] = json!("x".repeat(34000));
    }
    if s.shape == 19 {
        request["inputs"]["brief"] = json!("é\"\\".repeat(4000));
    }
    if s.shape == 21 {
        request["inputs"] = json!(["array inputs"]);
    }
    let summary = json!({"reply":reply,"outcome":if s.shape==6 {"failed"} else {"succeeded"},"reason":"reported failure","activity":{"tests":[{"action_key":"a".repeat(64)},{"action_key":"short"}],"token":"redact"}});
    let mut files: BTreeMap<String,Vec<u8>> = [
        ("run/attempt/request.json".into(),request.to_string().into_bytes()),
        ("run/attempt/memory.json".into(),summary.to_string().into_bytes()),
        ("run/attempt/exit.json".into(),json!({"code":if s.shape==4 {1} else {0}}).to_string().into_bytes()),
        ("run/attempt/stdout.jsonl".into(),b"invalid\n{\"event\":\"progress\"}\n{\"event\":\"reply\",\"reply\":{\"older\":true}}\n{\"event\":\"reply\",\"reply\":{\"final\":true}}\n".to_vec()),
        ("run/worktree/evidence.rs".into(),b"named_function returns false\n".to_vec()),
    ].into();
    match s.shape {
        0 => {
            files.remove("run/attempt/request.json");
        }
        2 => {
            files.insert("run/attempt/request.json".into(), b"invalid".to_vec());
        }
        5 => {
            files.remove("run/attempt/exit.json");
        }
        7 => {
            files.remove("run/attempt/stdout.jsonl");
        }
        9 => {
            files.insert("run/attempt/memory.json".into(), b"invalid".to_vec());
        }
        13 => {
            files.insert("run/attempt/exit.json".into(), b"invalid".to_vec());
        }
        14 => {
            files.insert("run/attempt/memory.json".into(), vec![b'x'; 32769]);
        }
        _ => {}
    }
    Run {
        trace: vec![],
        effects: vec![],
        rows,
        remote: BTreeMap::new(),
        files,
        result: String::new(),
        writes: 0,
    }
}
pub(super) fn run(s: &Schedule, entry: Entry, current: bool, cut: Option<(usize, bool)>) -> Run {
    static HOOK: ::std::sync::Once = ::std::sync::Once::new();
    HOOK.call_once(|| {
        let previous = ::std::panic::take_hook();
        ::std::panic::set_hook(Box::new(move |info| {
            if !info.payload().is::<Crash>() {
                previous(info);
            }
        }));
    });
    ACTIVE.with(|a| {
        *a.borrow_mut() = Some(Recording {
            s: s.clone(),
            run: initial(s),
            read: 0,
            write: 0,
            http: 0,
            cut,
        })
    });
    let result = catch_unwind(AssertUnwindSafe(|| {
        let mut store = Store;
        let settings = settings::Hindsight {
            bank: "bank".into(),
            endpoint: ::reqwest::Url::parse("http://hindsight.invalid/").unwrap(),
        };
        let mut result = Ok(0usize);
        for _ in 0..if s.repeat { 2 } else { 1 } {
            result = match entry {
                Entry::Queue => if current {
                    current::queue_attempt(&mut store, "key", Path::new("run/attempt"))
                } else {
                    reference::queue_attempt(&mut store, "key", Path::new("run/attempt"))
                }
                .map(|()| 0),
                Entry::Flush => {
                    if current {
                        current::flush(&mut store, &settings)
                    } else {
                        reference::flush(&mut store, &settings)
                    }
                }
                Entry::QueueFlush => {
                    let queued = if current {
                        current::queue_attempt(&mut store, "key", Path::new("run/attempt"))
                    } else {
                        reference::queue_attempt(&mut store, "key", Path::new("run/attempt"))
                    };
                    queued.and_then(|()| {
                        if current {
                            current::flush(&mut store, &settings)
                        } else {
                            reference::flush(&mut store, &settings)
                        }
                    })
                }
            };
        }
        format!("{result:?}")
    }));
    let result = match result {
        Ok(v) => v,
        Err(p) if p.is::<Crash>() => "crash".into(),
        Err(p) => ::std::panic::resume_unwind(p),
    };
    ACTIVE.with(|a| {
        let mut r = a.borrow_mut().take().unwrap().run;
        r.result = result;
        r
    })
}
pub(super) fn compare(s: &Schedule, entry: Entry) -> ::std::result::Result<usize, String> {
    let expected = run(s, entry, false, None);
    let actual = run(s, entry, true, None);
    if expected != actual {
        return Err(format!(
            "complete {entry:?}: {s:?}\nold={expected:?}\nnew={actual:?}"
        ));
    }
    for write in 0..expected.writes {
        for after in [false, true] {
            let old = run(s, entry, false, Some((write, after)));
            let new = run(s, entry, true, Some((write, after)));
            if old != new {
                return Err(format!(
                    "cut {write}/{after} {entry:?}: {s:?}\nold={old:?}\nnew={new:?}"
                ));
            }
        }
    }
    Ok(1 + 2 * expected.writes)
}

pub(super) struct Store;
impl Store {
    pub fn load(&mut self) -> Result<bool> {
        let mode = sample("store.load", Value::Null)?;
        let held = with(|r| r.s.held) || mode == 2;
        returned(json!(held));
        Ok(held)
    }
    pub fn set_memory_feed_attempt(&mut self, id: &str, held: bool) -> Result<()> {
        mutate("store.set_memory_feed_attempt", json!([id, held]), |r| {
            r.run
                .rows
                .insert("feed".into(), json!({"lineage":id,"held":held}));
        })
    }
    pub fn memory_item_bytes(&mut self, item: &Value) -> Result<i32> {
        let mode = sample("store.memory_item_bytes", item.clone())?;
        let size = if mode == 3 {
            32769
        } else {
            stored_json_bytes(item) as i32
        };
        returned(json!(size));
        Ok(size)
    }
    pub fn queue_memory(&mut self, id: &str, item: &Value) -> Result<()> {
        mutate("store.queue_memory", json!([id, item]), |r| {
            let sequence = r
                .run
                .rows
                .values()
                .filter_map(|v| v["sequence"].as_i64())
                .max()
                .unwrap_or(0)
                + 1;
            r.run
                .rows
                .entry(format!("queued:{id}"))
                .or_insert(json!({"sequence":sequence,"id":id,"item":item,"parked":null}));
        })
    }
    pub fn memory_signals(&mut self, id: &str) -> Result<Vec<(String, String, Option<String>)>> {
        let mode = sample("store.memory_signals", json!(id))?;
        let signals = if mode == 2 {
            vec![("hidden".into(), "rob".into(), None)]
        } else {
            vec![]
        };
        returned(json!(signals));
        Ok(signals)
    }
    pub fn record_memory_curation(
        &mut self,
        id: &str,
        c: &tl3_store::MemoryCuration,
    ) -> Result<()> {
        let value = json!({"memory_id":c.memory_id,"state":c.state,"verdict":c.verdict,"source":c.source,"request":c.request,"run":c.run,"claim":c.claim,"reason":c.reason,"evidence":c.evidence});
        mutate("store.record_memory_curation", json!([id, value]), |r| {
            let mut value = value;
            value["sequence"] = json!(300 + r.run.rows.len());
            value["delivered"] = json!(false);
            value["status"] = Value::Null;
            r.run
                .rows
                .entry(format!("cur:queued:{id}"))
                .or_insert(value);
        })
    }
    pub fn expire_parked_memories(&mut self) -> Result<()> {
        mutate("store.expire_parked_memories", Value::Null, |r| {
            r.run.rows.retain(|_, v| v["expired"] != true)
        })
    }
    pub fn pending_memory_curations(&mut self) -> Result<Vec<(i64, String, String, String)>> {
        let mode = sample("store.pending_memory_curations", Value::Null)?;
        let mut values = with(|r| {
            r.run
                .rows
                .values()
                .filter(|v| {
                    v["verdict"] == "accepted"
                        && v["state"] != "hidden"
                        && v["delivered"] == false
                        && v["status"].is_null()
                })
                .map(|v| {
                    (
                        v["sequence"].as_i64().unwrap(),
                        v["memory_id"].as_str().unwrap().into(),
                        v["state"].as_str().unwrap().into(),
                        v["reason"].as_str().unwrap().into(),
                    )
                })
                .collect::<Vec<_>>()
        });
        values.sort_by_key(|v| v.0);
        values.truncate(64);
        if mode == 2 {
            values.clear();
        }
        returned(json!(values));
        Ok(values)
    }
    pub fn pending_memories(&mut self, after: i64) -> Result<Vec<(i64, String, Value)>> {
        let mode = sample("store.pending_memories", json!(after))?;
        let mut values = with(|r| {
            r.run
                .rows
                .values()
                .filter(|v| {
                    v.get("id").is_some()
                        && v["parked"].is_null()
                        && v["sequence"].as_i64().unwrap() > after
                })
                .map(|v| {
                    (
                        v["sequence"].as_i64().unwrap(),
                        v["id"].as_str().unwrap().into(),
                        v["item"].clone(),
                    )
                })
                .collect::<Vec<_>>()
        });
        values.sort_by_key(|v| v.0);
        values.truncate(64);
        if mode == 2 {
            values.clear();
        }
        returned(json!(values));
        Ok(values)
    }
    pub fn forget_memory(&mut self, id: &str) -> Result<()> {
        mutate("store.forget_memory", json!(id), |r| {
            r.run.rows.retain(|_, v| v["id"] != id)
        })
    }
    pub fn park_memory(&mut self, id: &str, status: u16) -> Result<()> {
        mutate("store.park_memory", json!([id, status]), |r| {
            for v in r.run.rows.values_mut().filter(|v| v["id"] == id) {
                v["parked"] = json!("http");
                v["status"] = json!(status);
            }
        })
    }
    pub fn reject_memory_key(&mut self, status: u16) -> Result<()> {
        mutate("store.reject_memory_key", json!(status), |r| {
            r.run.rows.insert("health".into(), json!(status));
        })
    }
    pub fn settle_memory_curation(&mut self, sequence: i64, status: Option<u16>) -> Result<()> {
        mutate(
            "store.settle_memory_curation",
            json!([sequence, status]),
            |r| {
                for v in r
                    .run
                    .rows
                    .values_mut()
                    .filter(|v| v.get("verdict").is_some() && v["sequence"] == sequence)
                {
                    if let Some(status) = status {
                        v["status"] = json!(status);
                    } else {
                        v["delivered"] = json!(true);
                    }
                }
            },
        )
    }
    pub fn replay_capacity_memories(&mut self) -> Result<u64> {
        let mut count = 0;
        mutate("store.replay_capacity_memories", Value::Null, |r| {
            for v in r
                .run
                .rows
                .values_mut()
                .filter(|v| v["parked"] == "capacity")
            {
                v["parked"] = Value::Null;
                count += 1;
            }
        })?;
        returned(json!(count));
        Ok(count)
    }
    pub fn memory_delivery_ack(&mut self) -> Result<()> {
        mutate("store.memory_delivery_ack", Value::Null, |r| {
            r.run.rows.insert("ack".into(), json!(true));
        })
    }
}

pub(super) struct File(Cursor<Vec<u8>>);
impl File {
    pub fn open(path: impl AsRef<Path>) -> ::std::io::Result<Self> {
        let path = path.as_ref().to_string_lossy().into_owned();
        let mode = sample("fs.open", json!(path))
            .map_err(|_| ::std::io::Error::from(::std::io::ErrorKind::PermissionDenied))?;
        if mode == 2 {
            return Err(::std::io::ErrorKind::NotFound.into());
        }
        let bytes =
            with(|r| r.run.files.get(&path).cloned()).ok_or(::std::io::ErrorKind::NotFound)?;
        let bytes = if mode == 3 {
            b"invalid".to_vec()
        } else {
            bytes
        };
        returned(json!(bytes));
        Ok(Self(Cursor::new(bytes)))
    }
}
impl Read for File {
    fn read(&mut self, buf: &mut [u8]) -> ::std::io::Result<usize> {
        sample("fs.read", json!(buf.len()))
            .map_err(|_| ::std::io::Error::from(::std::io::ErrorKind::Other))?;
        let n = self.0.read(buf)?;
        returned(json!(&buf[..n]));
        Ok(n)
    }
}
pub(super) mod mock_std {
    pub use ::std::{collections, io, path, result};
    pub mod fs {
        use super::super::*;
        pub struct Metadata {
            size: u64,
        }
        impl Metadata {
            pub fn file_type(&self) -> Self {
                Self { size: self.size }
            }
            pub fn is_symlink(&self) -> bool {
                false
            }
            pub fn is_file(&self) -> bool {
                true
            }
            pub fn len(&self) -> u64 {
                self.size
            }
        }
        pub fn symlink_metadata(path: impl AsRef<Path>) -> ::std::io::Result<Metadata> {
            metadata(path)
        }
        pub fn metadata(path: impl AsRef<Path>) -> ::std::io::Result<Metadata> {
            let path = path.as_ref().to_string_lossy().into_owned();
            sample("fs.metadata", json!(path))
                .map_err(|_| ::std::io::Error::from(::std::io::ErrorKind::NotFound))?;
            let size = with(|r| r.run.files.get(&path).map(|v| v.len() as u64));
            if path == "run/worktree" {
                return Ok(Metadata { size: 0 });
            }
            size.map(|size| Metadata { size })
                .ok_or(::std::io::ErrorKind::NotFound.into())
        }
        pub fn read_to_string(path: impl AsRef<Path>) -> ::std::io::Result<String> {
            let mut s = String::new();
            File::open(path)?.read_to_string(&mut s)?;
            Ok(s)
        }
    }
}
pub(super) mod settings {
    use super::*;
    pub struct Hindsight {
        pub bank: String,
        pub endpoint: ::reqwest::Url,
    }
    impl Hindsight {
        pub fn retain_key(&self) -> Result<Option<String>> {
            sample("settings.retain_key", Value::Null)?;
            if with(|r| r.s.key_error) {
                Err(Error::Invalid("recorded key failure"))
            } else {
                Ok(None)
            }
        }
    }
}
pub(super) mod reqwest {
    use super::*;
    pub use ::reqwest::Url;
    pub mod redirect {
        pub struct Policy;
        impl Policy {
            pub fn none() -> Self {
                Self
            }
        }
    }
    #[derive(Debug, thiserror::Error)]
    #[error("recorded HTTP transport or body failure")]
    pub struct HttpError;
    pub struct Client;
    pub struct Builder;
    pub struct Request {
        method: &'static str,
        url: Url,
        body: Value,
    }
    pub struct Response {
        status: u16,
        body: Value,
        bad_body: bool,
    }
    pub struct Status(u16);
    impl Status {
        pub fn is_success(&self) -> bool {
            (200..300).contains(&self.0)
        }
        pub fn as_u16(&self) -> u16 {
            self.0
        }
    }
    impl Client {
        pub fn builder() -> Builder {
            Builder
        }
        pub fn post(&self, url: Url) -> Request {
            Request {
                method: "POST",
                url,
                body: Value::Null,
            }
        }
        pub fn patch(&self, url: Url) -> Request {
            Request {
                method: "PATCH",
                url,
                body: Value::Null,
            }
        }
    }
    impl Builder {
        pub fn timeout(self, d: Duration) -> Self {
            with(|r| {
                r.run
                    .trace
                    .push(("http.timeout".into(), json!(d.as_millis()), Value::Null))
            });
            self
        }
        pub fn connect_timeout(self, d: Duration) -> Self {
            with(|r| {
                r.run.trace.push((
                    "http.connect_timeout".into(),
                    json!(d.as_millis()),
                    Value::Null,
                ))
            });
            self
        }
        pub fn no_proxy(self) -> Self {
            self
        }
        pub fn redirect(self, _: redirect::Policy) -> Self {
            self
        }
        pub fn build(self) -> ::std::result::Result<Client, HttpError> {
            sample("http.client", Value::Null).map_err(|_| HttpError)?;
            Ok(Client)
        }
    }
    impl Request {
        pub fn json(mut self, v: &Value) -> Self {
            self.body = v.clone();
            self
        }
        pub fn bearer_auth(self, _: impl AsRef<str>) -> Self {
            self
        }
        pub async fn send(self) -> ::std::result::Result<Response, HttpError> {
            let mode = with(|r| {
                let m = r.s.http.get(r.http).copied().unwrap_or(0);
                r.http += 1;
                m
            });
            let status = match mode {
                1 => 401,
                2 => 403,
                3 => 408,
                4 => 429,
                5 => 400,
                6 => 422,
                7 => 499,
                8 => 500,
                9 => 503,
                20 => 201,
                21 => 299,
                22 => 302,
                _ => 200,
            };
            let mut body = json!({"success":true,"async":true,"operation_id":self.body["operation_id"],"bank_id":"bank","items_count":1});
            match mode {
                12 => body["success"] = json!(false),
                13 => body["async"] = json!(false),
                14 => body["operation_id"] = json!("wrong"),
                15 => body["bank_id"] = json!("wrong"),
                16 => body["items_count"] = json!(0),
                17 => body["items_count"] = json!(2),
                _ => {}
            }
            with(|r| {
                let args = json!({"url":self.url.as_str(),"body":self.body});
                r.run.trace.push((
                    format!("http.{}", self.method),
                    args.clone(),
                    json!({"mode":mode,"status":status}),
                ));
                if (200..300).contains(&status) && mode != 10 {
                    let key = if self.method == "POST" {
                        self.body["operation_id"].as_str().unwrap().to_owned()
                    } else {
                        self.url.path().to_owned()
                    };
                    let new = if self.method == "POST" {
                        r.run.remote.insert(key, self.body.clone()).is_none()
                    } else {
                        r.run.remote.insert(key, self.body.clone());
                        true
                    };
                    if new {
                        r.run
                            .effects
                            .push((format!("remote.{}", self.method), args));
                    }
                }
            });
            if mode == 10 || mode == 11 {
                Err(HttpError)
            } else {
                Ok(Response {
                    status,
                    body,
                    bad_body: mode == 18,
                })
            }
        }
    }
    impl Response {
        pub fn status(&self) -> Status {
            Status(self.status)
        }
        pub async fn json(self) -> ::std::result::Result<Value, HttpError> {
            sample("http.body", Value::Null).map_err(|_| HttpError)?;
            if self.bad_body {
                Err(HttpError)
            } else {
                returned(self.body.clone());
                Ok(self.body)
            }
        }
    }
}
