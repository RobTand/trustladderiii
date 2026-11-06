use tl3_controller::adapters::{delivery_router::Router, direct_merge};

#[derive(Default)]
struct DirectState {
    puts: Vec<Value>,
    successes: usize,
    status: Option<u16>,
    lost_reply: bool,
    fail_read_after_put: bool,
    moved: bool,
    already_merged: bool,
    reads: usize,
    reject_as_merged: bool,
    root: std::path::PathBuf,
    intent_roots: Vec<std::path::PathBuf>,
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
            if direct.fail_read_after_put && !direct.puts.is_empty() { return ResponseTemplate::new(503); }
            let mut state = self.base.0.lock().unwrap();
            if direct.moved { state.pull["head"]["sha"] = json!("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"); }
            if direct.already_merged { state.merged(false); }
            state.pull_reads += 1;
            return ResponseTemplate::new(200).set_body_json(state.pull.clone());
        }
        if method == "GET" && path.ends_with("/annotations") {
            return ResponseTemplate::new(200).set_body_json(self.base.0.lock().unwrap().annotations.clone());
        }
        if method == "PUT" && path == "/repos/owner/sandbox/pulls/100/merge" {
            let body: Value = serde_json::from_slice(&request.body).unwrap();
            let mut direct = self.direct.lock().unwrap();
            let roots = if direct.intent_roots.is_empty() { vec![direct.root.clone()] } else {direct.intent_roots.clone()};
            for root in roots {
                assert!(root.join("epoch.fence").exists(), "fence must precede PUT");
                let intents: Vec<_> = std::fs::read_dir(&root).unwrap().map(|e| e.unwrap().path())
                    .filter(|p| p.extension().is_some_and(|e| e == "intent")).collect();
                assert_eq!(intents.len(), 1, "durable intent must precede PUT");
                let intent: Value = serde_json::from_slice(&std::fs::read(&intents[0]).unwrap()).unwrap();
                assert_eq!(intent["head"], body["sha"]);
                assert_eq!(intent["action"], "Merge");
            }
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
struct DirectFixture { api: Server, root: tempfile::TempDir, direct: Arc<Mutex<DirectState>>, wakes: direct_merge::Wakes, clock: head_checks::Clock }
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
        Self { api, root, direct, wakes: direct_merge::Wakes::default(), clock: head_checks::Clock(std::rc::Rc::new(std::cell::Cell::new(std::time::SystemTime::now()))) }
    }
    fn adapter(&self, settings: &tl3_config::Store) -> direct_merge::Adapter {
        direct_merge::Adapter::new(self.api.clients(), 0, &self.root.path().join("direct"), settings.clone()).unwrap().with_wakes(self.wakes.clone())
    }
    fn open(&self, settings: &tl3_config::Store) -> Controller {
        self.open_adapter(settings, self.adapter(settings))
    }
    fn open_adapter(&self, settings: &tl3_config::Store, adapter: direct_merge::Adapter) -> Controller {
        let mut registry = adapters::simulation(self.root.path()).unwrap();
        registry.register(Upstream::Delivery, Router::new(self.api.adapter(&self.root.path().join("mergify"), 0).with_store(settings.clone()), adapter, &[direct_repo()]));
        let mut controller = Controller::open_with_clock(Store::open(settings).unwrap(), registry, bootstrap(), self.clock.clone()).unwrap();
        let mut allowances = tl3_config::Allowances::default();
        allowances.github_mergify.silence_secs = 1;
        controller.set_allowances(allowances);
        controller.reconcile_repository_delivery(&[direct_repo()]).unwrap();
        controller.register_github_translator("mergify", tl3_controller::adapters::delivery_router::Translator::new(self.api.clients()).unwrap().with_wakes(self.wakes.clone()));
        controller
    }
    fn start(&self, settings: &tl3_config::Store) -> (Controller, Delivery) {
        self.start_controller(settings, self.open(settings))
    }
    fn start_controller(&self, settings: &tl3_config::Store, mut controller: Controller) -> (Controller, Delivery) {
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
    registry.register(Upstream::Delivery, Router::new(fixture.api.adapter(&fixture.root.path().join("mergify"), 0).with_store(settings.clone()), fixture.adapter(&settings), &[repo]));
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
fn restart_at_direct_write_cuts_0_to_7_converges() { restart_direct_write_cuts(0..8); }
#[test]
fn restart_at_direct_write_cuts_8_to_15_converges() { restart_direct_write_cuts(8..16); }
#[test]
fn restart_at_direct_write_cuts_16_to_19_converges() { restart_direct_write_cuts(16..20); }
fn restart_direct_write_cuts(cuts: std::ops::Range<usize>) {
    use std::sync::atomic::{AtomicUsize, Ordering};
    // Fence, intent, annotation, merge and receipt, before and after each. The
    // real Records writer performs fsync/rename; the callback never replaces it.
    let mut database = support::Database::start();
    for cut in cuts {
        let settings = database.create(&format!("direct_crash_{cut}"));
        let mut fixture = DirectFixture::new();
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
        assert!(crashed.is_err(), "mandatory boundary {cut} did not crash");
        assert_eq!(count, cut + 1);
        assert_eq!(trace.lock().unwrap().len(), cut + 1);
        if cut == 19 {
            for write in [direct_merge::Write::Fence, direct_merge::Write::Intent,
                direct_merge::Write::Annotation, direct_merge::Write::Merge,
                direct_merge::Write::History, direct_merge::Write::Receipt] {
                for after in [false, true] {
                    assert!(trace.lock().unwrap().contains(&direct_merge::WriteBoundary { write, after }), "unreached {write:?}/{after}");
                }
            }
        }
        drop(issuer);
        // Reopen exactly as production does, without preparing history/receipts
        // through a separate adapter.reconcile call or a webhook.
        fixture.wakes = direct_merge::Wakes::default();
        let mut restarted = fixture.open(&settings);
        drain(&mut restarted);
        if fixture.direct.lock().unwrap().successes == 1 {
            assert!(restarted.snapshot().unwrap().rows.iter().any(|r|r.table == "delivery_history" && r.value["mode"] == "direct"),
                "unmasked Controller recovery must retain direct history at cut {cut}");
        }
        fixture.check_event(&settings);
        drain(&mut restarted);
        assert!(matches!(restarted.world().unwrap().nodes[0].phase, Phase::Closed(Outcome::Merged)));
        assert_eq!(fixture.direct.lock().unwrap().successes, 1);
        assert!(restarted.snapshot().unwrap().rows.iter().any(|r|r.table == "delivery_history" && r.value["mode"] == "direct"));
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
    fixture.direct.lock().unwrap().intent_roots = (0..2).map(|index| fixture.root.path().join(format!("issuer-{index}"))).collect();
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
    let mut controller = fixture.open(&settings);
    assert_eq!(controller.snapshot().unwrap().rows.into_iter().find(|r| r.table == "repos").unwrap().value, direct.value);
}


#[test]
fn missing_exact_head_approval_waits_without_put() {
    let mut database = support::Database::start();
    let settings = database.create("direct_no_approval");
    let fixture = DirectFixture::new();
    fixture.check(None);
    let (mut controller, delivery) = fixture.start(&settings);
    fixture.check(Some("success"));
    fixture.api.state.lock().unwrap().head_checks.retain(|check| check["name"] != "TL3 approval");
    let mut adapter = direct_merge::Adapter::new(fixture.api.clients(), 0,
        &fixture.root.path().join("direct"), settings.clone()).unwrap();
    assert!(matches!(adapter.reconcile(&delivery).unwrap(), Reconciliation::Pending));
    assert!(fixture.direct.lock().unwrap().puts.is_empty());
    assert_eq!(controller.world().unwrap().nodes[0].phase, Phase::Publishing);
}

#[test]
fn status_webhook_wakes_direct_protection_wait() {
    let mut database = support::Database::start();
    let settings = database.create("direct_status");
    let fixture = DirectFixture::new();
    fixture.check(None);
    let (mut controller, delivery) = fixture.start(&settings);
    drain(&mut controller);
    fixture.check(Some("success"));
    let body = json!({"repository":{"id":1,"name":"sandbox","owner":{"id":1,"login":"owner"}},"installation":{"id":3},"sha":delivery.head_sha,"state":"success"});
    deliver(&settings, &fixture.api, "status", &body);
    drain(&mut controller);
    assert_eq!(fixture.direct.lock().unwrap().successes, 1);
}


#[test]
fn r1_mixed_successful_check_and_pending_status_waits_for_event() {
    let mut database = support::Database::start();
    let settings = database.create("direct_mixed_status");
    let fixture = DirectFixture::new();
    fixture.check(Some("success"));
    {
        let mut state = fixture.api.state.lock().unwrap();
        state.required = false;
        state.classic = true;
        state.classic_context_only = true;
        state.statuses = json!([{"id":1200,"context":"Sandbox tests","state":"pending"}]);
    }
    let (mut controller, _) = fixture.start(&settings);
    drain(&mut controller);
    assert!(fixture.direct.lock().unwrap().puts.is_empty(), "a pending reported legacy status must prevent PUT even with a successful check run");
    let reads = fixture.direct.lock().unwrap().reads;
    for _ in 0..3 { assert!(!controller.drive_once().unwrap()); }
    assert_eq!(fixture.direct.lock().unwrap().reads, reads);
    fixture.api.state.lock().unwrap().statuses[0]["state"] = json!("success");
    fixture.check_event(&settings);
    drain(&mut controller);
    assert_eq!(fixture.direct.lock().unwrap().puts.len(), 1);
}

#[test]
fn r1_unlanded_put_silence_timer_cannot_retry_without_event() {
    let mut database = support::Database::start();
    let settings = database.create("direct_uncertain_timer");
    let fixture = DirectFixture::new();
    fixture.check(Some("success"));
    fixture.direct.lock().unwrap().status = Some(503);
    let (mut controller, _) = fixture.start(&settings);
    assert_eq!(fixture.direct.lock().unwrap().puts.len(), 1);
    let mut allowances = tl3_config::Allowances::default();
    allowances.github_mergify.silence_secs = 1;
    controller.set_allowances(allowances);
    fixture.direct.lock().unwrap().status = None;
    fixture.clock.0.set(fixture.clock.0.get() + Duration::from_secs(10));
    controller.look_due().unwrap();
    drain(&mut controller);
    assert_eq!(fixture.direct.lock().unwrap().puts.len(), 1, "a silence timer cannot authorize a retry PUT");
    let reads = fixture.direct.lock().unwrap().reads;
    fixture.check(None);
    fixture.check_event(&settings);
    drain(&mut controller);
    assert_eq!(fixture.direct.lock().unwrap().puts.len(), 1, "retry event must reverify pending protection");
    fixture.check(Some("success"));
    fixture.check_event(&settings);
    drain(&mut controller);
    assert_eq!(fixture.direct.lock().unwrap().puts.len(), 2);
    assert_eq!(fixture.direct.lock().unwrap().successes, 1);
    assert!(fixture.direct.lock().unwrap().reads > reads);
}

#[test]
fn r1_actions_infrastructure_and_pending_are_classified_by_q47() {
    let mut database = support::Database::start();
    for conclusion in ["startup_failure", "timed_out", "cancelled", "action_required", "stale"] {
        let settings = database.create(&format!("direct_class_{conclusion}"));
        let fixture = DirectFixture::new();
        fixture.check(Some(conclusion));
        fixture.api.state.lock().unwrap().head_checks.iter_mut().find(|c|c["id"] == 1100).unwrap()["app"]["slug"] = json!("github-actions");
        let (mut controller, _) = fixture.start(&settings);
        drain(&mut controller);
        assert!(fixture.direct.lock().unwrap().puts.is_empty());
        let snapshot = controller.snapshot().unwrap();
        let failures: Vec<_> = snapshot.rows.iter().filter(|r|r.table == "queue_facts" && r.value["evidence"]["CiFailed"].is_object()).collect();
        if matches!(conclusion, "action_required" | "stale") {
            assert!(failures.is_empty(), "{conclusion} must wait rather than fail");
            assert_eq!(controller.world().unwrap().nodes[0].round, 0);
        } else {
            assert!(failures.iter().any(|r|r.value["evidence"]["CiFailed"]["infra"] == true), "{conclusion} must retain infrastructure classification");
        }
    }
}

fn direct_catch_up(fixture: &DirectFixture, controller: &mut Controller, changed: bool) {
    use tl3_controller::ingress::GitHubTranslator;
    let pull = fixture.api.state.lock().unwrap().pull.clone();
    let changes = serde_json::from_value(json!({"repository":"owner/sandbox","issues":[],"comments":[],"labels":[],"closed":[],"pull_requests":if changed {vec![pull]} else {vec![]},"check_runs":[],"check_suites":[]})).unwrap();
    direct_merge::Translator::new(fixture.api.clients()).unwrap().translate(&ingress::GitHubDelivery::CatchUp { repository: Repository {owner:"owner".into(),name:"sandbox".into(),installation:3}, changes:Box::new(changes) }, &controller.snapshot().unwrap()).unwrap();
}
#[test]
fn r1_catch_up_skips_unchanged_closed_delivery_history() {
    let mut database = support::Database::start();
    let settings = database.create("direct_catch_history");
    let fixture = DirectFixture::new();
    let (mut controller, _) = fixture.start(&settings);
    drain(&mut controller);
    let reads = fixture.direct.lock().unwrap().reads;
    direct_catch_up(&fixture, &mut controller, false);
    assert_eq!(fixture.direct.lock().unwrap().reads, reads, "unchanged completed history must not be reread");
    direct_catch_up(&fixture, &mut controller, true);
    assert!(fixture.direct.lock().unwrap().reads <= reads + 1);
}
#[test]
fn r1_retirement_reclaims_terminal_direct_records() {
    let mut database = support::Database::start();
    let settings = database.create("direct_reclaim");
    let fixture = DirectFixture::new();
    let (mut controller, delivery) = fixture.start(&settings);
    drain(&mut controller);
    let files: Vec<_> = std::fs::read_dir(fixture.root.path().join("direct")).unwrap().map(|e|e.unwrap().path()).filter(|p|p.extension().is_some_and(|e|e=="intent"||e=="merged")).collect();
    assert!(files.is_empty(), "retired terminal evidence must be reclaimed: {files:?}");
    let mut adapter = fixture.adapter(&settings);
    assert!(matches!(adapter.reconcile(&delivery).unwrap(), Reconciliation::Found(_)));
    assert_eq!(fixture.direct.lock().unwrap().puts.len(), 1);
}

#[test]
fn r1_retirement_crash_before_intent_delete() { retirement_crash(0); }
#[test]
fn r1_retirement_crash_after_intent_delete() { retirement_crash(1); }
#[test]
fn r1_retirement_crash_before_receipt_delete() { retirement_crash(2); }
#[test]
fn r1_retirement_crash_after_receipt_delete() { retirement_crash(3); }
fn retirement_crash(cut: usize) {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let mut database = support::Database::start();
    {
        let settings = database.create(&format!("direct_reclaim_crash_{cut}"));
        let fixture = DirectFixture::new();
        fixture.check(None);
        let (controller, _) = fixture.start(&settings);
        drop(controller);
        let count = Arc::new(AtomicUsize::new(0));
        let seen = count.clone();
        let adapter = fixture.adapter(&settings).with_write_boundary(move |boundary| {
            if boundary.write == direct_merge::Write::Reclaim && seen.fetch_add(1, Ordering::SeqCst) == cut {
                panic!("#830 retirement crash");
            }
        });
        let mut controller = fixture.open_adapter(&settings, adapter);
        fixture.check(Some("success"));
        fixture.check_event(&settings);
        assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| drain(&mut controller))).is_err());
        assert_eq!(count.load(Ordering::SeqCst), cut + 1);
        let snapshot = controller.snapshot().unwrap();
        assert!(snapshot.rows.iter().any(|r|r.table == "delivery_history" && r.value["mode"] == "direct"));
        assert!(matches!(controller.world().unwrap().nodes[0].phase, Phase::Stopping(Outcome::Merged) | Phase::Closed(Outcome::Merged)));
        drop(controller);
        let mut restarted = fixture.open(&settings);
        drain(&mut restarted);
        assert_eq!(fixture.direct.lock().unwrap().puts.len(), 1);
        assert!(matches!(restarted.world().unwrap().nodes[0].phase, Phase::Closed(Outcome::Merged)));
        assert!(!std::fs::read_dir(fixture.root.path().join("direct")).unwrap().any(|e| e.unwrap().path().extension().is_some_and(|ext|ext=="intent"||ext=="merged")));
    }
}
#[test]
fn r1_board_retains_actual_delivery_mode_after_configuration_change() {
    let mut database = support::Database::start();
    let settings = database.create("direct_board_history");
    let fixture = DirectFixture::new();
    let (mut controller, _) = fixture.start(&settings);
    drain(&mut controller);
    let mut repo = direct_repo();repo.delivery = tl3_config::Delivery::Queue;
    controller.reconcile_repository_delivery(&[repo]).unwrap();
    let mut reader = Store::open(&settings).unwrap().reader().unwrap();
    let mut tx = reader.transaction().unwrap();
    let rows = tl3_controller::board::Rows::read(&mut tx).unwrap();
    assert!(rows.requests[0].direct_delivery, "switching config must not erase completed direct delivery history");
}


