//! #422: recall into initial context, and retiring memories shown to be wrong.
#[path = "../../store/tests/support/mod.rs"]
mod support;

use serde_json::{Value, json};
use std::{
    fs,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tl3_controller::memory::{self, CurationState, Recall};
use tl3_store::Store;
use wiremock::{
    Mock, MockServer, Request, Respond, ResponseTemplate,
    matchers::{method, path, path_regex},
};

const KEY: &str = "fixture-recall-authentication";
const QUOTE: &str = "def greet(): return 'hello'";

#[derive(Clone, Default)]
struct Bank {
    /// Results for the lessons-first call, which filters on curated kinds.
    curated: Arc<Mutex<Vec<Value>>>,
    /// Results for the unfiltered call.
    general: Arc<Mutex<Vec<Value>>>,
    recalls: Arc<Mutex<Vec<Value>>>,
    curations: Arc<Mutex<Vec<(String, Value)>>>,
    /// 0 answers normally; any other status is returned as is; 1 sends a malformed body.
    status: Arc<Mutex<u16>>,
    hung: Arc<Mutex<bool>>,
}
impl Respond for Bank {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        if *self.hung.lock().unwrap() {
            return ResponseTemplate::new(200).set_delay(Duration::from_secs(60));
        }
        if request.headers.get("authorization").map(|v| v.as_bytes())
            != Some(format!("Bearer {KEY}").as_bytes())
        {
            return ResponseTemplate::new(401);
        }
        match *self.status.lock().unwrap() {
            0 => {}
            1 => return ResponseTemplate::new(200).set_body_string("{\"results\": [oops"),
            status => return ResponseTemplate::new(status),
        }
        let body: Value = request.body_json().unwrap();
        if request.method.as_str() == "PATCH" {
            let id = request.url.path().rsplit('/').next().unwrap().to_owned();
            self.curations.lock().unwrap().push((id, body));
            return ResponseTemplate::new(200).set_body_json(json!({"id": "m", "state": "x"}));
        }
        self.recalls.lock().unwrap().push(body.clone());
        let results = if body.get("tags").is_some() || body.get("tag_groups").is_some() {
            self.curated.lock().unwrap().clone()
        } else {
            self.general.lock().unwrap().clone()
        };
        ResponseTemplate::new(200).set_body_json(json!({"results": results}))
    }
}

