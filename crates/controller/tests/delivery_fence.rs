use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Condvar, Mutex, mpsc},
    time::Duration,
};
use tl3_controller::{
    Repository, adapters,
    executor::{Delivery, Executor, Output, Reconciliation},
};
use tl3_github::{AppConfig, ClientOptions, GitHub, message::Policy};
use tl3_kernel::{EffectKind, NodeId};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

#[derive(Default)]
struct ReadGate {
    released: Mutex<bool>,
    ready: Condvar,
}

impl ReadGate {
    fn release(&self) {
        *self.released.lock().unwrap() = true;
        self.ready.notify_all();
    }

    fn wait(&self) {
        let (released, _) = self
            .ready
            .wait_timeout_while(
                self.released.lock().unwrap(),
                Duration::from_secs(20),
                |released| !*released,
            )
            .unwrap();
        assert!(*released, "the scheduled preflight was never released");
    }
}

struct ReleaseOnDrop(Arc<ReadGate>);

impl Drop for ReleaseOnDrop {
    fn drop(&mut self) {
        self.0.release();
    }
}

#[derive(Clone)]
struct Api {
    fence: PathBuf,
    posts: Arc<Mutex<Vec<(String, i64)>>>,
    checks: Arc<Mutex<Vec<Value>>>,
    preflight: Option<(mpsc::SyncSender<()>, Arc<ReadGate>)>,
}

impl Respond for Api {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        if request.method == "POST" && request.url.path() == "/app/installations/3/access_tokens" {
            return ResponseTemplate::new(201).set_body_json(json!({
                "token":"fixture-token", "expires_at":"2099-01-01T00:00:00Z"
            }));
        }
        if request.method == "GET" && request.url.path().ends_with("/check-runs") {
            let checks = self.checks.lock().unwrap().clone();
            if let Some((entered, gate)) = &self.preflight {
                entered.send(()).unwrap();
                gate.wait();
            }
            return ResponseTemplate::new(200).set_body_json(json!({"check_runs":checks}));
        }
        if request.method == "POST" && request.url.path() == "/repos/owner/sandbox/check-runs" {
            let mut check: Value = serde_json::from_slice(&request.body).unwrap();
            let durable: i64 = serde_json::from_slice(&fs::read(&self.fence).unwrap()).unwrap();
            self.posts
                .lock()
                .unwrap()
                .push((check["external_id"].as_str().unwrap().into(), durable));
            let mut checks = self.checks.lock().unwrap();
            check["id"] = json!(checks.len() + 1);
            check["app"] = json!({"id":123,"slug":"factory"});
            checks.push(check.clone());
            return ResponseTemplate::new(201).set_body_json(check);
        }
        if request.method == "PATCH" {
            let id: u64 = request
                .url
                .path()
                .rsplit('/')
                .next()
                .unwrap()
                .parse()
                .unwrap();
            let patch: Value = serde_json::from_slice(&request.body).unwrap();
            let mut checks = self.checks.lock().unwrap();
            let check = checks.iter_mut().find(|check| check["id"] == id).unwrap();
            for (key, value) in patch.as_object().unwrap() {
                check[key] = value.clone();
            }
            return ResponseTemplate::new(200).set_body_json(check.clone());
        }
        panic!(
            "unexpected fixture route {} {}",
            request.method,
            request.url.path()
        );
    }
}

fn adapter(uri: &str, root: &Path, epoch: i64) -> adapters::github::Adapter {
    let client = GitHub::new(
        AppConfig {
            id: 123,
            slug: "factory".into(),
            pem: include_str!("../../mergify/tests/fixtures/throwaway-key.pem").into(),
            webhook_secret: "fixture".into(),
        },
        3,
        ClientOptions {
            api_url: format!("{uri}/").parse().unwrap(),
            request_timeout: Duration::from_secs(25),
            mutation_interval: Duration::ZERO,
            max_retries: 0,
            ..ClientOptions::default()
        },
    )
    .unwrap();
    adapters::github::Adapter::new(
        BTreeMap::from([(3, client)]),
        adapters::workers::FakeWriter,
        epoch,
        Policy::default(),
        root,
        Some(tl3_config::OperationsIssue {
            owner: "owner".into(),
            name: "operations".into(),
            installation_id: 3,
            number: 81,
        }),
    )
    .unwrap()
}

fn approval(epoch: i64) -> Delivery {
    serde_json::from_value(json!({
        "key":format!("approval-epoch-{epoch}"), "epoch":epoch,
        "output":Output::Effect {
            node:NodeId(0), seq:1, kind:EffectKind::PostApproval, head:None
        },
        "generation":1, "head_sha":"a".repeat(40),
        "repository":Repository {owner:"owner".into(), name:"sandbox".into(), installation:3},
        "issue":12, "pull_request":null, "paths":[], "attempt":1, "round":0
    }))
    .unwrap()
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .max_blocking_threads(2)
        .enable_all()
        .build()
        .unwrap()
}