fn store_direct_check_event(fixture: &DirectFixture, settings: &tl3_config::Store, id: &str) {
    let check = fixture.api.state.lock().unwrap().head_checks.iter()
        .find(|check| check["name"] == "Sandbox tests").unwrap().clone();
    let mut body = webhook_fixture();
    body["action"] = json!("completed");
    body["repository"]["owner"]["login"] = json!("owner");
    body["repository"]["name"] = json!("sandbox");
    body["installation"] = json!({"id":3});
    body["check_run"] = check;
    assert!(Store::open(settings).unwrap().receive_ingress("github-webhook", id, "check_run", &serde_json::to_vec(&body).unwrap()).unwrap());
}

#[test]
fn r2_a_restart_after_put_requires_new_post_recovery_event() {
    let mut database = support::Database::start();
    let settings = database.create("direct_r2_restart");
    let mut fixture = DirectFixture::new();
    fixture.check(Some("success"));
    fixture.direct.lock().unwrap().status = Some(503);
    let (controller, _) = fixture.start(&settings);
    assert_eq!(fixture.direct.lock().unwrap().puts.len(), 1);
    drop(controller);
    fixture.wakes = direct_merge::Wakes::default();
    fixture.direct.lock().unwrap().status = None;
    let mut restarted = fixture.open(&settings);
    drain(&mut restarted);
    assert_eq!(fixture.direct.lock().unwrap().puts.len(), 1, "restart cannot reauthorize a consumed attempt");
    store_direct_check_event(&fixture, &settings, "post-recovery");
    drain(&mut restarted);
    assert_eq!(fixture.direct.lock().unwrap().puts.len(), 2);
    assert_eq!(fixture.direct.lock().unwrap().successes, 1);
}

