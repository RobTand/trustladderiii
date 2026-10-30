#[path = "../../store/tests/support/mod.rs"]
mod support;

use serde_json::{Value, json};
use std::{collections::BTreeMap, fs, sync::{Arc, Mutex, atomic::{AtomicBool, Ordering}}};
use tl3_controller::memory;
use tl3_store::Store;
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate, matchers::{method, path}};

#[derive(Clone, Default)]
struct Retain {
    documents: Arc<Mutex<BTreeMap<String, Value>>>,
    unavailable: Arc<AtomicBool>,
    lose_ack: Arc<AtomicBool>,
}
impl Respond for Retain {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        if self.unavailable.load(Ordering::SeqCst) {
            return ResponseTemplate::new(503);
        }
        let body: Value = request.body_json().unwrap();
        assert_eq!(body["async"], true);
        let id = body["operation_id"].as_str().unwrap();
        assert_eq!(id.len(), 36);
        let mut documents = self.documents.lock().unwrap();
        if let Some(previous) = documents.insert(id.into(), body["items"][0].clone()) {
            assert_eq!(previous, body["items"][0]);
        }
        if self.lose_ack.swap(false, Ordering::SeqCst) {
            return ResponseTemplate::new(503);
        }
        ResponseTemplate::new(200).set_body_json(json!({
            "success": true, "bank_id": "tl3", "async": true,
            "items_count": 1, "operation_id": id,
        }))
    }
}