/// State-table Effects, epoch fence: a stale delivery is refused at entry.
/// Concrete bound: two epochs, one key per epoch, no POST in flight.
#[test]
fn stale_approval_is_refused_when_the_fence_precedes_execute() {
    let runtime = runtime();
    let root = tempfile::tempdir().unwrap();
    let posts = Arc::new(Mutex::new(Vec::new()));
    let server = runtime.block_on(MockServer::start());
    runtime.block_on(
        Mock::given(|_: &Request| true)
            .respond_with(Api {
                fence: root.path().join("epoch.fence"),
                posts: posts.clone(),
                checks: Arc::default(),
                preflight: None,
            })
            .mount(&server),
    );
    let mut current = adapter(&server.uri(), root.path(), 2);
    assert!(matches!(
        current.repair_approval(&approval(2)).unwrap().0,
        Reconciliation::Pending
    ));
    let mut old = adapter(&server.uri(), root.path(), 1);
    assert!(old.execute(&approval(1)).is_err());
    assert!(posts.lock().unwrap().is_empty());
}

/// State-table Effects :744,753,755 and Q27 approval retries: after ownership
/// loss a zombie POST is a late landing. A current-owner reconciliation repairs
/// every matching copy, without applying an old key to a replacement operation.
/// Bound: two adapters, one paused GET, two POSTs and one final repair.
#[test]
fn zombie_approval_after_fence_advance_converges_by_original_key() {
    let runtime = runtime();
    let root = tempfile::tempdir().unwrap();
    let posts = Arc::new(Mutex::new(Vec::new()));
    let gate = Arc::new(ReadGate::default());
    let _release_on_panic = ReleaseOnDrop(gate.clone());
    let (entered, ready) = mpsc::sync_channel(1);
    // Separate frontends let the successor read while the old GET is paused.
    let old_server = runtime.block_on(MockServer::start());
    let current_server = runtime.block_on(MockServer::start());
    let api = Api {
        fence: root.path().join("epoch.fence"),
        posts: posts.clone(),
        checks: Arc::default(),
        preflight: None,
    };
    runtime.block_on(
        Mock::given(|_: &Request| true)
            .respond_with(Api {
                preflight: Some((entered, gate.clone())),
                ..api.clone()
            })
            .mount(&old_server),
    );
    runtime.block_on(
        Mock::given(|_: &Request| true)
            .respond_with(api.clone())
            .mount(&current_server),
    );
    let mut current = adapter(&current_server.uri(), root.path(), 2);
    let mut retry = approval(1);
    retry.epoch = 2;
    let old_uri = old_server.uri();
    let old_root = root.path().to_owned();
    let worker = std::thread::spawn(move || adapter(&old_uri, &old_root, 1).execute(&approval(1)));
    ready.recv_timeout(Duration::from_secs(20)).unwrap();
    let fence = root.path().join("epoch.fence");
    assert_eq!(fs::read(&fence).unwrap(), b"1");
    assert!(matches!(
        current.repair_approval(&retry).unwrap().0,
        Reconciliation::Pending
    ));
    assert_eq!(fs::read(&fence).unwrap(), b"2");
    // The adapter boundary models an explicitly authorized retry of the same key.
    assert_eq!(
        current.execute(&retry).unwrap().ack,
        tl3_kernel::Ack::Confirmed
    );
    gate.release();
    let result = worker.join().unwrap();
    assert_eq!(result.unwrap().ack, tl3_kernel::Ack::Confirmed);
    assert_eq!(posts.lock().unwrap().len(), 2);
    assert!(posts.lock().unwrap().iter().all(|(_, epoch)| *epoch == 2));
    // Force repair to exercise PATCH rather than merely observing equal POSTs.
    let expected = {
        let mut checks = api.checks.lock().unwrap();
        let expected = checks[0]["output"].clone();
        checks[1]["conclusion"] = json!("failure");
        checks[1]["output"] = json!({"title":"stale", "summary":"stale", "text":"stale"});
        let mut replacement = checks[1].clone();
        replacement["id"] = json!(3);
        replacement["external_id"] = json!(format!("{}:{}", approval(2).key, "a".repeat(40)));
        checks.push(replacement);
        expected
    };
    let (repaired, copies) = current.repair_approval(&retry).unwrap();
    assert_eq!(copies, Some(2));
    let Reconciliation::Found(completion) = repaired else {
        panic!("late approval was not reconciled");
    };
    assert_eq!(completion.ack, tl3_kernel::Ack::Confirmed);
    assert!(completion.observations.is_empty());
    let checks = api.checks.lock().unwrap();
    for check in &checks[..2] {
        assert_eq!(check["status"], "completed");
        assert_eq!(check["conclusion"], "success");
        assert_eq!(check["output"], expected);
        assert_eq!(
            check["external_id"],
            format!("{}:{}", retry.key, "a".repeat(40))
        );
    }
    assert_eq!(
        checks[2]["conclusion"], "failure",
        "repair crossed effect keys"
    );
}