#[test]
fn r2_a_crash_before_cursor_advance_cannot_replay_authorization() {
    let mut database = support::Database::start();
    let settings = database.create("direct_r2_replay");
    let mut fixture = DirectFixture::new();
    fixture.check(None);
    let (controller, _) = fixture.start(&settings);
    drop(controller);
    fixture.check(Some("success"));
    fixture.direct.lock().unwrap().status = Some(503);
    let adapter = fixture.adapter(&settings).with_write_boundary(|b| {
        if b == (direct_merge::WriteBoundary { write: direct_merge::Write::Merge, after: true }) {
            panic!("crash after PUT before translator cursor advance");
        }
    });
    let mut controller = fixture.open_adapter(&settings, adapter);
    store_direct_check_event(&fixture, &settings, "replayed-authorizer");
    assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| ingress::drain(&mut controller))).is_err());
    let mut store = Store::open(&settings).unwrap();
    let cursor = store.translator_cursor("github-translator:mergify").unwrap();
    assert!(store.ingress_after(cursor).unwrap().iter().any(|e|e.delivery_key == "replayed-authorizer"), "crash must leave the event replayable");
    drop(controller);
    fixture.wakes = direct_merge::Wakes::default();
    fixture.direct.lock().unwrap().status = None;
    let mut restarted = fixture.open(&settings);
    drain(&mut restarted);
    assert_eq!(fixture.direct.lock().unwrap().puts.len(), 1, "replayed event cannot reauthorize PUT");
    store_direct_check_event(&fixture, &settings, "fresh-after-replay");
    drain(&mut restarted);
    assert_eq!(fixture.direct.lock().unwrap().puts.len(), 2);
}