struct Fixture {
    database: support::Database,
    settings: tl3_config::Store,
    _runtime: tokio::runtime::Runtime,
    server: MockServer,
    bank: Bank,
    scratch: tempfile::TempDir,
}
impl Fixture {
    fn new() -> Self {
        let mut database = support::Database::start();
        let settings = database.create("context");
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        let server = runtime.block_on(MockServer::start());
        let bank = Bank::default();
        runtime.block_on(
            Mock::given(method("POST"))
                .and(path("/v1/default/banks/tl3/memories/recall"))
                .respond_with(bank.clone())
                .mount(&server),
        );
        runtime.block_on(
            Mock::given(method("PATCH"))
                .and(path_regex("^/v1/default/banks/tl3/memories/[^/]+$"))
                .respond_with(bank.clone())
                .mount(&server),
        );
        runtime.block_on(
            Mock::given(method("POST"))
                .and(path("/v1/default/banks/tl3/memories"))
                .respond_with(|request: &Request| {
                    let body: Value = request.body_json().unwrap();
                    ResponseTemplate::new(200).set_body_json(json!({
                        "success": true, "bank_id": "tl3", "async": true,
                        "items_count": 1, "operation_id": body["operation_id"],
                    }))
                })
                .mount(&server),
        );
        let scratch = tempfile::tempdir().unwrap();
        let key = scratch.path().join("hindsight-key");
        fs::write(&key, format!("{KEY}\n")).unwrap();
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&key, fs::Permissions::from_mode(0o600)).unwrap();
        Self {
            database,
            settings,
            _runtime: runtime,
            server,
            bank,
            scratch,
        }
    }
    fn store(&self) -> Store {
        Store::open(&self.settings).unwrap()
    }
    fn hindsight(&self) -> tl3_config::Hindsight {
        tl3_config::Hindsight {
            endpoint: self.server.uri().parse().unwrap(),
            bank: "tl3".into(),
            api_key_file: Some(self.scratch.path().join("hindsight-key")),
        }
    }
    fn recall(&self, recall: Recall<'_>) -> Value {
        memory::recall(&mut self.store(), &self.hindsight(), recall).unwrap()
    }
    /// A finished run laid out as the supervisor leaves it: attempt beside its base worktree.
    fn attempt(&self, role: &str, memories: Value, reply: Value) -> std::path::PathBuf {
        self.attempt_in(role, role, memories, reply)
    }
    fn attempt_in(
        &self,
        directory: &str,
        role: &str,
        memories: Value,
        reply: Value,
    ) -> std::path::PathBuf {
        let root = self.scratch.path().join(directory);
        let worktree = root.join("worktree");
        fs::create_dir_all(worktree.join("src")).unwrap();
        fs::write(
            worktree.join("greeting.py"),
            format!("{QUOTE}\n\n\ndef wave():\n    return 'bye'\n"),
        )
        .unwrap();
        let attempt = root.join("attempt-00000000");
        fs::create_dir_all(&attempt).unwrap();
        fs::write(
            attempt.join("request.json"),
            serde_json::to_vec(&json!({
                "role": role, "model": {"name": "fixture-model"},
                "operation": {"node": 3, "generation": 1},
                "inputs": {"memories": memories},
                "memory": {"repo": "RobTand/tl3-sandbox", "issue": 42, "request": "3", "epoch": 0},
            }))
            .unwrap(),
        )
        .unwrap();
        fs::write(
            attempt.join("memory.json"),
            serde_json::to_vec(&json!({"reply": reply, "outcome": "succeeded"})).unwrap(),
        )
        .unwrap();
        fs::write(attempt.join("exit.json"), r#"{"code":0}"#).unwrap();
        attempt
    }
    fn curations(&self) -> Vec<Value> {
        self.store().memory_curations().unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.database.stop();
    }
}

fn result(id: &str, text: &str, tags: &[&str]) -> Value {
    json!({"id": id, "text": text, "type": "world", "tags": tags})
}

fn supplied(id: &str, kind: &str) -> Value {
    json!([{"id": id, "kind": kind, "content": "greet() returns 'hi'."}])
}

fn evidence() -> Value {
    json!({"path": "greeting.py", "start_line": 1, "end_line": 1, "quote": QUOTE})
}

fn flag(id: &str, evidence: Value) -> Value {
    json!({"id": id, "reason": "greet() returns 'hello', not 'hi'.", "evidence": evidence})
}

fn context<'a>() -> Recall<'a> {
    Recall::Context {
        repo: "RobTand/tl3-sandbox",
        query: "Fix greeting\nReturn hello.",
    }
}

