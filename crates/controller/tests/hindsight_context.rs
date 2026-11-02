//! #422: recall into initial context, and retiring memories shown to be wrong.
#[path = "../../store/tests/support/mod.rs"]
mod support;

use serde_json::{Value, json};
use std::{
    fs,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tl3_controller::memory::{self, Breaker, Recall, RobCuration};
use tl3_store::Store;
use wiremock::{
    Mock, MockServer, Request, Respond, ResponseTemplate,
    matchers::{method, path, path_regex},
};

const KEY: &str = "fixture-recall-authentication";
const QUOTE: &str = "def greet_user(): return 'hello'";

#[derive(Clone, Default)]
struct Bank {
    /// Results for the lessons-first call, which filters on curated kinds.
    curated: Arc<Mutex<Vec<Value>>>,
    /// Results for the unfiltered call.
    general: Arc<Mutex<Vec<Value>>>,
    recalls: Arc<Mutex<Vec<Value>>>,
    curations: Arc<Mutex<Vec<(String, Value)>>>,
    /// Every request that reached the bank, including hung ones.
    hits: Arc<Mutex<usize>>,
    /// 0 answers normally; any other status is returned as is; 1 sends a malformed body.
    status: Arc<Mutex<u16>>,
    hung: Arc<Mutex<bool>>,
}
impl Respond for Bank {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        *self.hits.lock().unwrap() += 1;
        if *self.hung.lock().unwrap() {
            return ResponseTemplate::new(200).set_delay(Duration::from_secs(60));
        }
        let body: Value = request.body_json().unwrap();
        if request.url.path().ends_with("/recall") {
            // Regression fixtures use space-separated single-letter tokens.
            let tokens = body["query"].as_str().unwrap().split_whitespace().count();
            if tokens > 500 {
                return ResponseTemplate::new(400).set_body_json(json!({
                    "detail": format!("Query too long: {tokens} tokens exceeds maximum of 500. Please shorten your query.")
                }));
            }
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
    /// Each call gets a fresh breaker, as after a successful delivery.
    fn recall(&self, recall: Recall<'_>) -> Value {
        memory::recall(
            &mut self.store(),
            &self.hindsight(),
            &Breaker::default(),
            recall,
        )
        .unwrap()
    }
    /// A finished run laid out as the supervisor leaves it: attempt beside its base worktree.
    fn attempt(&self, role: &str, memories: Value, reply: Value) -> std::path::PathBuf {
        self.attempt_in(role, role, "3", memories, reply)
    }
    fn attempt_in(
        &self,
        directory: &str,
        role: &str,
        request: &str,
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
        fs::write(
            worktree.join("settings.py"),
            "database = connect()\ndef handler(): raise error\n",
        )
        .unwrap();
        fs::write(
            worktree.join("measure_served_gold.py"),
            "import hashlib\ndef _sha256_file(path):\n    return hashlib.sha256(open(path, 'rb').read()).hexdigest()\n",
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
                "memory": {"repo": "RobTand/tl3-sandbox", "issue": 42, "request": request, "epoch": 0},
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

const CLAIM: &str = "greet_user() returns 'hi'";
const REPO: &str = "RobTand/tl3-sandbox";

/// A supplied memory as recall leaves it in the run's inputs, with its repo tag.
fn supplied(id: &str, kind: &str, repo: Option<&str>) -> Value {
    let mut memory = json!({"id": id, "kind": kind, "content": "Earlier: greet_user() returns 'hi' to callers."});
    if let Some(repo) = repo {
        memory["repo"] = json!(repo);
    }
    json!([memory])
}

fn evidence() -> Value {
    json!({"path": "greeting.py", "start_line": 1, "end_line": 1, "quote": QUOTE})
}

fn flag(id: &str, claim: &str, evidence: Value) -> Value {
    json!({"id": id, "claim": claim, "reason": "greet() returns 'hello', not 'hi'.", "evidence": evidence})
}

fn planner_flag(id: &str, claim: &str, evidence: Value) -> Value {
    json!({"kind": "cant_meet", "report": "x", "files": [], "memory_ids": [id],
           "memory_flags": [flag(id, claim, evidence)]})
}

/// Collects operations-issue notes instead of posting them.
#[derive(Default)]
struct Notes(Mutex<Vec<(String, String)>>);
impl memory::Announce for Notes {
    fn post(&self, key: &str, body: &str) -> tl3_controller::Result<()> {
        self.0.lock().unwrap().push((key.into(), body.into()));
        Ok(())
    }
}

fn context<'a>() -> Recall<'a> {
    Recall::Context {
        repo: "RobTand/tl3-sandbox",
        query: "Fix greeting\nReturn hello.",
    }
}

#[test]
fn fake_hindsight_rejects_over_500_tokens_with_the_live_error() {
    let f = Fixture::new();
    let response = f._runtime.block_on(async {
        reqwest::Client::new()
            .post(format!("{}/v1/default/banks/tl3/memories/recall", f.server.uri()))
            .json(&json!({"query": vec!["x"; 1449].join(" ")}))
            .send()
            .await
            .unwrap()
    });
    assert_eq!(response.status(), 400);
    let body: Value = f._runtime.block_on(response.json()).unwrap();
    assert_eq!(body, json!({"detail": "Query too long: 1449 tokens exceeds maximum of 500. Please shorten your query."}));
}

#[test]
fn long_issue_recall_keeps_repo_title_and_body_start_within_the_query_limit() {
    let f = Fixture::new();
    *f.bank.curated.lock().unwrap() = vec![result("lesson-1", "Keep greeting tests.", &["kind:lesson"])];
    for body in ["x ".repeat(2000), "界🦀 ".repeat(2000)] {
        let query = format!("Fix greeting\nReturn hello. {body} END-OF-ISSUE");
        for recall in [
            Recall::Context { repo: REPO, query: &query },
            Recall::Conventions { repo: REPO, query: &query },
        ] {
            let value = f.recall(recall);
            assert!(value.get("memory_note").is_none(), "long issue must recall memories: {value}");
            assert_eq!(value["memories"][0]["id"], "lesson-1");
        }
    }
    let recalls = f.bank.recalls.lock().unwrap();
    assert_eq!(recalls.len(), 6);
    for body in recalls.iter() {
        let query = body["query"].as_str().unwrap();
        assert!(query.starts_with(&format!("Repository {REPO}\nFix greeting\nReturn hello.")), "{query}");
        assert!(query.len() <= 400, "query must leave margin under 500 tokens: {} bytes", query.len());
        assert!(!query.contains("END-OF-ISSUE"));
    }
}

#[test]
fn recall_bad_request_does_not_pause_the_next_request() {
    let f = Fixture::new();
    let breaker = Breaker::default();
    *f.bank.status.lock().unwrap() = 400;
    let value = memory::recall(&mut f.store(), &f.hindsight(), &breaker, context()).unwrap();
    assert_eq!(value["memories"], json!([]));
    assert!(value["memory_note"].as_str().unwrap().contains("unavailable"));
    assert_eq!(*f.bank.hits.lock().unwrap(), 1);
    *f.bank.status.lock().unwrap() = 0;
    *f.bank.general.lock().unwrap() = vec![result("run-1", "text", &[])];
    let value = memory::recall(&mut f.store(), &f.hindsight(), &breaker, context()).unwrap();
    assert!(value.get("memory_note").is_none(), "a 400 must not open the breaker: {value}");
    assert_eq!(value["memories"][0]["id"], "run-1");
    assert_eq!(*f.bank.hits.lock().unwrap(), 3);
}

#[test]
fn recall_connection_failure_and_server_errors_pause_the_next_request() {
    let f = Fixture::new();
    for status in [0, 500, 503] {
        let breaker = Breaker::default();
        let mut down = f.hindsight();
        let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        if status == 0 {
            down.endpoint = format!("http://{}", closed.local_addr().unwrap()).parse().unwrap();
        }
        drop(closed);
        *f.bank.status.lock().unwrap() = status;
        let value = memory::recall(&mut f.store(), &down, &breaker, context()).unwrap();
        assert!(value["memory_note"].as_str().unwrap().contains("unavailable"));
        let hits = *f.bank.hits.lock().unwrap();
        *f.bank.status.lock().unwrap() = 0;
        let value = memory::recall(&mut f.store(), &f.hindsight(), &breaker, context()).unwrap();
        assert!(value["memory_note"].as_str().unwrap().contains("paused"), "status {status}: {value}");
        assert_eq!(*f.bank.hits.lock().unwrap(), hits);
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
    assert_eq!(memories[0]["repo"], REPO, "recall keeps the repo tag");
    assert!(
        memories[3].get("repo").is_none(),
        "an untagged memory has no repo"
    );
    for memory in memories {
        let keys: Vec<_> = memory.as_object().unwrap().keys().collect();
        assert!(
            keys == ["content", "id", "kind"] || keys == ["content", "id", "kind", "repo"],
            "{keys:?}"
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
        json!([{"id": "convention-1", "kind": "convention", "repo": REPO, "content": "Tests live in tests/."}])
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
    let value = memory::recall(&mut f.store(), &down, &Breaker::default(), context()).unwrap();
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
    let value =
        memory::recall(&mut f.store(), &missing_key, &Breaker::default(), context()).unwrap();
    assert_eq!(value["memories"], json!([]));
    assert!(value["memory_note"].is_string());
}

#[test]
fn a_hung_hindsight_costs_one_launch_until_a_delivery_succeeds() {
    let f = Fixture::new();
    *f.bank.general.lock().unwrap() = vec![result("run-1", "text", &[])];
    let breaker = Breaker::default();
    *f.bank.hung.lock().unwrap() = true;
    let started = Instant::now();
    let value = memory::recall(&mut f.store(), &f.hindsight(), &breaker, context()).unwrap();
    assert_eq!(value["memories"], json!([]));
    assert!(value["memory_note"].is_string());
    assert!(
        started.elapsed() < Duration::from_secs(12),
        "{:?}",
        started.elapsed()
    );
    let hits = *f.bank.hits.lock().unwrap();
    assert_eq!(hits, 1, "one failed call stops the launch's second call");

    // Later launches skip recall at once, without touching Hindsight.
    for _ in 0..3 {
        let started = Instant::now();
        let value = memory::recall(&mut f.store(), &f.hindsight(), &breaker, context()).unwrap();
        assert_eq!(value["memories"], json!([]));
        assert!(value["memory_note"].is_string());
        assert!(started.elapsed() < Duration::from_secs(1));
    }
    assert_eq!(*f.bank.hits.lock().unwrap(), hits);

    // Hindsight answers again, but nothing has shown it: recall stays off.
    *f.bank.hung.lock().unwrap() = false;
    memory::deliver(&mut f.store(), &f.hindsight(), &breaker).unwrap();
    let value = memory::recall(&mut f.store(), &f.hindsight(), &breaker, context()).unwrap();
    assert_eq!(
        value["memories"],
        json!([]),
        "an empty delivery is not evidence"
    );

    // A successful retain is the event that lets the next launch try again.
    let attempt = f.attempt("audit", json!([]), json!({"verdict": "agree"}));
    memory::queue_attempt(&mut f.store(), "run:audit", &attempt).unwrap();
    assert_eq!(
        memory::deliver(&mut f.store(), &f.hindsight(), &breaker).unwrap(),
        1
    );
    let value = memory::recall(&mut f.store(), &f.hindsight(), &breaker, context()).unwrap();
    assert!(value.get("memory_note").is_none(), "{value}");
    assert_eq!(value["memories"][0]["id"], "run-1");
}

#[test]
fn one_verified_flag_hides_and_a_second_run_retires() {
    let f = Fixture::new();
    let notes = Notes::default();
    let first = f.attempt_in(
        "first",
        "planner",
        "3",
        supplied("wrong-1", "lesson", Some(REPO)),
        planner_flag("wrong-1", CLAIM, evidence()),
    );
    memory::queue_attempt(&mut f.store(), "run:first", &first).unwrap();
    let curations = f.curations();
    assert_eq!(curations.len(), 1);
    assert_eq!(curations[0]["memory_id"], "wrong-1");
    assert_eq!(curations[0]["verdict"], "accepted");
    assert_eq!(curations[0]["state"], "hidden");
    assert_eq!(curations[0]["source"], "planner");
    assert_eq!(curations[0]["request"], "3");
    assert_eq!(curations[0]["claim"], CLAIM);
    assert_eq!(curations[0]["evidence"]["quote"], QUOTE);

    // Hidden at once from TL3's recall; Hindsight is not told yet.
    *f.bank.general.lock().unwrap() = vec![result(
        "wrong-1",
        "Earlier: greet_user() returns 'hi'.",
        &[],
    )];
    assert_eq!(f.recall(context())["memories"], json!([]));
    memory::flush(&mut f.store(), &f.hindsight()).unwrap();
    assert!(
        f.bank.curations.lock().unwrap().is_empty(),
        "one signal never retires"
    );

    // A second run of the same request is not independent.
    let again = f.attempt_in(
        "again",
        "ingest",
        "3",
        supplied("wrong-1", "lesson", Some(REPO)),
        json!({"kind": "brief", "brief": "b", "goals": [], "memory_ids": ["wrong-1"],
               "memory_flags": [flag("wrong-1", CLAIM, evidence())]}),
    );
    memory::queue_attempt(&mut f.store(), "run:again", &again).unwrap();
    memory::flush(&mut f.store(), &f.hindsight()).unwrap();
    assert!(f.bank.curations.lock().unwrap().is_empty());
    assert!(f.curations().iter().all(|c| c["state"] == "hidden"));

    // Another request's run is the second signal.
    let second = f.attempt_in(
        "second",
        "planner",
        "9",
        supplied("wrong-1", "lesson", Some(REPO)),
        planner_flag("wrong-1", CLAIM, evidence()),
    );
    memory::queue_attempt(&mut f.store(), "run:second", &second).unwrap();
    assert_eq!(f.curations().last().unwrap()["state"], "invalidated");
    memory::flush(&mut f.store(), &f.hindsight()).unwrap();
    let sent = f.bank.curations.lock().unwrap().clone();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].0, "wrong-1");
    assert_eq!(sent[0].1["state"], "invalidated");
    let reason = sent[0].1["reason"].as_str().unwrap();
    assert!(reason.contains(CLAIM) && reason.contains("greeting.py:1-1") && reason.contains(QUOTE));
    memory::flush(&mut f.store(), &f.hindsight()).unwrap();
    assert_eq!(f.bank.curations.lock().unwrap().len(), 1, "delivered once");

    // Replays record and send nothing new.
    for (key, attempt) in [("run:first", &first), ("run:second", &second)] {
        memory::queue_attempt(&mut f.store(), key, attempt).unwrap();
    }
    memory::flush(&mut f.store(), &f.hindsight()).unwrap();
    assert_eq!(f.curations().len(), 3);
    assert_eq!(f.bank.curations.lock().unwrap().len(), 1);

    // Every hide and retire is one line on the operations issue, once.
    assert_eq!(memory::announce(&mut f.store(), &notes).unwrap(), 3);
    assert_eq!(memory::announce(&mut f.store(), &notes).unwrap(), 0);
    let notes = notes.0.lock().unwrap();
    assert_eq!(notes.len(), 3);
    for (key, body) in notes.iter() {
        assert!(!key.is_empty());
        assert_eq!(body.lines().count(), 1, "{body}");
        for part in ["wrong-1", CLAIM, "greeting.py:1-1", "run "] {
            assert!(body.contains(part), "{part} missing from {body}");
        }
        assert!(!body.contains('@'));
    }
    assert!(notes[0].1.contains("hidden") && notes[2].1.contains("retired"));
    assert!(f.curations().iter().all(|c| !c["announced_at"].is_null()));
}

#[test]
fn unverified_flags_are_recorded_as_rejected_and_never_hide() {
    let f = Fixture::new();
    let lines = |start: u64, end: u64, quote: &str| json!({"path": "greeting.py", "start_line": start, "end_line": end, "quote": quote});
    let at = |path: &str| json!({"path": path, "start_line": 1, "end_line": 1, "quote": QUOTE});
    let cases = [
        (
            "not-supplied",
            "lesson",
            Some(REPO),
            json!(["wrong-1"]),
            "other-id",
            CLAIM,
            evidence(),
        ),
        (
            "not-cited",
            "lesson",
            Some(REPO),
            json!([]),
            "wrong-1",
            CLAIM,
            evidence(),
        ),
        (
            "quote-absent",
            "lesson",
            Some(REPO),
            json!(["wrong-1"]),
            "wrong-1",
            CLAIM,
            lines(1, 2, "def greet_user(): return 'hi'"),
        ),
        (
            "wrong-lines",
            "lesson",
            Some(REPO),
            json!(["wrong-1"]),
            "wrong-1",
            CLAIM,
            lines(4, 5, QUOTE),
        ),
        // A real line of the repo, but not about the memory's claim.
        (
            "unrelated-line",
            "lesson",
            Some(REPO),
            json!(["wrong-1"]),
            "wrong-1",
            CLAIM,
            lines(4, 5, "def wave(): return 'bye'"),
        ),
        (
            "claim-not-in-memory",
            "lesson",
            Some(REPO),
            json!(["wrong-1"]),
            "wrong-1",
            "greet() raises an error",
            evidence(),
        ),
        (
            "cross-repo",
            "lesson",
            Some("RobTand/prismaquant"),
            json!(["wrong-1"]),
            "wrong-1",
            CLAIM,
            evidence(),
        ),
        (
            "no-repo-tag",
            "lesson",
            None,
            json!(["wrong-1"]),
            "wrong-1",
            CLAIM,
            evidence(),
        ),
        (
            "escape",
            "lesson",
            Some(REPO),
            json!(["wrong-1"]),
            "wrong-1",
            CLAIM,
            at("../not-supplied/worktree/greeting.py"),
        ),
        (
            "git-metadata",
            "lesson",
            Some(REPO),
            json!(["wrong-1"]),
            "wrong-1",
            CLAIM,
            at(".git/greeting.py"),
        ),
        (
            "symlink",
            "lesson",
            Some(REPO),
            json!(["wrong-1"]),
            "wrong-1",
            CLAIM,
            at("src/link.py"),
        ),
        (
            "robs-decision",
            "decision",
            Some(REPO),
            json!(["wrong-1"]),
            "wrong-1",
            CLAIM,
            evidence(),
        ),
    ];
    let count = cases.len();
    for (case, kind, repo, cited, flagged, claim, evidence) in cases {
        let attempt = f.attempt_in(
            case,
            "planner",
            case,
            supplied("wrong-1", kind, repo),
            json!({"kind": "cant_meet", "report": "x", "files": [], "memory_ids": cited,
                   "memory_flags": [flag(flagged, claim, evidence)]}),
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
    assert_eq!(
        f.recall(context())["memories"][0]["id"],
        "wrong-1",
        "nothing was hidden"
    );
    let notes = Notes::default();
    assert_eq!(
        memory::announce(&mut f.store(), &notes).unwrap(),
        0,
        "rejections are not posted"
    );
}

#[test]
fn generic_claims_and_other_files_never_hide_but_identifiers_do() {
    let f = Fixture::new();
    let at = |path: &str, start: u64, end: u64, quote: &str| json!({"path": path, "start_line": start, "end_line": end, "quote": quote});
    let memory = |content: &str| json!([{"id": "wrong-1", "kind": "lesson", "repo": REPO, "content": content}]);
    let generic = "Requests fail when the database is down, and the error handler retries.";
    let cases = [
        // A generic word pairs with any line that happens to contain it.
        (
            "database",
            memory(generic),
            "when the database is down",
            at("settings.py", 1, 1, "database = connect()"),
        ),
        (
            "error",
            memory(generic),
            "the error handler retries",
            at("settings.py", 2, 2, "def handler(): raise error"),
        ),
        // The memory names its file; evidence from another file proves nothing.
        (
            "other-file",
            memory("farewell.py: greet_user() returns 'hi'."),
            CLAIM,
            evidence(),
        ),
    ];
    for (case, supplied, claim, evidence) in cases {
        let attempt = f.attempt_in(
            case,
            "planner",
            case,
            supplied,
            json!({"kind": "cant_meet", "report": "x", "files": [], "memory_ids": ["wrong-1"],
                   "memory_flags": [flag("wrong-1", claim, evidence)]}),
        );
        memory::queue_attempt(&mut f.store(), &format!("run:{case}"), &attempt).unwrap();
    }
    let curations = f.curations();
    assert_eq!(curations.len(), 3);
    for curation in &curations {
        assert_eq!(curation["verdict"], "rejected", "{curation}");
    }

    let claim = "measure_served_gold._sha256_file hashes with md5";
    let attempt = f.attempt_in("identifier", "planner", "identifier",
        memory("measure_served_gold._sha256_file hashes with md5, so pins drift."),
        json!({"kind": "cant_meet", "report": "x", "files": [], "memory_ids": ["wrong-1"],
               "memory_flags": [flag("wrong-1", claim, at("measure_served_gold.py", 2, 3,
                   "def _sha256_file(path): return hashlib.sha256(open(path, 'rb').read()).hexdigest()"))]}));
    memory::queue_attempt(&mut f.store(), "run:identifier", &attempt).unwrap();
    let last = f.curations().pop().unwrap();
    assert_eq!(last["verdict"], "accepted", "{last}");
    assert_eq!(last["state"], "hidden");
}

#[test]
fn failed_runs_and_other_roles_never_flag() {
    let f = Fixture::new();
    let attempt = f.attempt("maker", supplied("wrong-1", "lesson", Some(REPO)),
        json!({"kind": "done", "memory_ids": ["wrong-1"], "memory_flags": [flag("wrong-1", CLAIM, evidence())]}));
    memory::queue_attempt(&mut f.store(), "run:maker", &attempt).unwrap();
    let attempt = f.attempt(
        "ingest",
        supplied("wrong-1", "lesson", Some(REPO)),
        json!({"kind": "brief", "brief": "b", "goals": [], "memory_ids": ["wrong-1"],
               "memory_flags": [flag("wrong-1", CLAIM, evidence())]}),
    );
    fs::write(attempt.join("exit.json"), r#"{"code":1}"#).unwrap();
    memory::queue_attempt(&mut f.store(), "run:ingest", &attempt).unwrap();
    assert!(f.curations().is_empty());
}

#[test]
fn rob_confirms_invalidates_and_reverts() {
    let f = Fixture::new();
    assert!(
        memory::rob_curation(&mut f.store(), "wrong-1", RobCuration::Confirm, "Yes.").is_err(),
        "nothing hidden to confirm"
    );
    let attempt = f.attempt(
        "planner",
        supplied("wrong-1", "lesson", Some(REPO)),
        planner_flag("wrong-1", CLAIM, evidence()),
    );
    memory::queue_attempt(&mut f.store(), "run:planner", &attempt).unwrap();
    memory::rob_curation(
        &mut f.store(),
        "wrong-1",
        RobCuration::Confirm,
        "Confirmed wrong.",
    )
    .unwrap();
    memory::rob_curation(
        &mut f.store(),
        "rob-1",
        RobCuration::Invalidate,
        "Wrong since #2638.",
    )
    .unwrap();
    memory::rob_curation(
        &mut f.store(),
        "rob-1",
        RobCuration::Revert,
        "It was right after all.",
    )
    .unwrap();
    let rob: Vec<Value> = f
        .curations()
        .into_iter()
        .filter(|c| c["source"] == "rob")
        .collect();
    assert_eq!(rob.len(), 3);
    assert!(rob.iter().all(|c| c["verdict"] == "accepted"));
    // The latest decision wins locally before delivery.
    *f.bank.general.lock().unwrap() =
        vec![result("rob-1", "kept", &[]), result("wrong-1", "gone", &[])];
    let memories = f.recall(context())["memories"].clone();
    assert_eq!(memories.as_array().unwrap().len(), 1);
    assert_eq!(memories[0]["id"], "rob-1");
    memory::flush(&mut f.store(), &f.hindsight()).unwrap();
    let sent = f.bank.curations.lock().unwrap().clone();
    assert_eq!(
        sent.iter()
            .map(|(id, body)| (id.as_str(), body["state"].as_str().unwrap()))
            .collect::<Vec<_>>(),
        [
            ("wrong-1", "invalidated"),
            ("rob-1", "invalidated"),
            ("rob-1", "valid")
        ]
    );
    assert!(
        sent[2].1["reason"]
            .as_str()
            .unwrap()
            .contains("right after all")
    );
}

#[test]
fn missing_memory_parks_its_curation() {
    let f = Fixture::new();
    memory::rob_curation(&mut f.store(), "gone-1", RobCuration::Invalidate, "Wrong.").unwrap();
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
        repo: REPO,
        topic: Some("narrowing"),
        text: "Question: split or narrow?\nAnswer: narrow.\nReason: keep it small.",
        identity: "comment:42:7",
    };
    let mut store = f.store();
    memory::queue_lesson(&mut store, &lesson).unwrap();
    memory::queue_lesson(&mut store, &lesson).unwrap();
    let items = store.pending_memories(0).unwrap();
    assert_eq!(items.len(), 1, "a replayed lesson keeps its identity");
    assert_eq!(
        items[0].2["tags"],
        json!([
            "repo:RobTand/tl3-sandbox",
            "kind:decision",
            "source:rob",
            "topic:narrowing"
        ])
    );
}

#[test]
fn a_decision_keeps_the_question_the_choice_and_robs_words_for_it() {
    let comment = format!(
        "Continue with the first reading; the second one breaks the API.\n\nAlso, unrelated: {}",
        "please tidy the README later. ".repeat(40)
    );
    let text = memory::decision(
        "Which reading should TL3 follow?",
        &json!({"answer": "continue"}),
        &comment,
    );
    assert!(text.contains("Which reading should TL3 follow?"));
    assert!(text.contains("continue"));
    assert!(text.contains("Continue with the first reading; the second one breaks the API."));
    assert!(!text.contains("tidy the README"), "{text}");
    let guided = memory::decision(
        "How should TL3 retry?",
        &json!({"answer": "try_again", "guidance": "Use the public API."}),
        &comment,
    );
    assert!(guided.contains("Use the public API."));
    assert!(
        !guided.contains("first reading"),
        "the answer's own words replace the comment"
    );
}