#[test]
fn r2_a_older_queued_event_cannot_reauthorize_put() {
    let mut database = support::Database::start();
    let settings = database.create("direct_r2_queued");
    let fixture = DirectFixture::new();
    fixture.check(None);
    let (controller, _) = fixture.start(&settings);
    drop(controller);
    fixture.check(Some("success"));
    fixture.direct.lock().unwrap().status = Some(503);
    let direct = fixture.direct.clone();
    let adapter = fixture.adapter(&settings).with_write_boundary(move |b| {
        if b == (direct_merge::WriteBoundary { write: direct_merge::Write::Merge, after: true }) {
            direct.lock().unwrap().status = None;
        }
    });
    let mut controller = fixture.open_adapter(&settings, adapter);
    store_direct_check_event(&fixture, &settings, "first-authorizer");
    store_direct_check_event(&fixture, &settings, "already-queued");
    ingress::drain(&mut controller).unwrap();
    assert_eq!(fixture.direct.lock().unwrap().puts.len(), 1, "events already queued at PUT cannot authorize its retry");
    store_direct_check_event(&fixture, &settings, "new-after-put");
    drain(&mut controller);
    assert_eq!(fixture.direct.lock().unwrap().puts.len(), 2);
    assert_eq!(fixture.direct.lock().unwrap().successes, 1);
}