#[test]
fn recall_supplies_lessons_first_bounded_and_redacted() {
    let f = Fixture::new();
    *f.bank.curated.lock().unwrap() = vec![
        result(
            "lesson-1",
            "Greeting tests pin exact strings.",
            &[
                "kind:lesson",
                "source:postmortem",
                "repo:RobTand/tl3-sandbox",
            ],
        ),
        result(
            "convention-1",
            "Tests live in tests/.",
            &["kind:convention", "repo:RobTand/tl3-sandbox"],
        ),
    ];
    let mut general = vec![
        result(
            "run-1",
            "An earlier maker run changed greeting.py.",
            &["repo:RobTand/tl3-sandbox", "request:2", "role:maker"],
        ),
        result(
            "lesson-1",
            "Greeting tests pin exact strings.",
            &["kind:lesson"],
        ),
        result(
            "secret-1",
            "Deploy with token ghp_fixture1234567890 and SERVICE_API_KEY=fixture-value.",
            &[],
        ),
        result("long-1", &"x".repeat(5000), &[]),
    ];
    for n in 0..12 {
        general.push(result(&format!("extra-{n}"), &"y".repeat(1400), &[]));
    }
    *f.bank.general.lock().unwrap() = general;

    let value = f.recall(context());
    assert!(value.get("memory_note").is_none(), "{value}");
    let memories = value["memories"].as_array().unwrap();
    let ids: Vec<&str> = memories.iter().map(|m| m["id"].as_str().unwrap()).collect();
    assert_eq!(
        &ids[..5],
        ["lesson-1", "convention-1", "run-1", "secret-1", "long-1"],
        "lessons and conventions come first, without duplicates"
    );
    assert!(memories.len() <= memory::MAX_MEMORIES);
    assert_eq!(memories[0]["kind"], "lesson");
    assert_eq!(memories[1]["kind"], "convention");
    assert_eq!(memories[2]["kind"], "run");
    assert_eq!(memories[3]["kind"], "other");
    for memory in memories {
        assert_eq!(
            memory.as_object().unwrap().keys().collect::<Vec<_>>(),
            ["content", "id", "kind"]
        );
        assert!(memory["content"].as_str().unwrap().chars().count() <= memory::MEMORY_CHARS);
    }
    let total: usize = memories
        .iter()
        .map(|m| m["content"].as_str().unwrap().chars().count())
        .sum();
    assert!(total <= memory::MEMORIES_CHARS);
    let text = value.to_string();
    assert!(!text.contains("ghp_fixture") && !text.contains("fixture-value"));
    assert!(!text.contains(KEY));

    let recalls = f.bank.recalls.lock().unwrap();
    assert_eq!(recalls.len(), 2);
    for body in recalls.iter() {
        assert_eq!(body["types"], json!(["world", "experience"]));
        assert!(
            body["query"]
                .as_str()
                .unwrap()
                .contains("RobTand/tl3-sandbox")
        );
        assert!(body["query"].as_str().unwrap().contains("Return hello."));
    }
    assert_eq!(
        recalls[0]["tags"],
        json!(["kind:lesson", "kind:decision", "kind:convention"])
    );
    assert_eq!(recalls[0]["tags_match"], "any_strict");
}

#[test]
fn reviewer_recall_asks_only_for_the_repos_conventions() {
    let f = Fixture::new();
    *f.bank.curated.lock().unwrap() = vec![result(
        "convention-1",
        "Tests live in tests/.",
        &["kind:convention", "repo:RobTand/tl3-sandbox"],
    )];
    *f.bank.general.lock().unwrap() = vec![result("run-1", "unrelated", &["role:maker"])];
    let value = f.recall(Recall::Conventions {
        repo: "RobTand/tl3-sandbox",
        query: "Return hello.",
    });
    assert_eq!(
        value["memories"],
        json!([{"id": "convention-1", "kind": "convention", "content": "Tests live in tests/."}])
    );
    let recalls = f.bank.recalls.lock().unwrap();
    assert_eq!(recalls.len(), 1, "reviewers get conventions only");
    assert_eq!(
        recalls[0]["tag_groups"],
        json!([
            {"tags": ["kind:convention"], "match": "any_strict"},
            {"tags": ["repo:RobTand/tl3-sandbox"], "match": "any_strict"},
        ])
    );
}

