use tl3_controller::adapters::{delivery_router::Router, direct_merge};

#[derive(Default)]
struct DirectState {
    puts: Vec<Value>,
    successes: usize,
    status: Option<u16>,
    lost_reply: bool,
    moved: bool,
    already_merged: bool,
    reads: usize,
    reject_as_merged: bool,
    root: std::path::PathBuf,
}
#[derive(Clone)]
struct DirectApi { base: Api, direct: Arc<Mutex<DirectState>> }
impl Respond for DirectApi {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let path = request.url.path();
        let method = request.method.as_str();
        if method == "GET" && path == "/repos/owner/sandbox" {
            return ResponseTemplate::new(200).set_body_json(json!({"allow_merge_commit":true,"allow_squash_merge":true,"allow_rebase_merge":true}));
        }
        if method == "GET" && path == "/repos/owner/sandbox/pulls/100" {
            let mut direct = self.direct.lock().unwrap();
            direct.reads += 1;
            let mut state = self.base.0.lock().unwrap();
            if direct.moved { state.pull["head"]["sha"] = json!("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"); }
            if direct.already_merged { state.merged(false); }
            state.pull_reads += 1;
            return ResponseTemplate::new(200).set_body_json(state.pull.clone());
        }
        if method == "PUT" && path == "/repos/owner/sandbox/pulls/100/merge" {
            let body: Value = serde_json::from_slice(&request.body).unwrap();
            let mut direct = self.direct.lock().unwrap();
            assert!(direct.root.join("epoch.fence").exists(), "fence must precede PUT");
            let intents: Vec<_> = std::fs::read_dir(&direct.root).unwrap().map(|entry| entry.unwrap().path()).filter(|path| path.extension().is_some_and(|ext| ext == "intent")).collect();
            assert_eq!(intents.len(), 1, "durable intent must precede PUT");
            let intent: Value = serde_json::from_slice(&std::fs::read(&intents[0]).unwrap()).unwrap();
            assert_eq!(intent["head"], body["sha"]);
            assert_eq!(intent["action"], "Merge");
            assert_eq!(body["merge_method"], "squash");
            direct.puts.push(body.clone());
            let mut state = self.base.0.lock().unwrap();
            if state.pull["merged_at"].is_string() { return ResponseTemplate::new(405); }
            if body["sha"] != state.pull["head"]["sha"] { return ResponseTemplate::new(409); }
            if direct.reject_as_merged { state.merged(false); return ResponseTemplate::new(405); }
            if let Some(status) = direct.status { return ResponseTemplate::new(status); }
            state.merged(false);
            direct.successes += 1;
            return if direct.lost_reply { ResponseTemplate::new(503) }
                else { ResponseTemplate::new(200).set_body_json(json!({"merged":true,"sha":body["sha"],"message":"merged"})) };
        }
        self.base.respond(request)
    }
}
struct DirectFixture { api: Server, root: tempfile::TempDir, direct: Arc<Mutex<DirectState>>, wakes: direct_merge::Wakes }
fn direct_repo() -> tl3_config::Repository {
    serde_json::from_value(json!({"owner":"owner", "name":"sandbox", "installation_id":3, "delivery":"direct"})).unwrap()
}
impl DirectFixture {
    fn new() -> Self {
        let api = Server::new();
        let root = tempfile::tempdir().unwrap();
        let direct = Arc::new(Mutex::new(DirectState { root: root.path().join("direct"), ..Default::default() }));
        api._runtime.block_on(async {
            Mock::given(|_: &Request| true).respond_with(DirectApi { base: Api(api.state.clone()), direct: direct.clone() }).with_priority(1).mount(&api.server).await;
        });
        api.state.lock().unwrap().pull["mergeable"] = json!(true);
        Self { api, root, direct, wakes: direct_merge::Wakes::default() }
    }
    fn adapter(&self, settings: &tl3_config::Store) -> direct_merge::Adapter {
        direct_merge::Adapter::new(self.api.clients(), 0, &self.root.path().join("direct"), settings.clone()).unwrap().with_wakes(self.wakes.clone())
    }
    fn open(&self, settings: &tl3_config::Store) -> Controller {
        let mut registry = adapters::simulation(self.root.path()).unwrap();
        registry.register(Upstream::Mergify, Router::new(self.api.adapter(&self.root.path().join("mergify"), 0).with_store(settings.clone()), self.adapter(settings), &[direct_repo()]));
        let mut controller = Controller::open(Store::open(settings).unwrap(), registry, bootstrap()).unwrap();
        controller.reconcile_repository_delivery(&[direct_repo()]).unwrap();
        controller.register_github_translator("mergify", direct_merge::Translator::new(self.api.clients()).unwrap().with_wakes(self.wakes.clone()));
        controller
    }
    fn start(&self, settings: &tl3_config::Store) -> (Controller, Delivery) {
        let mut controller = self.open(settings);
        controller.intake(LabelledIssue { delivery:"direct-label".into(), owner:"owner".into(), repo:"sandbox".into(), installation:3, number:12, title:"change".into(), actor:42, label:"tl3".into(), labels:vec![] }).unwrap();
        let mut enqueue = None;
        for _ in 0..100 {
            let progressed = controller.drive_once_with(|boundary, key| {
                if boundary != Boundary::BeforeDispatch { return; }
                let effect = Store::open(settings).unwrap().effect(key).unwrap().unwrap();
                let mut d: Delivery = serde_json::from_value(effect.payload).unwrap();
                d.pull_request = Some(100);
                if !matches!(d.output, Out::Effect { kind: EffectKind::Enqueue, .. }) { return; }
                let mut state = self.api.state.lock().unwrap();
                state.pull["head"]["sha"] = json!(d.head_sha);
                state.head_checks.push(json!({"id":1001,"name":"TL3 approval","head_sha":d.head_sha,"status":"completed","conclusion":"success","external_id":format!("approval:{}",d.head_sha.as_deref().unwrap()),"app":{"id":123,"slug":"factory"}}));
                for check in &mut state.head_checks { if check["name"] != "TL3 approval" { check["head_sha"] = json!(d.head_sha); } }
                enqueue = Some(d);
            }).unwrap();
            if enqueue.is_some() || !progressed { break; }
        }
        (controller, enqueue.expect("reached Enqueue"))
    }
    fn check(&self, conclusion: Option<&str>) {
        let mut state = self.api.state.lock().unwrap();
        state.required = true;
        state.head_checks.retain(|check| check["name"] != "Sandbox tests");
        let sha = state.pull["head"]["sha"].clone();
        state.head_checks.push(json!({"id":1100,"name":"Sandbox tests","status":if conclusion.is_some(){"completed"}else{"in_progress"},"conclusion":conclusion,"head_sha":sha,"app":{"id":15368,"slug":"checks"}}));
    }
    fn check_event(&self, settings: &tl3_config::Store) {
        let state = self.api.state.lock().unwrap();
        let check = state.head_checks.iter().find(|check| check["name"] == "Sandbox tests").unwrap().clone();
        drop(state);
        let mut body = webhook_fixture();
        body["action"] = json!("completed");
        body["repository"]["owner"]["login"] = json!("owner");
        body["repository"]["name"] = json!("sandbox");
        body["installation"] = json!({"id":3});
        body["check_run"] = check;
        deliver(settings, &self.api, "check_run", &body);
    }
}