#[test]
fn r1_actions_runner_annotations_keep_infrastructure_evidence() {
    let mut database = support::Database::start();
    let settings = database.create("direct_runner_annotations");
    let fixture = DirectFixture::new();
    fixture.check(Some("failure"));
    {
        let mut state = fixture.api.state.lock().unwrap();
        state.head_checks.iter_mut().find(|c|c["id"] == 1100).unwrap()["app"]["slug"] = json!("github-actions");
        state.annotations = json!([{"annotation_level":"failure","message":"The runner has lost communication with the server. Please retry."}]);
    }
    let (mut controller, _) = fixture.start(&settings);
    drain(&mut controller);
    assert!(controller.snapshot().unwrap().rows.iter().any(|r|r.table == "queue_facts" && r.value["evidence"]["CiFailed"]["infra"] == true));
    assert!(fixture.direct.lock().unwrap().puts.is_empty());
}

#[test]
fn r1_unknown_merge_history_is_not_relabelled_as_direct() {
    let mut database = support::Database::start();
    let settings = database.create("direct_external_history");
    let fixture = DirectFixture::new();
    fixture.direct.lock().unwrap().already_merged = true;
    let (mut controller, _) = fixture.start(&settings);
    drain(&mut controller);
    let mut reader = Store::open(&settings).unwrap().reader().unwrap();
    let mut tx = reader.transaction().unwrap();
    let rows = tl3_controller::board::Rows::read(&mut tx).unwrap();
    assert!(!rows.requests[0].direct_delivery, "config alone cannot attribute an old merge to direct delivery");
}