#[test]
fn recall_outage_degrades_to_no_memories_with_a_note() {
    let f = Fixture::new();
    *f.bank.general.lock().unwrap() = vec![result("run-1", "text", &[])];
    let mut down = f.hindsight();
    down.endpoint = "http://127.0.0.1:9".parse().unwrap();
    let value = memory::recall(&mut f.store(), &down, context()).unwrap();
    assert_eq!(value["memories"], json!([]));
    assert!(
        value["memory_note"]
            .as_str()
            .unwrap()
            .contains("unavailable")
    );

    for status in [503, 1] {
        *f.bank.status.lock().unwrap() = status;
        let value = f.recall(context());
        assert_eq!(value["memories"], json!([]), "status {status}");
        assert!(value["memory_note"].is_string());
    }
    *f.bank.status.lock().unwrap() = 0;

    let mut missing_key = f.hindsight();
    missing_key.api_key_file = Some(f.scratch.path().join("absent-key"));
    let value = memory::recall(&mut f.store(), &missing_key, context()).unwrap();
    assert_eq!(value["memories"], json!([]));
    assert!(value["memory_note"].is_string());

    *f.bank.hung.lock().unwrap() = true;
    let started = Instant::now();
    let value = f.recall(context());
    assert_eq!(value["memories"], json!([]));
    assert!(value["memory_note"].is_string());
    assert!(
        started.elapsed() < Duration::from_secs(20),
        "a hung Hindsight must not hold the controller: {:?}",
        started.elapsed()
    );
}

#[test]
fn verified_flag_invalidates_the_memory_in_hindsight() {
    let f = Fixture::new();
    let attempt = f.attempt(
        "planner",
        supplied("wrong-1", "lesson"),
        json!({"kind": "cant_meet", "report": "x", "files": [], "memory_ids": ["wrong-1"],
               "memory_flags": [flag("wrong-1", evidence())]}),
    );
    memory::queue_attempt(&mut f.store(), "run:planner", &attempt).unwrap();
    let curations = f.curations();
    assert_eq!(curations.len(), 1);
    assert_eq!(curations[0]["memory_id"], "wrong-1");
    assert_eq!(curations[0]["verdict"], "accepted");
    assert_eq!(curations[0]["state"], "invalidated");
    assert_eq!(curations[0]["source"], "planner");
    assert_eq!(curations[0]["request"], "3");
    assert_eq!(curations[0]["evidence"]["quote"], QUOTE);
    assert!(curations[0]["delivered_at"].is_null());

    // Until Hindsight has it, recall drops the memory itself.
    *f.bank.general.lock().unwrap() = vec![result("wrong-1", "greet() returns 'hi'.", &[])];
    assert_eq!(f.recall(context())["memories"], json!([]));

    memory::flush(&mut f.store(), &f.hindsight()).unwrap();
    let sent = f.bank.curations.lock().unwrap().clone();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].0, "wrong-1");
    assert_eq!(sent[0].1["state"], "invalidated");
    let reason = sent[0].1["reason"].as_str().unwrap();
    assert!(reason.contains("greet() returns 'hello', not 'hi'."));
    assert!(reason.contains("greeting.py:1-1"));
    assert!(reason.contains(QUOTE));
    assert!(!f.curations()[0]["delivered_at"].is_null());
    memory::flush(&mut f.store(), &f.hindsight()).unwrap();
    assert_eq!(f.bank.curations.lock().unwrap().len(), 1, "delivered once");

    // Replaying the retirement does not record or send it again.
    memory::queue_attempt(&mut f.store(), "run:planner", &attempt).unwrap();
    memory::flush(&mut f.store(), &f.hindsight()).unwrap();
    assert_eq!(f.curations().len(), 1);
    assert_eq!(f.bank.curations.lock().unwrap().len(), 1);
}