#[test]
fn reviewed_checked_direct_head_does_not_enqueue_with_mergify() {
    let mut database = support::Database::start();
    let settings = database.create("direct_pinned");
    let fixture = DirectFixture::new();
    let (mut controller, delivery) = fixture.start(&settings);
    drain(&mut controller);
    let direct = fixture.direct.lock().unwrap();
    assert_eq!(direct.puts.len(), 1);
    assert_eq!(direct.puts[0]["sha"], json!(delivery.head_sha));
    assert_eq!(direct.successes, 1);
    let state = fixture.api.state.lock().unwrap();
    assert_eq!(state.posts, 0, "#830: a direct-mode repository must never enqueue with Mergify");
    assert!(state.pull["body"].as_str().unwrap().contains(direct_merge::VISIBILITY));
    assert!(matches!(controller.world().unwrap().nodes[0].phase, Phase::Closed(Outcome::Merged)));
}

#[test]
fn moved_head_refuses_and_returns_to_checking() {
    let mut database = support::Database::start();
    let settings = database.create("direct_moved");
    let fixture = DirectFixture::new();
    fixture.direct.lock().unwrap().moved = true;
    let (mut controller, _) = fixture.start(&settings);
    for _ in 0..10 {
        let n = controller.world().unwrap().nodes[0];
        if matches!(n.phase, Phase::Checking | Phase::Ready(tl3_kernel::Resume::Checking)) {
            assert!(n.rechecks > 0);
            assert!(fixture.direct.lock().unwrap().puts.is_empty());
            return;
        }
        controller.drive_once().unwrap();
    }
    panic!("outside push did not return to Checking");
}