#[test]
fn r2_b_webhook_merge_retires_without_stale_history_commit() {
    let mut database = support::Database::start();
    let settings = database.create("direct_r2_terminal_webhook");
    let fixture = DirectFixture::new();
    {
        let mut direct = fixture.direct.lock().unwrap();
        direct.lost_reply = true;
        direct.fail_read_after_put = true;
    }
    let (mut controller, _) = fixture.start(&settings);
    assert_eq!(fixture.direct.lock().unwrap().puts.len(), 1);
    assert_eq!(fixture.direct.lock().unwrap().successes, 1);
    assert!(!controller.snapshot().unwrap().rows.iter().any(|r|r.table == "delivery_history"));
    let mut body = webhook_fixture();
    body["action"] = json!("closed");
    body["repository"]["owner"]["login"] = json!("owner");
    body["repository"]["name"] = json!("sandbox");
    body["installation"] = json!({"id":3});
    body["pull_request"] = fixture.api.state.lock().unwrap().pull.clone();
    assert!(body["pull_request"]["merged_at"].is_string());
    assert!(Store::open(&settings).unwrap().receive_ingress("github-webhook", "terminal-merged", "pull_request", &serde_json::to_vec(&body).unwrap()).unwrap());
    drain(&mut controller);
    assert!(matches!(controller.world().unwrap().nodes[0].phase, Phase::Closed(Outcome::Merged)));
    let snapshot = controller.snapshot().unwrap();
    assert!(snapshot.rows.iter().any(|r|r.table == "delivery_history" && r.value["mode"] == "direct"));
    assert!(!snapshot.rows.iter().any(|r|r.table == "retiring_effects"));
    assert!(!std::fs::read_dir(fixture.root.path().join("direct")).unwrap().any(|e|e.unwrap().path().extension().is_some_and(|ext|ext=="intent"||ext=="merged")));
    assert_eq!(fixture.direct.lock().unwrap().puts.len(), 1);
}

