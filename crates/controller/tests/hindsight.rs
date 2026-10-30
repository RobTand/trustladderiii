#[path = "../../store/tests/support/mod.rs"]
mod support;

use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};
use tl3_controller::memory;
use tl3_store::Store;
use wiremock::{
    Mock, MockServer, Request, Respond, ResponseTemplate,
    matchers::{method, path},
};

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
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        let server = runtime.block_on(MockServer::start());
        let retain = Retain::default();
        runtime.block_on(
            Mock::given(method("POST"))
                .and(path("/v1/default/banks/tl3/memories"))
                .respond_with(retain.clone())
                .mount(&server),
        );
        Self {
            database,
            settings,
            _runtime: runtime,
            server,
            retain,
            scratch: tempfile::tempdir().unwrap(),
        }
    }
    fn store(&self) -> Store {
        Store::open(&self.settings).unwrap()
    }
    fn hindsight(&self) -> tl3_config::Hindsight {
        tl3_config::Hindsight {
            endpoint: self.server.uri().parse().unwrap(),
            bank: "tl3".into(),
            api_key_file: None,
        }
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
    fn flush(&self) -> usize {
        memory::flush(&mut self.store(), &self.hindsight()).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.database.stop();
    }
}

#[test]
fn one_record_per_role_with_inputs_activity_reply_and_outcome() {
    let f = Fixture::new();
    let roles: Vec<String> = fs::read_dir(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../workers/src/tl3_workers/roles"
    ))
    .unwrap()
    .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
    .filter_map(|name| name.strip_suffix(".py").map(str::to_owned))
    .filter(|name| name != "base" && !name.starts_with('_'))
    .collect();
    for role in &roles {
        let attempt = f.attempt(
            role,
            json!({"goals": ["Fix greeting"]}),
            json!({"summary": "Fixed greeting"}),
        );
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
        assert_eq!(
            record["activity"]["tests"][0]["action_key"],
            "pb-fixture-action"
        );
        assert_eq!(record["reply"]["summary"], "Fixed greeting");
        assert_eq!(record["outcome"], "succeeded");
        for tag in ["repo:RobTand/tl3-sandbox", "request:3"] {
            assert!(item["tags"].as_array().unwrap().contains(&json!(tag)));
        }
        assert!(
            item["tags"]
                .as_array()
                .unwrap()
                .contains(&json!(format!("role:{}", record["role"].as_str().unwrap())))
        );
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
    if let Ok(settings) = std::env::var("TL3_MEMORY_CRASH_SETTINGS") {
        use std::io::{Read, Write};
        let settings = serde_json::from_str(&settings).unwrap();
        let attempt = std::env::var("TL3_MEMORY_CRASH_ATTEMPT").unwrap();
        if std::env::var("TL3_MEMORY_CRASH_CUT").unwrap() == "after" {
            memory::queue_attempt(
                &mut Store::open(&settings).unwrap(),
                "run:maker",
                std::path::Path::new(&attempt),
            )
            .unwrap();
        }
        println!("MEMORY_CUT");
        std::io::stdout().flush().unwrap();
        std::io::stdin().read_exact(&mut [0]).unwrap();
        return;
    }
    let mut f = Fixture::new();
    let attempt = f.attempt(
        "maker",
        json!({"goals": ["Fix greeting"]}),
        json!({"kind": "done"}),
    );
    for cut in ["before", "after"] {
        use std::io::{BufRead, BufReader, Read};
        use std::process::{Command, Stdio};
        let settings = &f.settings;
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "crash_between_run_and_retain_preserves_record_and_identity", "--nocapture"])
            .env("TL3_MEMORY_CRASH_SETTINGS", json!({"host": settings.host, "port": settings.port, "database": settings.database, "user": settings.user, "password_file": settings.password_file, "connect_timeout_secs": settings.connect_timeout_secs}).to_string())
            .env("TL3_MEMORY_CRASH_ATTEMPT", &attempt)
            .env("TL3_MEMORY_CRASH_CUT", cut)
            .stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
        let mut reader = BufReader::new(child.stdout.take().unwrap());
        let mut ready = false;
        for line in reader.by_ref().lines() {
            if line.unwrap().contains("MEMORY_CUT") {
                ready = true;
                break;
            }
        }
        assert!(ready, "child did not reach the memory save point");
        child.kill().unwrap();
        assert!(!child.wait().unwrap().success());
        memory::queue_attempt(&mut f.store(), "run:maker", &attempt).unwrap();
    }
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
    for secret in [
        "fixture-password",
        "fixture-api-key",
        "fixture-bearer",
        "fixture-url-password",
        "fixture-token",
        "fixture-private-key",
        "/never/read/credentials",
        "model.invalid",
    ] {
        assert!(!sent.contains(secret), "leaked {secret}");
    }
    assert!(sent.contains("Fix greeting"));
}

#[test]
fn json_and_quoted_credentials_in_prose_are_not_sent() {
    let f = Fixture::new();
    let attempt = f.attempt(
        "audit",
        json!({"body": r#"Found {"api_key":"fixture-json-credential"} in a snippet."#}),
        json!({"summary": "Found 'password': 'fixture-quoted-credential' in a snippet."}),
    );
    memory::queue_attempt(&mut f.store(), "run:quoted-credentials", &attempt).unwrap();
    assert_eq!(f.flush(), 1);
    let sent = serde_json::to_string(&*f.retain.documents.lock().unwrap()).unwrap();
    for secret in ["fixture-json-credential", "fixture-quoted-credential"] {
        assert!(!sent.contains(secret), "leaked quoted credential: {secret}");
    }
}

#[test]
fn failed_run_without_summary_is_retained_and_size_is_bounded() {
    let f = Fixture::new();
    let attempt = f.attempt(
        "reviewer",
        json!({"goals": ["é".repeat(200_000)]}),
        json!({"summary": "x".repeat(200_000)}),
    );
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

#[test]
fn authenticated_retain_loads_a_runtime_key_without_recording_it() {
    let f = Fixture::new();
    let key = f.scratch.path().join("fixture-hindsight-key");
    fs::write(&key, "fixture-hindsight-authentication\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&key, fs::Permissions::from_mode(0o600)).unwrap();
    }
    let text = include_str!("../../config/example.toml")
        .replace(
            "[hindsight]",
            &format!("[hindsight]\napi_key_file = {key:?}"),
        )
        .replace("http://127.0.0.1:8890", &f.server.uri());
    let config = tl3_config::Config::parse(&text)
        .expect("Hindsight supports a separate runtime credential file");
    let attempt = f.attempt(
        "audit",
        json!({"goals": ["Fix greeting"]}),
        json!({"verdict": "agree"}),
    );
    memory::queue_attempt(&mut f.store(), "run:authenticated-audit", &attempt).unwrap();
    assert_eq!(memory::flush(&mut f.store(), &config.hindsight).unwrap(), 1);
    let requests = f._runtime.block_on(f.server.received_requests()).unwrap();
    assert_eq!(
        requests[0].headers.get("authorization").unwrap(),
        "Bearer fixture-hindsight-authentication"
    );
    assert!(
        !String::from_utf8_lossy(&requests[0].body).contains("fixture-hindsight-authentication")
    );
}