#[test]
fn failed_protection_check_uses_the_existing_ci_failure_path() {
    let mut database = support::Database::start();
    let settings = database.create("direct_failed");
    let fixture = DirectFixture::new();
    fixture.check(Some("failure"));
    let (mut controller, _) = fixture.start(&settings);
    for _ in 0..10 {
        if controller.world().unwrap().nodes[0].round == 1 {
            assert!(fixture.direct.lock().unwrap().puts.is_empty());
            return;
        }
        controller.drive_once().unwrap();
    }
    panic!("required-check failure did not start the existing failed round");
}

#[test]
fn pending_check_waits_for_webhook_without_polling() {
    let mut database = support::Database::start();
    let settings = database.create("direct_pending");
    let fixture = DirectFixture::new();
    fixture.check(None);
    let (mut controller, _) = fixture.start(&settings);
    drain(&mut controller);
    let reads = fixture.direct.lock().unwrap().reads;
    for _ in 0..5 { assert!(!controller.drive_once().unwrap()); }
    assert_eq!(fixture.direct.lock().unwrap().reads, reads);
    assert!(fixture.direct.lock().unwrap().puts.is_empty());
    assert_eq!(controller.world().unwrap().nodes[0].fx_failures, 0);
    fixture.check(Some("success"));
    fixture.check_event(&settings);
    drain(&mut controller);
    assert_eq!(fixture.direct.lock().unwrap().puts.len(), 1);
    assert!(matches!(controller.world().unwrap().nodes[0].phase, Phase::Closed(Outcome::Merged)));
}

#[test]
fn already_merged_is_a_noop() {
    let mut database = support::Database::start();
    let settings = database.create("direct_already");
    let fixture = DirectFixture::new();
    fixture.direct.lock().unwrap().already_merged = true;
    let (mut controller, _) = fixture.start(&settings);
    drain(&mut controller);
    assert!(fixture.direct.lock().unwrap().puts.is_empty());
    assert!(matches!(controller.world().unwrap().nodes[0].phase, Phase::Closed(Outcome::Merged)));
}

#[test]
fn lost_reply_reconciles_and_never_merges_again() {
    let mut database = support::Database::start();
    let settings = database.create("direct_lost");
    let fixture = DirectFixture::new();
    fixture.direct.lock().unwrap().lost_reply = true;
    let (mut controller, delivery) = fixture.start(&settings);
    drain(&mut controller);
    let mut restarted = fixture.adapter(&settings);
    assert!(matches!(restarted.reconcile(&delivery).unwrap(), Reconciliation::Found(_)));
    assert_eq!(fixture.direct.lock().unwrap().puts.len(), 1);
    assert_eq!(fixture.direct.lock().unwrap().successes, 1);
}

#[test]
fn merge_permission_refusal_becomes_an_incident() {
    let mut database = support::Database::start();
    let settings = database.create("direct_permission");
    let fixture = DirectFixture::new();
    fixture.direct.lock().unwrap().status = Some(403);
    let (mut controller, _) = fixture.start(&settings);
    drain(&mut controller);
    assert_eq!(fixture.direct.lock().unwrap().puts.len(), 1);
    assert!(controller.snapshot().unwrap().rows.iter().any(|row| row.table == "incidents" && row.value["open"] == true));
}