struct Fixture {
    database: support::Database,
    settings: tl3_config::Store,
    _runtime: tokio::runtime::Runtime,
    server: MockServer,
    retain: Retain,
    scratch: tempfile::TempDir,
}
impl Fixture {
    fn new() -> Self {
        let mut database = support::Database::start();
        let settings = database.create("memory");
        let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
        let server = runtime.block_on(MockServer::start());
        let retain = Retain::default();
        runtime.block_on(Mock::given(method("POST"))
            .and(path("/v1/default/banks/tl3/memories"))
            .respond_with(retain.clone()).mount(&server));
        Self { database, settings, _runtime: runtime, server, retain, scratch: tempfile::tempdir().unwrap() }
    }
    fn store(&self) -> Store { Store::open(&self.settings).unwrap() }
    fn hindsight(&self) -> tl3_config::Hindsight {
        tl3_config::Hindsight { endpoint: self.server.uri().parse().unwrap(), bank: "tl3".into() }
    }
    fn attempt(&self, role: &str, input: Value, reply: Value) -> std::path::PathBuf {
        let attempt = self.scratch.path().join(role).join("attempt-00000000");
        fs::create_dir_all(&attempt).unwrap();
        fs::write(attempt.join("request.json"), serde_json::to_vec(&json!({
            "role": role, "model": {"name": "fixture-model", "base_url": "http://model.invalid"},
            "operation": {"node": 3, "generation": 7}, "inputs": input,
            "key_file": "/never/read/credentials", "memory": {
                "repo": "RobTand/tl3-sandbox", "issue": 42, "request": "3", "epoch": 0
            }
        })).unwrap()).unwrap();
        fs::write(attempt.join("memory.json"), serde_json::to_vec(&json!({
            "activity": {"files_read": ["greeting.py"], "files_changed": ["greeting.py"],
                "tests": [{"action_key": "pb-fixture-action", "status": "completed", "totals": {"passed": 2}}]},
            "reply": reply, "outcome": "succeeded", "reason": null
        })).unwrap()).unwrap();
        fs::write(attempt.join("exit.json"), r#"{"code":0}"#).unwrap();
        attempt
    }
    fn flush(&self) -> usize { memory::flush(&mut self.store(), &self.hindsight()).unwrap() }
}
impl Drop for Fixture {
    fn drop(&mut self) { self.database.stop(); }
}

#[test]
fn one_record_per_role_with_inputs_activity_reply_and_outcome() {
    let f = Fixture::new();
    let roles: Vec<String> = fs::read_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../../workers/src/tl3_workers/roles"))
        .unwrap().map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .filter_map(|name| name.strip_suffix(".py").map(str::to_owned))
        .filter(|name| name != "base" && !name.starts_with('_')).collect();
    for role in &roles {
        let attempt = f.attempt(role, json!({"goals": ["Fix greeting"]}), json!({"summary": "Fixed greeting"}));
        memory::queue_attempt(&mut f.store(), &format!("run:{role}"), &attempt).unwrap();
    }
    assert_eq!(f.flush(), roles.len());
    assert_eq!(f.flush(), 0, "sent records must be cleaned up");
    let documents = f.retain.documents.lock().unwrap();
    assert_eq!(documents.len(), roles.len());
    for item in documents.values() {
        let record: Value = serde_json::from_str(item["content"].as_str().unwrap()).unwrap();
        assert!(roles.contains(&record["role"].as_str().unwrap().to_owned()));
        assert_eq!(record["model"], "fixture-model");
        assert_eq!(record["repo"], "RobTand/tl3-sandbox");
        assert_eq!(record["issue"], 42);
        assert_eq!(record["request"], "3");
        assert_eq!(record["inputs"]["goals"][0], "Fix greeting");
        assert_eq!(record["activity"]["files_read"][0], "greeting.py");
        assert_eq!(record["activity"]["tests"][0]["action_key"], "pb-fixture-action");
        assert_eq!(record["reply"]["summary"], "Fixed greeting");
        assert_eq!(record["outcome"], "succeeded");
        for tag in ["repo:RobTand/tl3-sandbox", "request:3"] { assert!(item["tags"].as_array().unwrap().contains(&json!(tag))); }
        assert!(item["tags"].as_array().unwrap().contains(&json!(format!("role:{}", record["role"].as_str().unwrap()))));
    }
}

#[test]
fn outage_then_lost_ack_and_retry_retain_exactly_once() {
    let f = Fixture::new();
    let attempt = f.attempt("audit", json!({}), json!({"verdict": "agree"}));
    memory::queue_attempt(&mut f.store(), "run:audit", &attempt).unwrap();
    f.retain.unavailable.store(true, Ordering::SeqCst);
    assert!(memory::flush(&mut f.store(), &f.hindsight()).is_err());
    assert!(f.retain.documents.lock().unwrap().is_empty());
    f.retain.unavailable.store(false, Ordering::SeqCst);
    f.retain.lose_ack.store(true, Ordering::SeqCst);
    assert!(memory::flush(&mut f.store(), &f.hindsight()).is_err());
    assert_eq!(f.flush(), 1);
    assert_eq!(f.flush(), 0);
    assert_eq!(f.retain.documents.lock().unwrap().len(), 1);
}

#[test]
fn crash_between_run_and_retain_preserves_record_and_identity() {
    let mut f = Fixture::new();
    let attempt = f.attempt("maker", json!({"goals": ["Fix greeting"]}), json!({"kind": "done"}));
    // The worker's durable result survives before the controller queues memory.
    memory::queue_attempt(&mut f.store(), "run:maker", &attempt).unwrap();
    f.database.restart();
    f.retain.lose_ack.store(true, Ordering::SeqCst);
    assert!(memory::flush(&mut f.store(), &f.hindsight()).is_err());
    f.database.restart();
    assert_eq!(f.flush(), 1);
    // Retirement may replay after the outbox receipt was already removed.
    memory::queue_attempt(&mut f.store(), "run:maker", &attempt).unwrap();
    f.flush();
    assert_eq!(f.retain.documents.lock().unwrap().len(), 1);
}

#[test]
fn no_credentials_or_tool_contents_are_sent() {
    let f = Fixture::new();
    let attempt = f.attempt("ingest", json!({"goals": ["Fix greeting"], "password": "fixture-password",
        "nested": {"api_key": "fixture-api-key"}, "body": "Authorization: Bearer fixture-bearer"}),
        json!({"text": "https://user:fixture-url-password@example.invalid", "token": "fixture-token",
            "private": "-----BEGIN PRIVATE KEY-----\nfixture-private-key\n-----END PRIVATE KEY-----"}));
    memory::queue_attempt(&mut f.store(), "run:ingest", &attempt).unwrap();
    assert_eq!(f.flush(), 1);
    let sent = serde_json::to_string(&*f.retain.documents.lock().unwrap()).unwrap();
    for secret in ["fixture-password", "fixture-api-key", "fixture-bearer", "fixture-url-password", "fixture-token", "fixture-private-key", "/never/read/credentials", "model.invalid"] {
        assert!(!sent.contains(secret), "leaked {secret}");
    }
    assert!(sent.contains("Fix greeting"));
}

#[test]
fn failed_run_without_summary_is_retained_and_size_is_bounded() {
    let f = Fixture::new();
    let attempt = f.attempt("reviewer", json!({"goals": ["é".repeat(200_000)]}), json!({"summary": "x".repeat(200_000)}));
    fs::remove_file(attempt.join("memory.json")).unwrap();
    fs::write(attempt.join("exit.json"), r#"{"code":1}"#).unwrap();
    memory::queue_attempt(&mut f.store(), "run:reviewer", &attempt).unwrap();
    assert_eq!(f.flush(), 1);
    let documents = f.retain.documents.lock().unwrap();
    let item = documents.values().next().unwrap();
    assert!(serde_json::to_vec(item).unwrap().len() <= 32_768);
    let record: Value = serde_json::from_str(item["content"].as_str().unwrap()).unwrap();
    assert_eq!(record["outcome"], "failed");
    assert!(record["reason"].as_str().unwrap().contains("exit"));
    assert!(record.to_string().contains("truncated"));
}