#[test]
fn r2_c_healthy_event_wait_never_accumulates_read_outages() {
    let mut database = support::Database::start();
    let settings = database.create("direct_r2_healthy_wait");
    let fixture = DirectFixture::new();
    fixture.check(None);
    let (mut controller, _) = fixture.start(&settings);
    drain(&mut controller);
    let reads = fixture.direct.lock().unwrap().reads;
    for allowance in 0..9 {
        fixture.clock.0.set(fixture.clock.0.get() + Duration::from_secs(10));
        controller.look_due().unwrap();
        drain(&mut controller);
        let snapshot = controller.snapshot().unwrap();
        assert!(!snapshot.rows.iter().any(|r|r.table == "incidents"), "healthy allowance {allowance} must not create an incident");
        for row in snapshot.rows.iter().filter(|r|r.table == "allowances") {
            assert_eq!(row.value["read_failures"].as_u64().unwrap_or(0), 0, "healthy wait must not be counted as a failed read");
            assert_eq!(row.value["read_incidents"].as_u64().unwrap_or(0), 0);
            assert!(row.value["failure"].is_null(), "healthy wait must not report upstream record unavailable: {}", row.value);
        }
        assert!(!snapshot.rows.iter().any(|r|r.value.to_string().contains("upstream record unavailable")));
    }
    assert_eq!(fixture.direct.lock().unwrap().reads, reads, "event wait must not poll");
    assert!(fixture.direct.lock().unwrap().puts.is_empty());
    fixture.check(Some("success"));
    store_direct_check_event(&fixture, &settings, "healthy-wait-ended");
    drain(&mut controller);
    assert_eq!(fixture.direct.lock().unwrap().puts.len(), 1);
}

fn direct_intent_path(fixture: &DirectFixture) -> std::path::PathBuf {
    let paths: Vec<_> = std::fs::read_dir(fixture.root.path().join("direct")).unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|ext|ext == "intent")).collect();
    assert_eq!(paths.len(), 1);
    paths[0].clone()
}

#[test]
fn r3_a_annotation_crash_preserves_legacy_and_current_merge_attempts() {
    let mut database = support::Database::start();
    for legacy in [true, false] {
        let settings = database.create(if legacy {"direct_r3_legacy"} else {"direct_r3_current"});
        let mut fixture = DirectFixture::new();
        fixture.check(Some("success"));
        fixture.direct.lock().unwrap().status = Some(503);
        let (controller, _) = fixture.start(&settings);
        assert_eq!(fixture.direct.lock().unwrap().puts.len(), 1);
        drop(controller);
        let path = direct_intent_path(&fixture);
        let mut intent: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(intent["action"], "Merge");
        if legacy { intent.as_object_mut().unwrap().remove("consumed_through"); }
        std::fs::write(&path, serde_json::to_vec(&intent).unwrap()).unwrap();
        {
            let mut state = fixture.api.state.lock().unwrap();
            state.pull["body"] = json!(state.pull["body"].as_str().unwrap().replace(direct_merge::VISIBILITY, ""));
        }
        fixture.wakes = direct_merge::Wakes::default();
        let adapter = fixture.adapter(&settings).with_write_boundary(|boundary| {
            if boundary == (direct_merge::WriteBoundary {write:direct_merge::Write::Annotation, after:true}) {
                panic!("crash after annotation before the next Merge intent");
            }
        });
        let mut controller = fixture.open_adapter(&settings, adapter);
        store_direct_check_event(&fixture, &settings, "authorize-annotation");
        assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| ingress::drain(&mut controller))).is_err());
        let intent: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(intent["action"], "Annotate");
        assert_eq!(fixture.direct.lock().unwrap().puts.len(), 1);
        drop(controller);
        fixture.wakes = direct_merge::Wakes::default();
        fixture.direct.lock().unwrap().status = None;
        let mut recovered = fixture.open(&settings);
        drain(&mut recovered);
        assert_eq!(fixture.direct.lock().unwrap().puts.len(), 1,
            "annotation must not erase a prior Merge attempt, legacy={legacy}; recovery needs new ingress");
        store_direct_check_event(&fixture, &settings, "post-annotation-recovery");
        drain(&mut recovered);
        assert_eq!(fixture.direct.lock().unwrap().puts.len(), 2);
        assert_eq!(fixture.direct.lock().unwrap().successes, 1);
        assert!(matches!(recovered.world().unwrap().nodes[0].phase, Phase::Closed(Outcome::Merged)));
    }
}