#[test]
fn queue_repository_keeps_mergify() {
    let mut database = support::Database::start();
    let settings = database.create("direct_queue_control");
    let fixture = DirectFixture::new();
    let mut repo = direct_repo();
    repo.delivery = tl3_config::Delivery::Queue;
    let mut registry = adapters::simulation(fixture.root.path()).unwrap();
    registry.register(Upstream::Mergify, Router::new(fixture.api.adapter(&fixture.root.path().join("mergify"), 0).with_store(settings.clone()), fixture.adapter(&settings), &[repo]));
    let mut controller = Controller::open(Store::open(&settings).unwrap(), registry, bootstrap()).unwrap();
    controller.intake(LabelledIssue { delivery:"queue-control".into(), owner:"owner".into(), repo:"sandbox".into(), installation:3, number:12, title:"change".into(), actor:42, label:"tl3".into(), labels:vec![] }).unwrap();
    for _ in 0..100 {
        if !controller.drive_once_with(|boundary, key| {
            if boundary == Boundary::BeforeDispatch {
                let effect = Store::open(&settings).unwrap().effect(key).unwrap().unwrap();
                let d: Delivery = serde_json::from_value(effect.payload).unwrap();
                if matches!(d.output, Out::Effect {kind: EffectKind::Enqueue, ..}) { fixture.api.state.lock().unwrap().pull["head"]["sha"] = json!(d.head_sha); }
            }
        }).unwrap() { break; }
    }
    assert_eq!(fixture.api.state.lock().unwrap().posts, 1);
    assert!(fixture.direct.lock().unwrap().puts.is_empty());
    assert_eq!(controller.world().unwrap().nodes[0].phase, Phase::Queued);
}


#[test]
fn already_merged_405_reconciles_without_failure() {
    let mut database = support::Database::start();
    let settings = database.create("direct_405");
    let fixture = DirectFixture::new();
    fixture.direct.lock().unwrap().reject_as_merged = true;
    let (mut controller, _) = fixture.start(&settings);
    drain(&mut controller);
    assert_eq!(fixture.direct.lock().unwrap().puts.len(), 1);
    assert!(matches!(controller.world().unwrap().nodes[0].phase, Phase::Closed(Outcome::Merged)));
    assert!(!controller.snapshot().unwrap().rows.iter().any(|r| r.table == "incidents"));
}

#[test]
fn restart_at_every_direct_write_converges() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    // Fence, intent, annotation, merge and receipt, before and after each. The
    // real Records writer performs fsync/rename; the callback never replaces it.
    let mut database = support::Database::start();
    let mut boundaries = Vec::new();
    for cut in 0..24 {
        let settings = database.create(&format!("direct_crash_{cut}"));
        let fixture = DirectFixture::new();
        fixture.check(None);
        let (controller, delivery) = fixture.start(&settings);
        drop(controller);
        fixture.check(Some("success"));
        let seen = Arc::new(AtomicUsize::new(0));
        let count = seen.clone();
        let trace = Arc::new(Mutex::new(Vec::new()));
        let recorded = trace.clone();
        // Fresh process wake state, but the same durable records and world.
        let mut issuer = direct_merge::Adapter::new(fixture.api.clients(), 0,
            &fixture.root.path().join("direct"), settings.clone()).unwrap()
            .with_write_boundary(move |boundary| {
                recorded.lock().unwrap().push(boundary);
                if count.fetch_add(1, Ordering::SeqCst) == cut { panic!("#830 injected write crash"); }
            });
        let crashed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| issuer.execute(&delivery)));
        let count = seen.load(Ordering::SeqCst);
        if crashed.is_ok() {
            assert_eq!(cut, count, "all preceding boundaries must have crashed");
            assert_eq!(cut, boundaries.len());
            break;
        }
        boundaries.push(*trace.lock().unwrap().last().unwrap());
        drop(issuer);
        let mut restarted = fixture.open(&settings);
        // A restart has no in-memory sleeping effects.
        let mut adapter = direct_merge::Adapter::new(fixture.api.clients(), 0,
            &fixture.root.path().join("direct"), settings.clone()).unwrap();
        let completion = match adapter.reconcile(&delivery).unwrap() {
            Reconciliation::Found(c) => c,
            _ => panic!("fresh reconciliation must converge at cut {cut}"),
        };
        assert_eq!(completion.ack, tl3_kernel::Ack::Confirmed);
        drop(adapter);
        fixture.check_event(&settings);
        drain(&mut restarted);
        assert!(matches!(restarted.world().unwrap().nodes[0].phase, Phase::Closed(Outcome::Merged)));
        assert_eq!(fixture.direct.lock().unwrap().successes, 1);
    }
    for write in [direct_merge::Write::Fence, direct_merge::Write::Intent,
        direct_merge::Write::Annotation, direct_merge::Write::Merge, direct_merge::Write::Receipt] {
        for after in [false, true] {
            assert!(boundaries.contains(&direct_merge::WriteBoundary { write, after }), "unreached {write:?}/{after}");
        }
    }
}

