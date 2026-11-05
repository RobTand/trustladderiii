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
            if let Some((entered, gate)) = &self.preflight {
                entered.send(()).unwrap();
                gate.wait();
            }
            return ResponseTemplate::new(200).set_body_json(json!({"check_runs":[]}));
        }
        if request.method == "POST" && request.url.path() == "/repos/owner/sandbox/check-runs" {
            let mut check: Value = serde_json::from_slice(&request.body).unwrap();
            let durable: i64 = serde_json::from_slice(&fs::read(&self.fence).unwrap()).unwrap();
            self.posts
                .lock()
                .unwrap()
                .push((check["external_id"].as_str().unwrap().into(), durable));
            check["id"] = json!(1);
            check["app"] = json!({"id":123,"slug":"factory"});
            return ResponseTemplate::new(201).set_body_json(check);
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

/// State-table Effects, epoch fence: an old executor must not issue HTTP after
/// the durable fence advances. Concrete bound: two adapters, one paused GET,
/// one successor repair and one possible POST. Channels impose the ordering;
/// deadlines only abort a broken test schedule.
#[test]
fn fence_advance_during_preflight_refuses_the_old_approval_post() {
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
            .respond_with(api)
            .mount(&current_server),
    );
    let mut current = adapter(&current_server.uri(), root.path(), 2);
    let old_uri = old_server.uri();
    let old_root = root.path().to_owned();
    let worker = std::thread::spawn(move || adapter(&old_uri, &old_root, 1).execute(&approval(1)));
    ready.recv_timeout(Duration::from_secs(20)).unwrap();
    let fence = root.path().join("epoch.fence");
    assert_eq!(fs::read(&fence).unwrap(), b"1");
    assert!(matches!(
        current.repair_approval(&approval(2)).unwrap().0,
        Reconciliation::Pending
    ));
    assert_eq!(fs::read(&fence).unwrap(), b"2");
    gate.release();
    let result = worker.join().unwrap();
    let observed = posts.lock().unwrap();
    assert!(
        observed.is_empty(),
        "old epoch 1 issued POST after durable fence 2: {observed:?}"
    );
    assert!(result.is_err(), "the old executor must report fencing");
}