#[test]
fn unverified_flags_are_recorded_as_rejected_and_never_sent() {
    let f = Fixture::new();
    let cases = [
        (
            "not-supplied",
            "lesson",
            json!(["wrong-1"]),
            "other-id",
            evidence(),
        ),
        ("not-cited", "lesson", json!([]), "wrong-1", evidence()),
        (
            "quote-absent",
            "lesson",
            json!(["wrong-1"]),
            "wrong-1",
            json!({"path": "greeting.py", "start_line": 1, "end_line": 2, "quote": "def greet(): return 'hi'"}),
        ),
        (
            "wrong-lines",
            "lesson",
            json!(["wrong-1"]),
            "wrong-1",
            json!({"path": "greeting.py", "start_line": 4, "end_line": 5, "quote": QUOTE}),
        ),
        (
            "escape",
            "lesson",
            json!(["wrong-1"]),
            "wrong-1",
            json!({"path": "../not-supplied/worktree/greeting.py", "start_line": 1, "end_line": 1, "quote": QUOTE}),
        ),
        (
            "git-metadata",
            "lesson",
            json!(["wrong-1"]),
            "wrong-1",
            json!({"path": ".git/greeting.py", "start_line": 1, "end_line": 1, "quote": QUOTE}),
        ),
        (
            "symlink",
            "lesson",
            json!(["wrong-1"]),
            "wrong-1",
            json!({"path": "src/link.py", "start_line": 1, "end_line": 1, "quote": QUOTE}),
        ),
        (
            "robs-decision",
            "decision",
            json!(["wrong-1"]),
            "wrong-1",
            evidence(),
        ),
    ];
    let count = cases.len();
    for (case, kind, cited, flagged, evidence) in cases {
        let attempt = f.attempt_in(
            case,
            "planner",
            supplied("wrong-1", kind),
            json!({"kind": "brief", "brief": "b", "goals": [], "memory_ids": cited,
                   "memory_flags": [flag(flagged, evidence)]}),
        );
        let worktree = attempt.parent().unwrap().join("worktree");
        std::os::unix::fs::symlink(worktree.join("greeting.py"), worktree.join("src/link.py"))
            .unwrap();
        fs::create_dir_all(worktree.join(".git")).unwrap();
        fs::copy(
            worktree.join("greeting.py"),
            worktree.join(".git/greeting.py"),
        )
        .unwrap();
        memory::queue_attempt(&mut f.store(), &format!("run:{case}"), &attempt).unwrap();
    }
    let curations = f.curations();
    assert_eq!(curations.len(), count);
    for curation in &curations {
        assert_eq!(curation["verdict"], "rejected", "{curation}");
        assert!(!curation["reason"].as_str().unwrap().is_empty());
    }
    memory::flush(&mut f.store(), &f.hindsight()).unwrap();
    assert!(f.bank.curations.lock().unwrap().is_empty());
    *f.bank.general.lock().unwrap() = vec![result("wrong-1", "kept", &[])];
    assert_eq!(f.recall(context())["memories"][0]["id"], "wrong-1");
}