#[test]
fn racing_pinned_puts_have_one_success_and_both_converge() {
    let mut database = support::Database::start();
    let settings = database.create("direct_race");
    let fixture = DirectFixture::new();
    fixture.check(None);
    let (controller, delivery) = fixture.start(&settings);
    drop(controller);
    fixture.check(Some("success"));
    // Both independently reconcile the open, approved head before either PUT.
    // Separate record roots simulate two controller processes with private state.
    let barrier = Arc::new(std::sync::Barrier::new(2));
    let mut issuers = Vec::new();
    for index in 0..2 {
        let path = fixture.root.path().join(format!("issuer-{index}"));
        let gate = barrier.clone();
        let adapter = direct_merge::Adapter::new(fixture.api.clients(), 0, &path, settings.clone()).unwrap()
            .with_write_boundary(move |boundary| {
                if boundary == (direct_merge::WriteBoundary { write: direct_merge::Write::Merge, after: false }) { gate.wait(); }
            });
        issuers.push(adapter);
    }
    // The mock validates each issuer's fence/intent directory at its request.
    // Use an existing durable intent copy for the common server assertion.
    let mut warmup = direct_merge::Adapter::new(fixture.api.clients(), 0,
        &fixture.root.path().join("direct"), settings.clone()).unwrap().with_write_boundary(|boundary| {
            if boundary == (direct_merge::WriteBoundary { write: direct_merge::Write::Merge, after: false }) { panic!("before PUT"); }
        });
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| warmup.execute(&delivery)));
    let handles: Vec<_> = issuers.into_iter().map(|mut adapter| {
        let delivery = delivery.clone();
        std::thread::spawn(move || adapter.execute(&delivery))
    }).collect();
    for handle in handles { assert_eq!(handle.join().unwrap().unwrap().ack, tl3_kernel::Ack::Confirmed); }
    let state = fixture.direct.lock().unwrap();
    assert_eq!(state.puts.len(), 2);
    assert_eq!(state.successes, 1);
    assert!(state.puts.iter().all(|put| put["sha"] == json!(delivery.head_sha)));
}

#[test]
fn repository_delivery_is_reconciled_in_stored_world() {
    let mut database = support::Database::start();
    let settings = database.create("direct_config");
    let fixture = DirectFixture::new();
    let mut controller = fixture.open(&settings);
    let direct = controller.snapshot().unwrap().rows.into_iter().find(|r| r.table == "repos").unwrap();
    assert_eq!(direct.value["delivery"], "direct");
    let mut repo = direct_repo(); repo.delivery = tl3_config::Delivery::Queue;
    controller.reconcile_repository_delivery(&[repo]).unwrap();
    let queue = controller.snapshot().unwrap().rows.into_iter().find(|r| r.table == "repos").unwrap();
    assert!(queue.value.get("delivery").is_none());
    let revision = controller.snapshot().unwrap().revision;
    let mut repo = direct_repo(); repo.delivery = tl3_config::Delivery::Queue;
    controller.reconcile_repository_delivery(&[repo]).unwrap();
    assert_eq!(controller.snapshot().unwrap().revision, revision);
    drop(controller);
    let controller = fixture.open(&settings);
    assert_eq!(controller.snapshot().unwrap().rows.into_iter().find(|r| r.table == "repos").unwrap().value, direct.value);
}
