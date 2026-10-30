#[path = "../../store/tests/support/mod.rs"]
mod support;

use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU16, Ordering},
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
    status: Arc<AtomicU16>,
    require_auth: Arc<AtomicBool>,
    hung: Arc<AtomicBool>,
}
impl Respond for Retain {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        if self.hung.load(Ordering::SeqCst) {
            return ResponseTemplate::new(200).set_delay(std::time::Duration::from_secs(60));
        }
        let status = self.status.load(Ordering::SeqCst);
        if status != 0 {
            return ResponseTemplate::new(status);
        }
        if self.require_auth.load(Ordering::SeqCst)
            && request.headers.get("authorization").map(|v| v.as_bytes())
                != Some(b"Bearer fixture-hindsight-authentication".as_slice())
        {
            return ResponseTemplate::new(401);
        }
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
    f.retain.require_auth.store(true, Ordering::SeqCst);
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

fn connection(f: &Fixture) -> postgres::Client {
    postgres::Config::new()
        .host(&f.settings.host)
        .port(f.settings.port)
        .dbname(&f.settings.database)
        .user(&f.settings.user)
        .password("tl3-disposable-test-password")
        .connect(postgres::NoTls)
        .unwrap()
}

fn rendered_status(f: &Fixture) -> String {
    let mut reader = tl3_store::Reader::open(&f.settings).unwrap();
    tl3_controller::status::render_html(&tl3_controller::status::load(&mut reader).unwrap())
}

fn rejected_key(status: u16) {
    let f = Fixture::new();
    for role in ["audit", "maker"] {
        let attempt = f.attempt(role, json!({}), json!({"ok": true}));
        memory::queue_attempt(&mut f.store(), role, &attempt).unwrap();
    }
    f.retain.status.store(status, Ordering::SeqCst);
    let error = memory::flush(&mut f.store(), &f.hindsight()).unwrap_err();
    assert!(
        error.to_string().contains(&status.to_string()),
        "retain error must keep HTTP status: {error}"
    );
    assert_eq!(
        f._runtime
            .block_on(f.server.received_requests())
            .unwrap()
            .len(),
        1,
        "a rejected key stops the pass"
    );
    memory::flush(&mut f.store(), &f.hindsight()).unwrap_err();
    let status = rendered_status(&f);
    assert_eq!(
        status.matches("Hindsight rejected TL3&#39;s key").count(),
        1,
        "one operations incident"
    );
    assert_eq!(f.store().pending_memories(0).unwrap().len(), 2);
}

#[test]
fn unauthorized_stops_pass_and_raises_one_incident() {
    rejected_key(401);
}
#[test]
fn forbidden_stops_pass_and_raises_one_incident() {
    rejected_key(403);
}

#[test]
fn permanent_4xx_is_parked_with_status_and_counted() {
    let f = Fixture::new();
    let attempt = f.attempt("audit", json!({}), json!({"ok": true}));
    memory::queue_attempt(&mut f.store(), "bad-record", &attempt).unwrap();
    f.retain.status.store(422, Ordering::SeqCst);
    let _ = memory::flush(&mut f.store(), &f.hindsight());
    assert!(
        f.store().pending_memories(0).unwrap().is_empty(),
        "4xx record must leave retry set"
    );
    let row = connection(&f)
        .query_one(
            "SELECT to_jsonb(m)->>'http_status' FROM tl3_memory_outbox m",
            &[],
        )
        .unwrap();
    assert_eq!(row.get::<_, Option<String>>(0).as_deref(), Some("422"));
    assert!(rendered_status(&f).contains("Parked: 1"));
    assert_eq!(f.flush(), 0);
}

#[test]
fn outbox_cap_parks_oldest_without_losing_record() {
    let f = Fixture::new();
    let _store = f.store();
    let mut db = connection(&f);
    db.execute("INSERT INTO tl3_memory_outbox (operation_id, item) SELECT 'seed-' || n, '{}'::jsonb FROM generate_series(1, 10000) n", &[]).unwrap();
    f.store()
        .queue_memory("newest", &json!({"content": "keep me"}))
        .unwrap();
    let mut store = f.store();
    let mut after = 0;
    let mut active = Vec::new();
    loop {
        let batch = store.pending_memories(after).unwrap();
        if batch.is_empty() {
            break;
        }
        for (seq, id, _) in batch {
            after = seq;
            active.push(id);
        }
    }
    assert_eq!(active.len(), 10000, "outbox retry set is capped");
    assert!(
        !active.contains(&"seed-1".into()),
        "oldest record is parked first"
    );
    assert!(active.contains(&"newest".into()));
    assert_eq!(
        db.query_one("SELECT count(*) FROM tl3_memory_outbox", &[])
            .unwrap()
            .get::<_, i64>(0),
        10001,
        "parking preserves records for operations"
    );
    assert!(rendered_status(&f).contains("Pending: 10000"));
    assert!(rendered_status(&f).contains("Parked: 1"));
}

#[test]
fn nul_bytes_are_removed_before_postgres_jsonb() {
    let f = Fixture::new();
    let attempt = f.attempt(
        "audit",
        json!({"goals": ["good\0goal"]}),
        json!({"summary": "good\0reply"}),
    );
    memory::queue_attempt(&mut f.store(), "nul", &attempt)
        .expect("NUL bytes must not prevent memory queueing");
    let rows = f.store().pending_memories(0).unwrap();
    let record: Value = serde_json::from_str(rows[0].2["content"].as_str().unwrap()).unwrap();
    assert_eq!(record["inputs"]["goals"][0], "goodgoal");
    assert_eq!(record["reply"]["summary"], "goodreply");
}

#[test]
fn record_limit_uses_postgres_jsonb_text_size() {
    let f = Fixture::new();
    let values: Vec<_> = (0..32)
        .map(|n| {
            (
                format!("field{n}"),
                json!({"nested": (0..32).collect::<Vec<_>>() }),
            )
        })
        .collect();
    let input = Value::Object(values.into_iter().collect());
    let attempt = f.attempt("audit", input.clone(), input);
    memory::queue_attempt(&mut f.store(), "jsonb-size", &attempt)
        .expect("Postgres stored size must fit its constraint");
    let size: i32 = connection(&f)
        .query_one(
            "SELECT octet_length(item::text) FROM tl3_memory_outbox",
            &[],
        )
        .unwrap()
        .get(0);
    assert!(size <= 32768);
}

#[test]
fn public_action_ids_survive_only_in_structured_test_activity() {
    let f = Fixture::new();
    let public_id = "9d3e8f1c4a6b02d57e90c183ab64f27018d4ce39a6025b78f3d91ac04e67b258";
    let bare_secret = "f9c83a01b647de2390abc81456fed279";
    let attempt = f.attempt(
        "audit",
        json!({"action_key": "fixture-input-secret",
        "service_key": "fixture-service-secret", "body": bare_secret}),
        json!({"ok": true}),
    );
    let path = attempt.join("memory.json");
    let mut summary: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    summary["activity"]["tests"][0]["action_key"] = json!(public_id);
    fs::write(path, summary.to_string()).unwrap();
    memory::queue_attempt(&mut f.store(), "public-action-id", &attempt).unwrap();
    assert_eq!(f.flush(), 1);
    let documents = f.retain.documents.lock().unwrap();
    let record: Value = serde_json::from_str(
        documents.values().next().unwrap()["content"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        record["activity"]["tests"][0]["action_key"], public_id,
        "PrismaBuild action IDs are public identifiers in structured test results"
    );
    for secret in [
        bare_secret,
        "fixture-input-secret",
        "fixture-service-secret",
    ] {
        assert!(
            !record.to_string().contains(secret),
            "input credentials must remain redacted"
        );
    }
}


#[test]
fn review2_key_recovery_clears_incident_and_later_rejection_updates_status() {
    let f = Fixture::new();
    let attempt = f.attempt("audit", json!({}), json!({"ok": true}));
    memory::queue_attempt(&mut f.store(), "first", &attempt).unwrap();
    f.retain.status.store(401, Ordering::SeqCst);
    memory::flush(&mut f.store(), &f.hindsight()).unwrap_err();
    f.retain.status.store(0, Ordering::SeqCst);
    assert_eq!(f.flush(), 1);
    assert!(!rendered_status(&f).contains("Hindsight rejected"), "successful retain clears key incident");
    memory::queue_attempt(&mut f.store(), "later", &attempt).unwrap();
    for status in [401, 403, 403] {
        f.retain.status.store(status, Ordering::SeqCst);
        memory::flush(&mut f.store(), &f.hindsight()).unwrap_err();
        let page = rendered_status(&f);
        assert_eq!(page.matches("Hindsight rejected TL3&#39;s key").count(), 1);
        assert!(page.contains(&format!("HTTP {status}")), "incident reflects latest rejected status");
        assert_eq!(connection(&f).query_one("SELECT count(*) FROM tl3_memory_health", &[]).unwrap().get::<_, i64>(0), 1);
    }
}

#[test]
fn review2_rejection_updates_401_to_403_without_duplicate_incident() {
    let f = Fixture::new();
    let mut store = f.store();
    store.reject_memory_key(401).unwrap();
    store.reject_memory_key(403).unwrap();
    assert!(rendered_status(&f).contains("HTTP 403"), "latest rejection must replace stale HTTP 401");
    assert_eq!(connection(&f).query_one("SELECT count(*) FROM tl3_memory_health", &[]).unwrap().get::<_, i64>(0), 1);
}

#[test]
fn review2_prose_survives_and_only_sha_span_is_redacted() {
    let f = Fixture::new();
    let sha = "a1b2c3d4e5f6789012345678901234567890abcd";
    let attempt = f.attempt("maker", json!({"notes": "task-runner risk-based disk-full Token: counts rose"}),
        json!({"head": sha, "summary": format!("Committed {sha} and fixed the runner.")}));
    memory::queue_attempt(&mut f.store(), "prose", &attempt).unwrap();
    let rows = f.store().pending_memories(0).unwrap();
    let record: Value = serde_json::from_str(rows[0].2["content"].as_str().unwrap()).unwrap();
    assert_eq!(record["inputs"]["notes"], "task-runner risk-based disk-full Token: counts rose");
    assert_eq!(record["reply"]["summary"], "Committed [redacted] and fixed the runner.");
    assert_eq!(record["head_sha"], sha, "known maker head is structured provenance");
}

#[test]
fn review2_capacity_parking_replays_oldest_first_on_recovery_flush() {
    let f = Fixture::new();
    let _store = f.store();
    let mut db = connection(&f);
    db.execute("INSERT INTO tl3_memory_outbox (operation_id, item) SELECT 'seed-' || n, '{}'::jsonb FROM generate_series(1, 10000) n", &[]).unwrap();
    for role in ["audit", "maker"] {
        let attempt = f.attempt(role, json!({}), json!({"ok": true}));
        memory::queue_attempt(&mut f.store(), role, &attempt).unwrap();
    }
    // Model acknowledgements of the older active rows without 10,000 HTTP calls.
    db.execute("DELETE FROM tl3_memory_outbox WHERE operation_id LIKE 'seed-%' AND parked_reason IS NULL", &[]).unwrap();
    f.retain.status.store(503, Ordering::SeqCst);
    memory::flush(&mut f.store(), &f.hindsight()).unwrap_err();
    assert!(rendered_status(&f).contains("Parked: 2"));
    f.retain.status.store(0, Ordering::SeqCst);
    // Parked seeds need UUIDs accepted by the fake retain responder.
    db.execute("UPDATE tl3_memory_outbox SET operation_id = lpad(sequence::text, 36, '0') WHERE parked_reason = 'capacity'", &[]).unwrap();
    assert_eq!(f.flush(), 4, "recovery flush also drains capacity-parked records");
    let requests = f._runtime.block_on(f.server.received_requests()).unwrap();
    let ids: Vec<String> = requests.iter().skip(3).map(|r| r.body_json::<Value>().unwrap()["operation_id"].as_str().unwrap().into()).collect();
    assert_eq!(ids, vec![format!("{:036}", 1), format!("{:036}", 2)], "capacity replay is oldest first");
    assert!(rendered_status(&f).contains("Parked: 0"));
}

#[test]
fn review2_http_parking_expires_after_30_days_even_without_pending_rows() {
    let f = Fixture::new();
    let attempt = f.attempt("audit", json!({}), json!({"ok": true}));
    for key in ["old", "recent"] {
        memory::queue_attempt(&mut f.store(), key, &attempt).unwrap();
    }
    f.retain.status.store(422, Ordering::SeqCst);
    f.flush();
    let mut db = connection(&f);
    // Schema stub for the retention clock, replaced by the production migration.
    db.batch_execute("ALTER TABLE tl3_memory_outbox ADD COLUMN IF NOT EXISTS parked_at timestamptz DEFAULT CURRENT_TIMESTAMP").unwrap();
    db.execute("UPDATE tl3_memory_outbox SET parked_at = CURRENT_TIMESTAMP - INTERVAL '31 days' WHERE sequence = (SELECT min(sequence) FROM tl3_memory_outbox)", &[]).unwrap();
    assert_eq!(f.flush(), 0);
    assert_eq!(db.query_one("SELECT count(*) FROM tl3_memory_outbox WHERE parked_reason = 'http'", &[]).unwrap().get::<_, i64>(0), 1, "4xx parking expires after 30 days at flush time");
    assert!(rendered_status(&f).contains("Parked: 1"));
}

fn retryable_status(status: u16) {
    let f = Fixture::new();
    let attempt = f.attempt("audit", json!({}), json!({"ok": true}));
    memory::queue_attempt(&mut f.store(), "retryable", &attempt).unwrap();
    f.retain.status.store(status, Ordering::SeqCst);
    assert!(memory::flush(&mut f.store(), &f.hindsight()).is_err(), "HTTP {status} remains retryable");
    assert_eq!(f.store().pending_memories(0).unwrap().len(), 1);
    assert!(rendered_status(&f).contains("Parked: 0"));
    f.retain.status.store(0, Ordering::SeqCst);
    assert_eq!(f.flush(), 1);
}
#[test]
fn review2_http_408_remains_pending() { retryable_status(408); }
#[test]
fn review2_http_429_remains_pending() { retryable_status(429); }

#[test]
fn review2_hung_endpoint_costs_one_timeout_per_pass_and_resumes() {
    let f = Fixture::new();
    for role in ["audit", "maker", "reviewer"] {
        let attempt = f.attempt(role, json!({}), json!({"ok": true}));
        memory::queue_attempt(&mut f.store(), role, &attempt).unwrap();
    }
    f.retain.hung.store(true, Ordering::SeqCst);
    assert!(memory::flush(&mut f.store(), &f.hindsight()).is_err());
    assert_eq!(f._runtime.block_on(f.server.received_requests()).unwrap().len(), 1, "transport error ends pass after one timeout");
    assert_eq!(f.store().pending_memories(0).unwrap().len(), 3);
    f.retain.hung.store(false, Ordering::SeqCst);
    assert_eq!(f.flush(), 3, "next wake retries untouched rows");
}