#[test]
fn failed_runs_and_other_roles_never_flag() {
    let f = Fixture::new();
    let attempt = f.attempt(
        "maker",
        supplied("wrong-1", "lesson"),
        json!({"kind": "done", "memory_ids": ["wrong-1"], "memory_flags": [flag("wrong-1", evidence())]}),
    );
    memory::queue_attempt(&mut f.store(), "run:maker", &attempt).unwrap();
    let attempt = f.attempt(
        "ingest",
        supplied("wrong-1", "lesson"),
        json!({"kind": "brief", "brief": "b", "goals": [], "memory_ids": ["wrong-1"],
               "memory_flags": [flag("wrong-1", evidence())]}),
    );
    fs::write(attempt.join("exit.json"), r#"{"code":1}"#).unwrap();
    memory::queue_attempt(&mut f.store(), "run:ingest", &attempt).unwrap();
    assert!(f.curations().is_empty());
}

#[test]
fn rob_can_invalidate_and_revert() {
    let f = Fixture::new();
    memory::rob_curation(
        &mut f.store(),
        "rob-1",
        CurationState::Invalidated,
        "Wrong since #2638.",
    )
    .unwrap();
    memory::rob_curation(
        &mut f.store(),
        "rob-1",
        CurationState::Valid,
        "It was right after all.",
    )
    .unwrap();
    let curations = f.curations();
    assert_eq!(curations.len(), 2);
    assert!(
        curations
            .iter()
            .all(|c| c["source"] == "rob" && c["verdict"] == "accepted")
    );
    // The latest decision wins locally before delivery.
    *f.bank.general.lock().unwrap() = vec![result("rob-1", "kept", &[])];
    assert_eq!(f.recall(context())["memories"][0]["id"], "rob-1");
    memory::flush(&mut f.store(), &f.hindsight()).unwrap();
    let sent = f.bank.curations.lock().unwrap().clone();
    assert_eq!(
        sent.iter()
            .map(|(id, body)| (id.as_str(), body["state"].as_str().unwrap()))
            .collect::<Vec<_>>(),
        [("rob-1", "invalidated"), ("rob-1", "valid")]
    );
    assert!(
        sent[1].1["reason"]
            .as_str()
            .unwrap()
            .contains("right after all")
    );
}

#[test]
fn missing_memory_parks_its_curation() {
    let f = Fixture::new();
    memory::rob_curation(
        &mut f.store(),
        "gone-1",
        CurationState::Invalidated,
        "Wrong.",
    )
    .unwrap();
    *f.bank.status.lock().unwrap() = 404;
    memory::flush(&mut f.store(), &f.hindsight()).unwrap();
    assert_eq!(f.curations()[0]["http_status"], 404);
    *f.bank.status.lock().unwrap() = 0;
    memory::flush(&mut f.store(), &f.hindsight()).unwrap();
    assert!(
        f.bank.curations.lock().unwrap().is_empty(),
        "parked rows are not retried"
    );
}

#[test]
fn verified_conventions_are_retained_with_tags() {
    let f = Fixture::new();
    let attempt = f.attempt(
        "planner",
        json!([]),
        json!({"kind": "cant_meet", "report": "x", "files": [], "memory_ids": [],
               "conventions": [
                   {"text": "Greetings are plain string literals.", "evidence": evidence()},
                   {"text": "Unproven.", "evidence": {"path": "greeting.py", "start_line": 1, "end_line": 1, "quote": "absent text here"}},
               ]}),
    );
    let mut store = f.store();
    memory::queue_attempt(&mut store, "run:planner", &attempt).unwrap();
    let items: Vec<Value> = store
        .pending_memories(0)
        .unwrap()
        .into_iter()
        .map(|(_, _, item)| item)
        .collect();
    assert_eq!(items.len(), 2, "the run record and one verified convention");
    let run = items
        .iter()
        .find(|item| item["context"] == "TL3 Deep Agents run summary")
        .unwrap();
    assert!(run["tags"].as_array().unwrap().contains(&json!("kind:run")));
    assert!(
        run["tags"]
            .as_array()
            .unwrap()
            .contains(&json!("source:run"))
    );
    let convention = items
        .iter()
        .find(|item| item["context"] != "TL3 Deep Agents run summary")
        .unwrap();
    let tags = convention["tags"].as_array().unwrap();
    for tag in [
        "repo:RobTand/tl3-sandbox",
        "kind:convention",
        "source:planner",
    ] {
        assert!(tags.contains(&json!(tag)), "{tags:?}");
    }
    let content = convention["content"].as_str().unwrap();
    assert!(content.contains("Greetings are plain string literals."));
    assert!(content.contains("greeting.py:1-1"));
    assert!(!content.contains("Unproven."));
}

#[test]
fn lessons_queue_once_with_agreed_tags() {
    let f = Fixture::new();
    let lesson = memory::Lesson {
        kind: memory::LessonKind::Decision,
        source: "rob",
        repo: "RobTand/tl3-sandbox",
        topic: Some("narrowing"),
        text: "Question: split or narrow?\nAnswer: narrow.\nReason: keep it small.",
        identity: "comment:42:7",
    };
    let mut store = f.store();
    memory::queue_lesson(&mut store, &lesson).unwrap();
    memory::queue_lesson(&mut store, &lesson).unwrap();
    let items = store.pending_memories(0).unwrap();
    assert_eq!(items.len(), 1, "a replayed lesson keeps its identity");
    let tags = items[0].2["tags"].as_array().unwrap().clone();
    assert_eq!(
        tags,
        [
            json!("repo:RobTand/tl3-sandbox"),
            json!("kind:decision"),
            json!("source:rob"),
            json!("topic:narrowing"),
        ]
    );
}