#[test]
fn r3_b_unmasked_controller_recovery_keeps_history_before_merged_fact() {
    let mut database = support::Database::start();
    let settings = database.create("direct_r3_observation_history");
    let mut fixture = DirectFixture::new();
    let adapter = fixture.adapter(&settings).with_write_boundary(|boundary| {
        if boundary == (direct_merge::WriteBoundary {write:direct_merge::Write::History, after:false}) {
            panic!("crash after successful PUT before History");
        }
    });
    let controller = fixture.open_adapter(&settings, adapter);
    assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| fixture.start_controller(&settings, controller))).is_err());
    assert_eq!(fixture.direct.lock().unwrap().puts.len(), 1);
    assert_eq!(fixture.direct.lock().unwrap().successes, 1);
    assert!(!Store::open(&settings).unwrap().load().unwrap().rows.iter().any(|r|r.table == "delivery_history"));
    fixture.wakes = direct_merge::Wakes::default();
    let writes = Arc::new(Mutex::new(Vec::new()));
    let observed_writes = writes.clone();
    let adapter = fixture.adapter(&settings).with_write_boundary(move |boundary| {
        observed_writes.lock().unwrap().push(boundary);
    });
    let mut recovered = fixture.open_adapter(&settings, adapter);
    // No adapter reconciliation preparation, no webhook and no ingress delivery.
    drain(&mut recovered);
    assert!(matches!(recovered.world().unwrap().nodes[0].phase, Phase::Closed(Outcome::Merged)));
    assert!(recovered.snapshot().unwrap().rows.iter().any(|r|r.table == "delivery_history" && r.value["mode"] == "direct"),
        "observations() must persist direct history before its terminal fact retires the intent");
    let writes = writes.lock().unwrap();
    let position = |write, after| writes.iter().position(|boundary| *boundary == direct_merge::WriteBoundary {write, after}).unwrap();
    assert!(position(direct_merge::Write::History, true) < position(direct_merge::Write::Receipt, false));
    assert!(position(direct_merge::Write::Receipt, true) < position(direct_merge::Write::Reclaim, false));
    let mut reader = Store::open(&settings).unwrap().reader().unwrap();
    let mut tx = reader.transaction().unwrap();
    let rows = tl3_controller::board::Rows::read(&mut tx).unwrap();
    assert!(rows.requests[0].direct_delivery, "unmasked recovery must keep the board's direct delivery marker");
    assert_eq!(fixture.direct.lock().unwrap().puts.len(), 1);
}

/// PRD #878 story 16: repeated delivery of the same outcome has one kernel path.
#[test]
fn fake_and_production_merge_outcomes_have_identical_receipts() {
    use tl3_controller::executor::Observation;
    let mut database = support::Database::start();
    let mut results = Vec::new();
    for fake in [true, false] {
        let settings = database.create(if fake { "outcome_fake" } else { "outcome_production" });
        let api = Server::new();
        let root = tempfile::tempdir().unwrap();
        let mut controller = queued(&settings, &api, root.path(), true);
        if fake {
            let snapshot = controller.snapshot().unwrap();
            let row = snapshot.rows.iter().find(|row| row.table == "watches").unwrap();
            let mut delivery: Delivery = serde_json::from_value(row.value.clone()).unwrap();
            delivery.pull_request = Some(100);
            delivery.head_sha = Some(api.state.lock().unwrap().pull["head"]["sha"].as_str().unwrap().into());
            let mut adapter = adapters::mergify::Fake::new(root.path());
            for key in ["first", "repeat"] {
                let completion = adapter.execute(&delivery).unwrap();
                for observation in completion.observations {
                    controller.observe(key, observation).unwrap();
                }
                drain(&mut controller);
            }
            controller.observe("late-ci", Observation::CiFailed {
                node: NodeId(0), infra: false, checks: vec![],
            }).unwrap();
            drain(&mut controller);
        } else {
            api.state.lock().unwrap().merged(false);
            api.state.lock().unwrap().checks = fixture("merged", "checks");
            catch_up(&settings, &api, &mut controller);
            drain(&mut controller);
            catch_up(&settings, &api, &mut controller);
            drain(&mut controller);
            // A late check still comes through the production queue adapter vocabulary.
            controller.observe("late-ci", Observation::Queue(adapters::mergify::QueueFact {
                repository: bootstrap().repos[0].clone(), number: 100,
                evidence: adapters::mergify::Evidence::CiFailed {
                    check: 1001, completed_at: None,
                    head: api.state.lock().unwrap().pull["head"]["sha"].as_str().unwrap().into(),
                    infra: false, head_check: true, details: Default::default(),
                },
            })).unwrap();
            drain(&mut controller);
        }
        let facts: Vec<_> = Store::open(&settings).unwrap()
            .events_after(tl3_store::Revision::ZERO, 2000).unwrap().into_iter()
            .filter_map(|event| event.receipt.result["kernel_event"]["Fact"]["fact"].as_object().cloned())
            .map(Value::Object).collect();
        results.push((facts, serde_json::to_value(controller.world().unwrap()).unwrap()));
    }
    assert_eq!(results[0], results[1], "fake and production must apply identical facts and world rows");
}
