use tl3_controller::adapters::direct_merge;

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
    /// PR reads that fail with HTTP 502 once a durable acceptance exists.
    fail_reads_after_acceptance: usize,
    failed_reads: usize,
    no_merge_method: bool,
    different_tree: bool,
    different_parent: bool,
    landed_parent: Option<String>,
    landed_status: Option<u16>,
    head_status: Option<u16>,
    branch_status_after_merge: Option<u16>,
    protection_refused_after_merge: bool,
    reply_without_sha: bool,
    missing_merge_commit: bool,
    rebase_method: bool,
    checks_fail_after_merge: bool,
    merge_error_after_success: Option<u16>,
}
fn accepted_records(root: &std::path::Path) -> Vec<Value> {
    std::fs::read_dir(root).into_iter().flatten()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "accepted"))
        .map(|path| serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap())
        .collect()
}
#[derive(Clone)]
struct DirectApi { base: Api, direct: Arc<Mutex<DirectState>> }
impl Respond for DirectApi {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let path = request.url.path();
        let method = request.method.as_str();
        if method == "GET" && path == "/repos/owner/sandbox" {
            let direct = self.direct.lock().unwrap();
            let allowed = !direct.no_merge_method;
            return ResponseTemplate::new(200).set_body_json(json!({"allow_merge_commit":allowed && !direct.rebase_method,"allow_squash_merge":allowed && !direct.rebase_method,"allow_rebase_merge":allowed}));
        }
        if method == "GET" && path.starts_with("/repos/owner/sandbox/git/commits/") {
            let sha = path.rsplit('/').next().unwrap();
            let direct = self.direct.lock().unwrap();
            let state = self.base.0.lock().unwrap();
            let landed = sha == "dddddddddddddddddddddddddddddddddddddddd";
            if let Some(status) = if landed {direct.landed_status} else {direct.head_status} { return ResponseTemplate::new(status); }
            return ResponseTemplate::new(200).set_body_json(json!({"sha":sha,
                "tree":{"sha":if landed && direct.different_tree {"2".repeat(40)} else {"1".repeat(40)}},
                "parents":[{"sha":if landed && direct.different_parent {json!("3".repeat(40))} else {direct.landed_parent.as_ref().map_or_else(||state.pull["base"]["sha"].clone(), |base|json!(base))}}]}));
        }
        if method == "GET" && (path.contains("/branches/") || path.contains("/rules/branches/")) {
            let direct = self.direct.lock().unwrap();
            if self.base.0.lock().unwrap().pull["merged_at"].is_string() {
                if direct.protection_refused_after_merge && path.contains("/rules/branches/") { return ResponseTemplate::new(403); }
                if !path.contains("/rules/") && let Some(status) = direct.branch_status_after_merge { return ResponseTemplate::new(status); }
            }
        }
        if method == "GET" && path == "/repos/owner/sandbox/pulls/100" {
            let mut direct = self.direct.lock().unwrap();
            direct.reads += 1;
            if direct.fail_reads_after_acceptance > 0 && !accepted_records(&direct.root).is_empty() {
                direct.fail_reads_after_acceptance -= 1;
                direct.failed_reads += 1;
                return ResponseTemplate::new(502);
            }
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
            assert_eq!(body["merge_method"], if direct.rebase_method {"rebase"} else {"squash"});
            direct.puts.push(body.clone());
            let mut state = self.base.0.lock().unwrap();
            if state.pull["merged_at"].is_string() { return ResponseTemplate::new(405); }
            if body["sha"] != state.pull["head"]["sha"] { return ResponseTemplate::new(409); }
            if direct.reject_as_merged { state.merged(false); return ResponseTemplate::new(405); }
            if let Some(status) = direct.status { return ResponseTemplate::new(status); }
            direct.landed_parent = Some(state.live_base.clone().unwrap_or_else(||state.pull["base"]["sha"].as_str().unwrap().to_owned()));
            state.merged(false);
            direct.successes += 1;
            if direct.missing_merge_commit { state.pull["merge_commit_sha"] = Value::Null; }
            if direct.checks_fail_after_merge { for check in &mut state.head_checks { if check["name"] == "Sandbox tests" { check["conclusion"] = json!("failure"); } } }
            if let Some(status) = direct.merge_error_after_success { return ResponseTemplate::new(status); }
            if direct.reply_without_sha { return ResponseTemplate::new(200).set_body_json(json!({"merged":true,"sha":null,"message":"merged"})); }
            return if direct.lost_reply { ResponseTemplate::new(503) }
                else { ResponseTemplate::new(200).set_body_json(json!({"merged":true,"sha":"dddddddddddddddddddddddddddddddddddddddd","message":"merged"})) };
        }
        self.base.respond(request)
    }
}
struct DirectFixture { api: Server, root: tempfile::TempDir, direct: Arc<Mutex<DirectState>>, wakes: direct_merge::Wakes, clock: Clock }
impl DirectFixture {
    fn new() -> Self {
        let api = Server::new();
        let root = tempfile::tempdir().unwrap();
        let direct = Arc::new(Mutex::new(DirectState { root: root.path().join("direct"), ..Default::default() }));
        api._runtime.block_on(async {
            Mock::given(|_: &Request| true).respond_with(DirectApi { base: Api(api.state.clone()), direct: direct.clone() }).with_priority(1).mount(&api.server).await;
        });
        api.state.lock().unwrap().pull["mergeable"] = json!(true);
        Self { api, root, direct, wakes: direct_merge::Wakes::default(), clock: Clock(std::rc::Rc::new(std::cell::Cell::new(std::time::SystemTime::now()))) }
    }
    fn adapter(&self, settings: &tl3_config::Store) -> direct_merge::Adapter {
        direct_merge::Adapter::new(self.api.clients(), 0, &self.root.path().join("direct"), settings.clone()).unwrap().with_wakes(self.wakes.clone())
    }
    fn open(&self, settings: &tl3_config::Store) -> Controller {
        self.open_adapter(settings, self.adapter(settings))
    }
    fn open_adapter(&self, settings: &tl3_config::Store, adapter: direct_merge::Adapter) -> Controller {
        self.open_adapters(settings, adapter, None)
    }
    fn open_adapters(&self, settings: &tl3_config::Store, adapter: direct_merge::Adapter, worktrees: Option<SameHeadRebase>) -> Controller {
        let mut registry = adapters::simulation(self.root.path()).unwrap();
        registry.register(Upstream::Delivery, adapter);
        if let Some(worktrees) = worktrees { registry.register(Upstream::Worktrees, worktrees); }
        let mut controller = Controller::open_with_clock(Store::open(settings).unwrap(), registry, bootstrap(), self.clock.clone()).unwrap();
        let mut allowances = tl3_config::Allowances::default();
        allowances.github_delivery.silence_secs = 1;
        controller.set_allowances(allowances);
        controller.register_github_translator("delivery", direct_merge::Translator::new(self.api.clients()).unwrap().with_wakes(self.wakes.clone()));
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
fn pending_direct_handoff_releases_full_gate_and_keeps_authenticated_wake() {
    let mut database = support::Database::start();
    let settings = database.create("direct_neutral_delivery");
    let fixture = DirectFixture::new();
    fixture.check(None);
    let (mut controller, delivery) = fixture.start(&settings);
    drain(&mut controller);
    let world = controller.world().unwrap();
    world.check().unwrap();
    let accepted = world.nodes[0];
    assert!(!accepted.holds, "durable Direct responsibility must release the full change gate before checks pass");
    assert_eq!(accepted.phase.name(), "AwaitingDelivery");
    assert_eq!(accepted.generation, delivery.generation + 1);
    assert_eq!(accepted.head, accepted.checked);
    assert_eq!(accepted.head, accepted.approved);
    assert!(accepted.approved.is_some());
    assert!(!accepted.footprint.is_empty());
    assert!(accepted.effect.is_none() && accepted.run.is_none() && accepted.job.is_none());
    assert!(!accepted.pool && accepted.ticket.is_none());
    assert!(controller.snapshot().unwrap().rows.iter().any(|row| row.table == "watches" && row.id == delivery.key));
    assert!(fixture.direct.lock().unwrap().puts.is_empty());

    controller.intake(LabelledIssue { delivery:"direct-overlap".into(), owner:"owner".into(), repo:"sandbox".into(), installation:3, number:13, title:"overlapping change".into(), actor:42, label:"tl3".into(), labels:vec![] }).unwrap();
    let mut overlapping = false;
    for _ in 0..100 {
        let world = controller.world().unwrap();
        world.check().unwrap();
        if world.nodes.len() > 1 && world.nodes[1].holds {
            assert_eq!(world.nodes[1].footprint, accepted.footprint, "fixture must exercise overlapping files");
            assert!(!world.nodes[0].holds);
            assert_eq!(world.nodes[0].approved, accepted.approved);
            overlapping = true;
            break;
        }
        assert!(controller.drive_once().unwrap(), "released slot must admit the next change");
    }
    assert!(overlapping, "overlapping Ready work must acquire the released slot");
    fixture.check(Some("success"));
    fixture.check_event(&settings);
    ingress::drain(&mut controller).unwrap();
    for _ in 0..100 {
        let world = controller.world().unwrap();
        world.check().unwrap();
        if world.nodes[0].phase == Phase::Closed(Outcome::Merged) {
            let direct = fixture.direct.lock().unwrap();
            assert_eq!(direct.puts.len(), 1);
            assert_eq!(direct.puts[0]["sha"], json!(delivery.head_sha));
            return;
        }
        controller.drive_once().unwrap();
    }
    panic!("accepted watch lost authenticated wake: world={:?}, puts={:?}, rows={:?}", controller.world().unwrap(), fixture.direct.lock().unwrap().puts, controller.snapshot().unwrap().rows.iter().filter(|r|matches!(r.table.as_str(), "allowances" | "incidents" | "watches")).collect::<Vec<_>>());
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
    assert!(matches!(restarted.reconcile(&delivery).unwrap(), Reconciliation::Pending), "closed delivery cannot revive authority");
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
    let node = controller.world().unwrap().nodes[0];
    assert_eq!(node.phase, Phase::AwaitingDelivery);
    assert!(!node.holds && node.effect.is_none());
    fixture.check(Some("success"));
    fixture.check_event(&settings);
    drain(&mut controller);
    assert_eq!(fixture.direct.lock().unwrap().puts.len(), 1, "incident must fence later authenticated wakes");
    assert_eq!(fixture.direct.lock().unwrap().successes, 0);
}

#[test]
fn removed_queue_route_is_rejected() {
    assert!(serde_json::from_value::<tl3_config::Repository>(json!({
        "owner":"owner", "name":"sandbox", "installation_id":3, "delivery":"queue"
    })).is_err());
}


#[test]
fn already_merged_405_reconciles_without_failure() {
    let mut database = support::Database::start();
    let settings = database.create("direct_405");
    let fixture = DirectFixture::new();
    fixture.check(Some("success"));
    fixture.direct.lock().unwrap().reject_as_merged = true;
    let (mut controller, _) = fixture.start(&settings);
    drain(&mut controller);
    assert_eq!(fixture.direct.lock().unwrap().puts.len(), 1);
    assert!(matches!(controller.world().unwrap().nodes[0].phase, Phase::Closed(Outcome::Merged)));
    assert!(!controller.snapshot().unwrap().rows.iter().any(|r| r.table == "incidents"));
}

#[test]
fn restart_at_acceptance_and_fence_boundaries_converges() {
    restart_direct_write_cuts(&[direct_merge::Write::Fence, direct_merge::Write::Acceptance]);
}
#[test]
fn restart_at_intent_and_mutation_boundaries_converges() {
    restart_direct_write_cuts(&[direct_merge::Write::Intent, direct_merge::Write::Annotation, direct_merge::Write::Merge]);
}
#[test]
fn restart_at_history_and_receipt_boundaries_converges() {
    restart_direct_write_cuts(&[direct_merge::Write::History, direct_merge::Write::Receipt]);
}
fn restart_direct_write_cuts(writes: &[direct_merge::Write]) {
    let mut database = support::Database::start();
    for &write in writes {
        for after in [false, true] {
            let settings = database.create(&format!("direct_crash_{write:?}_{after}").to_lowercase());
            let mut fixture = DirectFixture::new();
            fixture.check(Some("success"));
            let target = direct_merge::WriteBoundary {write, after};
            let trace = Arc::new(Mutex::new(Vec::new()));
            let recorded = trace.clone();
            let adapter = fixture.adapter(&settings).with_write_boundary(move |boundary| {
                recorded.lock().unwrap().push(boundary);
                if boundary == target { panic!("injected durable write crash at {target:?}"); }
            });
            let controller = fixture.open_adapter(&settings, adapter);
            let crashed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| fixture.start_controller(&settings, controller)));
            assert!(crashed.is_err(), "mandatory boundary {target:?} did not crash");
            assert!(trace.lock().unwrap().contains(&target));
            fixture.wakes = direct_merge::Wakes::default();
            let mut restarted = fixture.open(&settings);
            drain(&mut restarted);
            if fixture.direct.lock().unwrap().successes == 1 {
                assert!(restarted.snapshot().unwrap().rows.iter().any(|r|r.table == "delivery_history" && r.value["mode"] == "direct"));
            }
            fixture.check_event(&settings);
            drain(&mut restarted);
            assert_eq!(restarted.world().unwrap().nodes[0].phase, Phase::Closed(Outcome::Merged));
            assert_eq!(fixture.direct.lock().unwrap().successes, 1);
            assert!(restarted.snapshot().unwrap().rows.iter().any(|r|r.table == "delivery_history" && r.value["mode"] == "direct"));
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
    fixture.direct.lock().unwrap().intent_roots = (0..2).map(|index| fixture.root.path().join(format!("issuer-{index}"))).collect();
    let handles: Vec<_> = issuers.into_iter().map(|mut adapter| {
        let delivery = delivery.clone();
        std::thread::spawn(move || adapter.execute(&delivery))
    }).collect();
    for handle in handles { assert_eq!(handle.join().unwrap().unwrap().ack, tl3_kernel::Ack::DeliveryAccepted); }
    let state = fixture.direct.lock().unwrap();
    assert_eq!(state.puts.len(), 2);
    assert_eq!(state.successes, 1);
    assert!(state.puts.iter().all(|put| put["sha"] == json!(delivery.head_sha)));
}

#[test]
fn direct_configuration_does_not_add_a_kernel_route() {
    let mut database = support::Database::start();
    let settings = database.create("direct_config");
    let fixture = DirectFixture::new();
    let mut controller = fixture.open(&settings);
    let snapshot = controller.snapshot().unwrap();
    let repo = snapshot.rows.iter().find(|r| r.table == "repos").unwrap();
    assert!(repo.value.get("delivery").is_none());
    assert!(!serde_json::to_value(controller.world().unwrap()).unwrap().to_string().contains("delivery_route"));
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
    let Reconciliation::Found(completion) = adapter.reconcile(&delivery).unwrap() else { panic!("durable responsibility must be accepted before approval checks report"); };
    assert_eq!(completion.ack, tl3_kernel::Ack::DeliveryAccepted);
    assert!(fixture.direct.lock().unwrap().puts.is_empty());
    assert_eq!(controller.world().unwrap().nodes[0].phase, Phase::AwaitingDelivery);
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
    allowances.github_delivery.silence_secs = 1;
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
        let failures: Vec<_> = snapshot.rows.iter().filter(|r|r.table == "delivery_outcomes" && r.value["outcome"]["CiFailed"].is_object()).collect();
        if matches!(conclusion, "action_required" | "stale") {
            assert!(failures.is_empty(), "{conclusion} must wait rather than fail");
            assert_eq!(controller.world().unwrap().nodes[0].round, 0);
        } else {
            assert!(failures.iter().any(|r|r.value["outcome"]["CiFailed"]["infra"] == true), "{conclusion} must retain infrastructure classification");
        }
    }
}

#[test]
fn waiting_direct_infrastructure_preserves_credit_and_findings() {
    let mut database = support::Database::start();
    for conclusion in ["startup_failure", "timed_out", "cancelled"] {
        let settings = database.create(&format!("direct_waiting_infra_{conclusion}"));
        let fixture = DirectFixture::new();
        fixture.check(None);
        fixture.api.state.lock().unwrap().head_checks.iter_mut()
            .find(|check| check["id"] == 1100).unwrap()["app"]["slug"] = json!("github-actions");
        let (mut controller, delivery) = fixture.start(&settings);
        drain(&mut controller);
        let before = controller.world().unwrap();
        assert_eq!(before.nodes[0].phase, Phase::AwaitingDelivery);
        assert!(before.check().is_ok());
        let findings = |snapshot: tl3_store::Snapshot| {
            snapshot.rows.into_iter()
                .filter(|row| matches!(row.table.as_str(), "maker_findings" | "findings"))
                .map(|row| (row.table, row.id, row.value)).collect::<Vec<_>>()
        };
        let before_findings = findings(controller.snapshot().unwrap());
        fixture.check(Some(conclusion));
        fixture.api.state.lock().unwrap().head_checks.iter_mut()
            .find(|check| check["id"] == 1100).unwrap()["app"]["slug"] = json!("github-actions");
        fixture.check_event(&settings);
        drain(&mut controller);
        let snapshot = controller.snapshot().unwrap();
        let row = snapshot.rows.iter().find(|row| row.table == "delivery_outcomes"
            && row.id.ends_with("/head-ci:1100")).expect("authenticated current-head CI receipt");
        let failure = &row.value["outcome"]["CiFailed"];
        assert_eq!(failure["infra"], json!(true));
        assert_eq!(failure["head"], json!(delivery.head_sha.unwrap()));
        assert!(failure["evidence"]["cause"]["Change"].is_object());
        let facts: Vec<_> = r1_kernel_facts(&settings).into_iter()
            .filter_map(|fact| fact.get("CiFailed").cloned()).collect();
        assert_eq!(facts, vec![json!({"infra": true})]);
        let after = controller.world().unwrap();
        assert!(after.check().is_ok());
        assert_eq!(after.nodes[0].credit, before.nodes[0].credit);
        assert_eq!(after, before, "waiting infrastructure failure must stutter the real kernel world");
        assert_eq!(findings(snapshot), before_findings, "infrastructure must not produce failed-round findings");
        assert!(fixture.direct.lock().unwrap().puts.is_empty());
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
    assert!(matches!(adapter.reconcile(&delivery).unwrap(), Reconciliation::Pending), "retired delivery cannot revive authority");
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
fn board_retains_recorded_direct_delivery_after_restart() {
    let mut database = support::Database::start();
    let settings = database.create("direct_board_history");
    let fixture = DirectFixture::new();
    let (mut controller, _) = fixture.start(&settings);
    drain(&mut controller);
    drop(controller);
    let mut controller = fixture.open(&settings);
    drain(&mut controller);
    let mut reader = Store::open(&settings).unwrap().reader().unwrap();
    let mut tx = reader.transaction().unwrap();
    let rows = tl3_controller::board::Rows::read(&mut tx).unwrap();
    assert!(rows.requests[0].direct_delivery);
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
    let cursor = store.translator_cursor("github-translator:delivery").unwrap();
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
    assert!(controller.snapshot().unwrap().rows.iter().any(|r|r.table == "delivery_outcomes" && r.value["outcome"]["CiFailed"]["infra"] == true));
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
fn annotation_crash_preserves_consumed_merge_attempts() {
    let mut database = support::Database::start();
    {
        let settings = database.create("direct_r3_current");
        let mut fixture = DirectFixture::new();
        fixture.check(Some("success"));
        fixture.direct.lock().unwrap().status = Some(503);
        let (controller, _) = fixture.start(&settings);
        assert_eq!(fixture.direct.lock().unwrap().puts.len(), 1);
        drop(controller);
        let path = direct_intent_path(&fixture);
        let intent: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(intent["action"], "Merge");
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
            "annotation must not erase a prior Merge attempt, recovery needs new ingress");
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
        let settings = database.create(if fake {"outcome_fake"} else {"outcome_production"});
        let fixture = DirectFixture::new(); fixture.check(None);
        let (mut controller, delivery) = fixture.start(&settings);
        drain(&mut controller);
        let observations = if fake {
            direct_merge::Fake::new(fixture.root.path()).execute(&delivery).unwrap().observations
        } else {
            fixture.api.state.lock().unwrap().merged(false);
            r1_adapter(&fixture, &settings).observations(&delivery).unwrap()
        };
        for key in ["first", "repeat"] {
            for observation in &observations { controller.observe(key, observation.clone()).unwrap(); }
            drain(&mut controller);
        }
        controller.observe("late-ci", Observation::CiFailed { node:NodeId(0), infra:false, checks:vec![] }).unwrap();
        drain(&mut controller);
        let facts = r1_kernel_facts(&settings);
        let rows = r1_delivery_rows(&mut controller);
        assert!(rows.is_empty(), "closed outcome paths retain no delivery or inbox rows: {rows:?}");
        results.push((facts, serde_json::to_value(controller.world().unwrap()).unwrap(), rows));
    }
    assert_eq!(results[0], results[1]);
}

/// Existing upstream names and ingress cursor keys remain usable after S2.
#[test]
fn delivery_upstream_uses_current_serialization() {
    assert_eq!(serde_json::to_value(Upstream::Delivery).unwrap(), "Delivery");
    assert_eq!(Upstream::Delivery.kind(), "Delivery");
    assert!(serde_json::from_value::<Upstream>(json!("Mergify")).is_err());
}

/// PRD #878 story 16: live CI failures retain the same failed round and feedback.
#[test]
fn fake_and_production_ci_outcomes_have_identical_findings() {
    use tl3_controller::{executor::Observation, findings::CiFailure};
    let mut database = support::Database::start();
    for infra in [false, true] {
        let mut results = Vec::new();
        for fake in [true, false] {
            let settings = database.create(&format!("ci_parity_{infra}_{fake}"));
            let fixture = DirectFixture::new(); fixture.check(None);
            let (mut controller, delivery) = fixture.start(&settings);
            drain(&mut controller);
            let checks = vec![CiFailure {name:"unit tests".into(), summary:"assertion failed".into(), url:None}];
            let observation = if fake { Observation::CiFailed {node:NodeId(0), infra, checks} }
            else { Observation::Outcome { node:NodeId(0), receipt:Some("owner/sandbox/3/100/head-ci:1001".into()),
                outcome:tl3_controller::outcome::Outcome::CiFailed {
                    head:delivery.head_sha.clone().unwrap(), infra, failed_tests:vec![],
                    evidence:tl3_controller::findings::CiEvidence {checks, ..Default::default()}
                } } };
            controller.observe("same-ci-outcome", observation).unwrap();
            assert!(controller.drive_once().unwrap());
            let facts: Vec<_> = r1_kernel_facts(&settings).into_iter().filter_map(|f|f.get("CiFailed").cloned()).collect();
            let findings: Vec<_> = controller.snapshot().unwrap().rows.into_iter()
                .filter(|row| row.table == "feedback")
                .map(|row| (row.table, row.id, row.value)).collect();
            assert_eq!(facts, vec![json!({"infra":infra})]);
            assert!(!controller.snapshot().unwrap().rows.iter().any(|r| r.table == "inbox"));
            results.push((facts, serde_json::to_value(controller.world().unwrap()).unwrap(), findings));
        }
        assert_eq!(results[0], results[1], "CI classification {infra} must share facts, world rows and findings");
    }
}

fn r1_delivery_rows(controller: &mut Controller) -> Vec<(String, String, Value)> {
    let mut rows: Vec<_> = controller.snapshot().unwrap().rows.into_iter()
        .filter(|row|matches!(row.table.as_str(), "delivery_outcomes" | "inbox"))
        .map(|row|(row.table,row.id,row.value)).collect();
    rows.sort_by(|a,b|(&a.0,&a.1).cmp(&(&b.0,&b.1)));
    rows
}

fn r1_adapter(fixture: &DirectFixture, settings: &tl3_config::Store) -> direct_merge::Adapter {
    direct_merge::Adapter::new(fixture.api.clients(), 0, &fixture.root.path().join("direct"), settings.clone()).unwrap()
}

fn r1_kernel_facts(settings: &tl3_config::Store) -> Vec<Value> {
    Store::open(settings).unwrap().events_after(tl3_store::Revision::ZERO, 4000).unwrap().into_iter()
        .filter_map(|event| event.receipt.result["kernel_event"]["Fact"]["fact"].as_object().cloned())
        .map(Value::Object).collect()
}

struct SameHeadRebase(adapters::worktrees::Fake);
impl Executor for SameHeadRebase {
    fn execute(&mut self, delivery: &Delivery) -> tl3_controller::Result<tl3_controller::executor::Completion> {
        if matches!(delivery.output, Out::Effect { kind: EffectKind::StartRebase, .. }) {
            return Ok(tl3_controller::executor::Completion::confirmed(vec![tl3_controller::executor::Observation::Rebased {
                node: delivery.node(), generation: delivery.generation, sha: delivery.head_sha.clone().unwrap(),
            }]));
        }
        self.0.execute(delivery)
    }
    fn reconcile(&mut self, delivery: &Delivery) -> tl3_controller::Result<Reconciliation> { self.0.reconcile(delivery) }
}

/// #901 r1/S1: a no-op rebase must not suppress the next conflict on the same head.
#[test]
fn r1_direct_conflict_repeats_after_same_head_rebase() {
    let mut database = support::Database::start();
    let settings = database.create("r1_repeat_conflict");
    let fixture = DirectFixture::new();
    fixture.check(None);
    let controller = fixture.open_adapters(&settings, fixture.adapter(&settings), Some(SameHeadRebase(adapters::worktrees::Fake::new(fixture.root.path()))));
    let (mut controller, delivery) = fixture.start_controller(&settings, controller);
    drain(&mut controller);
    fixture.api.state.lock().unwrap().pull["mergeable"] = json!(false);
    fixture.check(Some("success"));
    fixture.check_event(&settings);
    ingress::drain(&mut controller).unwrap();
    let mut rebased = false;
    let mut republished = false;
    for _ in 0..150 {
        let node = controller.world().unwrap().nodes[0];
        rebased |= node.phase == Phase::Rebasing;
        republished |= rebased && node.phase == Phase::Publishing;
        let conflicts = Store::open(&settings).unwrap().events_after(tl3_store::Revision::ZERO,4000).unwrap().iter()
            .filter(|event| event.receipt.result["kernel_event"]["Fact"]["fact"] == "Conflict").count();
        if conflicts == 2 {
            assert!(rebased && republished, "the second conflict must follow a same-head rebase and return to Publishing");
            assert_eq!(node.phase, Phase::Rebasing);
            assert_eq!(delivery.head_sha.as_deref(), fixture.api.state.lock().unwrap().pull["head"]["sha"].as_str());
            assert!(!controller.snapshot().unwrap().rows.iter().any(|row|row.table=="delivery_outcomes" && row.id.contains("/conflict:")));
            return;
        }
        if !controller.drive_once().unwrap() { break; }
    }
    panic!("same-head second conflict never reached the kernel: world {:?}, events {:?}", controller.world().unwrap(), Store::open(&settings).unwrap().events_after(tl3_store::Revision::ZERO,4000).unwrap().iter().map(|event|event.receipt.result["kernel_event"].clone()).collect::<Vec<_>>());
}

/// #901 r1/N2: legacy inbox corruption must be rejected through the input-error path.
#[test]
fn r1_out_of_range_legacy_inbox_returns_error_without_panicking() {
    use tl3_controller::executor::Observation;
    for merged in [false, true] {
        let mut database = support::Database::start();
        let settings = database.create(if merged {"r1_invalid_merged"} else {"r1_invalid_ci"});
        let fixture = DirectFixture::new();
        let mut controller = fixture.open(&settings);
        controller.observe("legacy-invalid", if merged { Observation::Merged {node:NodeId(99),sha:"a".repeat(40),checks_passed:true} }
            else {Observation::CiFailed {node:NodeId(99),infra:false,checks:vec![]}}).unwrap();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(||controller.drive_once()));
        assert!(result.is_ok(), "legacy input must not panic");
        let result = result.unwrap();
        assert!(result.is_err() || controller.snapshot().unwrap().rows.iter().any(|row|row.table=="incidents" && row.value["input_failure"].is_object()), "legacy input must report an error");
    }
}

/// #901 r1/N4: stale Direct head CI is journaled once without a kernel event.
#[test]
fn r1_stale_direct_ci_records_receipt_without_reapplying() {
    let mut database = support::Database::start();
    let settings = database.create("r1_stale_ci");
    let fixture = DirectFixture::new();
    fixture.check(None);
    let (mut controller, mut delivery) = fixture.start(&settings);
    let stale = "b".repeat(40);
    delivery.head_sha = Some(stale.clone());
    fixture.api.state.lock().unwrap().pull["head"]["sha"] = json!(stale);
    fixture.check(Some("failure"));
    let mut adapter = r1_adapter(&fixture,&settings);
    let observations = adapter.observations(&delivery).unwrap();
    assert_eq!(observations.len(),1);
    let before = r1_kernel_facts(&settings);
    controller.observe("stale-direct-ci",observations.into_iter().next().unwrap()).unwrap();
    controller.drive_once().unwrap();
    assert_eq!(r1_kernel_facts(&settings),before);
    let snapshot = controller.snapshot().unwrap();
    let row = snapshot.rows.iter().find(|row|row.table=="delivery_outcomes" && row.id.ends_with("/head-ci:1100")).expect("stale CI needs a receipt");
    assert_eq!(row.value["outcome"]["CiFailed"]["head"],stale);
    assert!(row.value["outcome"]["CiFailed"]["evidence"].get("cause").is_none(), "base did not attribute stale heads");
    assert!(adapter.observations(&delivery).unwrap().is_empty(), "a repeated look must not enqueue the stale CI again");
}

/// #901 r1/N4: Direct head CI retains the base Change attribution fingerprint.
#[test]
fn r1_current_direct_ci_retains_change_cause() {
    let mut database = support::Database::start();
    let settings = database.create("r1_ci_cause");
    let fixture = DirectFixture::new(); fixture.check(None);
    let (mut controller,delivery) = fixture.start(&settings);
    fixture.check(Some("failure"));
    let observations = r1_adapter(&fixture,&settings).observations(&delivery).unwrap();
    assert_eq!(observations.len(),1);
    controller.observe("current-direct-ci",observations.into_iter().next().unwrap()).unwrap();
    controller.drive_once().unwrap();
    let snapshot=controller.snapshot().unwrap();
    let row=snapshot.rows.iter().find(|row|row.table=="delivery_outcomes" && row.id.ends_with("/head-ci:1100")).unwrap();
    assert_eq!(row.value["outcome"]["CiFailed"]["evidence"]["cause"],json!({"Change":{"fingerprint":format!("head:{}:unknown",delivery.head_sha.unwrap())}}));
}

/// #901 r1/S3: pre-S2 push and head-CI receipts survive an upgrade and restart.
#[test]
fn current_direct_receipts_deduplicate_after_restart() {
    for ci in [false, true] {
        let mut database = support::Database::start();
        let settings = database.create(if ci {"direct_receipt_ci"} else {"direct_receipt_push"});
        let fixture = DirectFixture::new(); fixture.check(None);
        let (mut controller, delivery) = fixture.start(&settings);
        if ci { fixture.check(Some("failure")); } else { fixture.direct.lock().unwrap().moved = true; }
        let observations = r1_adapter(&fixture, &settings).observations(&delivery).unwrap();
        assert_eq!(observations.len(), 1);
        controller.observe("current-outcome", observations[0].clone()).unwrap();
        controller.drive_once().unwrap();
        let before = r1_kernel_facts(&settings);
        drop(controller);
        let mut controller = fixture.open(&settings);
        assert!(r1_adapter(&fixture, &settings).observations(&delivery).unwrap().is_empty());
        controller.look_key(&delivery.key).unwrap();
        assert_eq!(r1_kernel_facts(&settings), before);
    }
}

#[test]
fn changed_head_after_durable_intent_process_crash_never_replays_put() {
    use std::io::{BufRead, BufReader};
    use std::process::{Command, Stdio};
    const CHILD: &str = "TD_DIRECT_STALE_CRASH_CHILD";
    if let Some(path) = std::env::var_os(CHILD) {
        let input: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        let settings: tl3_config::Store = serde_json::from_value(input["settings"].clone()).unwrap();
        let root = std::path::PathBuf::from(input["root"].as_str().unwrap());
        let clients = BTreeMap::from([(3, client(input["uri"].as_str().unwrap()))]);
        let wakes = direct_merge::Wakes::default();
        let adapter = direct_merge::Adapter::new(clients.clone(), 0, &root.join("direct"), settings.clone()).unwrap()
            .with_wakes(wakes.clone())
            .with_write_boundary(|boundary| {
                if boundary == (direct_merge::WriteBoundary {write:direct_merge::Write::Intent, after:true}) {
                    println!("TD_INTENT_DURABLE");
                    std::io::stdout().flush().unwrap();
                    let mut line = String::new();
                    std::io::stdin().read_line(&mut line).unwrap();
                    panic!("parent must terminate the process before any destructor or PUT");
                }
            });
        let mut registry = adapters::simulation(&root).unwrap();
        registry.register(Upstream::Delivery, adapter);
        let mut controller = Controller::open(Store::open(&settings).unwrap(), registry, bootstrap()).unwrap();
        controller.register_github_translator("delivery", direct_merge::Translator::new(clients).unwrap().with_wakes(wakes));
        println!("TD_RECOVERY_READY");
        std::io::stdout().flush().unwrap();
        let mut line = String::new();
        std::io::stdin().read_line(&mut line).unwrap();
        drain(&mut controller);
        panic!("authenticated wake must reach the durable intent boundary");
    }
    let mut database = support::Database::start();
    let settings = database.create("direct_changed_head_process_crash");
    let mut fixture = DirectFixture::new();
    fixture.check(None);
    let mut controller = fixture.open(&settings);
    store_direct_check_event(&fixture, &settings, "pre-wait-pending");
    let pending = Store::open(&settings).unwrap().ingress_after(0).unwrap()
        .into_iter().find(|event| event.delivery_key == "pre-wait-pending").unwrap();
    let expected_waiting_through = u64::try_from(pending.sequence).unwrap();
    ingress::drain(&mut controller).unwrap();
    assert_eq!(Store::open(&settings).unwrap().translator_cursor("github-translator:delivery").unwrap(), pending.sequence);
    let (mut controller, delivery) = fixture.start_controller(&settings, controller);
    drain(&mut controller);
    assert_eq!(controller.world().unwrap().nodes[0].phase, Phase::AwaitingDelivery);
    drop(controller);
    let control = fixture.root.path().join("crash-input.json");
    std::fs::write(&control, serde_json::to_vec(&json!({
        "settings":{"host":settings.host,"port":settings.port,"database":settings.database,"user":settings.user,
            "password_file":settings.password_file,"connect_timeout_secs":settings.connect_timeout_secs},
        "root":fixture.root.path(), "uri":fixture.api.server.uri()
    })).unwrap()).unwrap();
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "changed_head_after_durable_intent_process_crash_never_replays_put", "--nocapture"])
        .env(CHILD, &control)
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::inherit()).spawn().unwrap();
    let mut output = BufReader::new(child.stdout.take().unwrap());
    fn reached(output: &mut impl BufRead, marker: &str) {
        loop {
            let mut line = String::new();
            assert_ne!(output.read_line(&mut line).unwrap(), 0, "child exited before {marker}");
            if line.trim() == marker { break; }
        }
    }
    reached(&mut output, "TD_RECOVERY_READY");
    fixture.check(Some("success"));
    store_direct_check_event(&fixture, &settings, "post-child-start-success");
    let authorizer = Store::open(&settings).unwrap().ingress_after(pending.sequence).unwrap()
        .into_iter().find(|event| event.delivery_key == "post-child-start-success").unwrap();
    let expected_merge_consumed = u64::try_from(authorizer.sequence).unwrap();
    assert!(expected_merge_consumed > expected_waiting_through);
    child.stdin.as_mut().unwrap().write_all(b"go\n").unwrap();
    reached(&mut output, "TD_INTENT_DURABLE");
    fixture.direct.lock().unwrap().moved = true;
    fixture.api.state.lock().unwrap().pull["head"]["sha"] = json!("b".repeat(40));
    child.kill().unwrap();
    let status = child.wait().unwrap();
    use std::os::unix::process::ExitStatusExt;
    assert_eq!(status.signal(), Some(9), "SIGKILL must prevent destructor unwinding at the reached intent boundary");
    let intent_path = direct_intent_path(&fixture);
    let intent_bytes = std::fs::read(&intent_path).unwrap();
    let intent: Value = serde_json::from_slice(&intent_bytes).unwrap();
    assert_eq!(intent["head"], json!(delivery.head_sha));
    assert!(matches!(intent["action"].as_str(), Some("Merge" | "Annotate")));
    // Even annotation must retain the consumed ingress fence from the accepted wait.
    let accepted: Vec<Value> = std::fs::read_dir(fixture.root.path().join("direct")).unwrap()
        .map(|entry|entry.unwrap().path()).filter(|path|path.extension().is_some_and(|ext|ext == "accepted"))
        .map(|path|serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()).collect();
    assert_eq!(accepted.len(), 1);
    assert_eq!(accepted[0]["key"], delivery.key);
    assert_eq!(accepted[0]["head"], json!(delivery.head_sha));
    assert_eq!(accepted[0]["generation"], delivery.generation);
    assert_eq!(accepted[0]["waiting_through"], json!(expected_waiting_through),
        "retained waiting watermark must equal the real pre-wait ingress fence");
    let expected_intent_consumed = match intent["action"].as_str().unwrap() {
        "Annotate" => expected_waiting_through,
        "Merge" => expected_merge_consumed,
        _ => unreachable!("the reached Intent belongs to annotation or merge"),
    };
    assert_eq!(intent["consumed_through"], json!(expected_intent_consumed),
        "retained intent must consume exactly its action's real ingress fence");
    let epoch: i64 = serde_json::from_slice(&std::fs::read(fixture.root.path().join("direct/epoch.fence")).unwrap()).unwrap();
    assert_eq!(epoch, delivery.epoch);
    assert!(fixture.direct.lock().unwrap().puts.is_empty());
    fixture.wakes = direct_merge::Wakes::default();
    let mut recovered = fixture.open(&settings);
    ingress::drain(&mut recovered).unwrap();
    for _ in 0..12 {
        if matches!(recovered.world().unwrap().nodes[0].phase, Phase::Checking | Phase::Ready(tl3_kernel::Resume::Checking)) { break; }
        recovered.drive_once().unwrap();
    }
    let world = recovered.world().unwrap();
    world.check().unwrap();
    assert!(matches!(world.nodes[0].phase, Phase::Checking | Phase::Ready(tl3_kernel::Resume::Checking)));
    assert!(world.nodes[0].rechecks > 0);
    let retained: Value = serde_json::from_slice(&std::fs::read(
        std::fs::read_dir(fixture.root.path().join("direct")).unwrap()
            .map(|entry|entry.unwrap().path()).find(|path|path.extension().is_some_and(|ext|ext == "accepted")).unwrap()
    ).unwrap()).unwrap();
    assert_eq!(retained["waiting_through"], json!(expected_waiting_through),
        "recovery must retain the exact precrash waiting watermark");
    let retained_intent: Value = serde_json::from_slice(&std::fs::read(&intent_path).unwrap()).unwrap();
    assert_eq!(retained_intent["consumed_through"], json!(expected_intent_consumed),
        "changed-head recovery cannot rewrite the consumed authorization");
    assert_eq!(fixture.direct.lock().unwrap().successes, 0);
    assert!(fixture.direct.lock().unwrap().puts.is_empty(), "changed-head crash recovery cannot replay a stale PUT");
    let before = r1_kernel_facts(&settings);
    recovered.look_key(&delivery.key).unwrap();
    assert_eq!(r1_kernel_facts(&settings), before, "retired delivery cannot revive the old attempt");
    assert!(fixture.direct.lock().unwrap().puts.is_empty());
}

fn open_read_incidents(controller: &mut Controller) -> Vec<(String, Value)> {
    controller.snapshot().unwrap().rows.into_iter()
        .filter(|row| row.table == "incidents" && row.value["open"] == true && row.id.ends_with(".read"))
        .map(|row| (row.id, row.value))
        .collect()
}
/// Silence looks only: no webhook is delivered, so nothing new authorizes a PUT.
fn look_until_read_incident(fixture: &DirectFixture, controller: &mut Controller) -> Vec<(String, Value)> {
    for _ in 0..10 {
        if !open_read_incidents(controller).is_empty() { break; }
        fixture.clock.0.set(fixture.clock.0.get() + Duration::from_secs(2));
        controller.look_due().unwrap();
        drain(controller);
    }
    open_read_incidents(controller)
}
/// Stops the real Direct adapter just before it writes the durable acceptance,
/// leaving the Publishing Enqueue handoff dispatched but never accepted.
fn unaccepted_handoff(fixture: &DirectFixture, settings: &tl3_config::Store) -> Delivery {
    let stop = direct_merge::WriteBoundary { write: direct_merge::Write::Acceptance, after: false };
    let adapter = fixture.adapter(settings).with_write_boundary(move |boundary| {
        if boundary == stop { panic!("fixture stops before durable acceptance"); }
    });
    let controller = fixture.open_adapter(settings, adapter);
    let stopped = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| fixture.start_controller(settings, controller)));
    assert!(stopped.is_err(), "the handoff must stop before durable acceptance");
    let snapshot = Store::open(settings).unwrap().load().unwrap();
    let effect = snapshot.pending_effects.iter()
        .find(|effect| matches!(serde_json::from_value::<Delivery>(effect.payload.clone()).unwrap().output,
            Out::Effect { kind: EffectKind::Enqueue, .. }))
        .expect("pending Enqueue handoff");
    let mut delivery: Delivery = serde_json::from_value(effect.payload.clone()).unwrap();
    delivery.epoch = effect.epoch.get();
    delivery.pull_request = Some(100);
    assert!(accepted_records(&fixture.root.path().join("direct")).is_empty());
    delivery
}
fn fresh_adapter(fixture: &DirectFixture, settings: &tl3_config::Store) -> direct_merge::Adapter {
    direct_merge::Adapter::new(fixture.api.clients(), 0, &fixture.root.path().join("direct"), settings.clone()).unwrap()
}

/// Review MF1(a): a read that keeps failing after durable acceptance is a failed
/// look. Q7 read accounting counts it and raises the `.read` incident; it is
/// never turned into an uncounted acceptance.
#[test]
fn persistent_read_failure_after_acceptance_raises_read_incident() {
    let mut database = support::Database::start();
    let settings = database.create("direct_accepted_read_outage");
    let fixture = DirectFixture::new();
    fixture.direct.lock().unwrap().fail_reads_after_acceptance = usize::MAX;
    let (mut controller, _) = fixture.start(&settings);
    drain(&mut controller);
    assert_eq!(accepted_records(&fixture.root.path().join("direct")).len(), 1, "the failure must follow durable acceptance");
    let incidents = look_until_read_incident(&fixture, &mut controller);
    assert_eq!(incidents.len(), 1, "a persistent read failure after acceptance must end in a read incident: world={:?}, rows={:?}",
        controller.world().unwrap(),
        controller.snapshot().unwrap().rows.iter().filter(|r| matches!(r.table.as_str(), "allowances" | "incidents")).collect::<Vec<_>>());
    let (_, incident) = &incidents[0];
    assert_eq!(incident["signature"]["upstream"], "Delivery");
    assert!(fixture.direct.lock().unwrap().failed_reads >= 2);
    let node = controller.world().unwrap().nodes[0];
    assert_eq!(node.phase, Phase::Publishing, "a failed read is not an acknowledgement of the handoff");
    assert_eq!(node.effect.map(|effect| effect.kind), Some(EffectKind::Enqueue));
    assert!(fixture.direct.lock().unwrap().puts.is_empty());
    assert!(controller.world().unwrap().valid());
}

/// Review MF1(b): one transient 5xx on the first attempt keeps the initial merge
/// authorization. Once the read recovers the PR merges exactly once, with no
/// new webhook.
#[test]
fn transient_read_failure_on_first_attempt_still_merges_once() {
    let mut database = support::Database::start();
    let settings = database.create("direct_accepted_transient_read");
    let fixture = DirectFixture::new();
    fixture.direct.lock().unwrap().fail_reads_after_acceptance = 1;
    let (mut controller, delivery) = fixture.start(&settings);
    drain(&mut controller);
    for _ in 0..5 {
        if controller.world().unwrap().nodes[0].phase == Phase::Closed(Outcome::Merged) { break; }
        fixture.clock.0.set(fixture.clock.0.get() + Duration::from_secs(2));
        controller.look_due().unwrap();
        drain(&mut controller);
    }
    let direct = fixture.direct.lock().unwrap();
    assert_eq!(direct.failed_reads, 1, "the first attempt must meet exactly one transient read failure");
    assert_eq!(direct.puts.len(), 1, "the recovered read must merge exactly once without a new webhook: world={:?}", controller.world().unwrap());
    assert_eq!(direct.puts[0]["sha"], json!(delivery.head_sha));
    assert_eq!(direct.successes, 1);
    drop(direct);
    assert_eq!(controller.world().unwrap().nodes[0].phase, Phase::Closed(Outcome::Merged));
    assert!(open_read_incidents(&mut controller).is_empty());
}

/// Review MF2(c): a fatal stored-state error raised after durable acceptance
/// propagates. It is never converted into an acceptance and never consumes the
/// merge authorization.
#[test]
fn fatal_stored_error_after_acceptance_propagates() {
    let mut database = support::Database::start();
    let settings = database.create("direct_accepted_fatal_store");
    let fixture = DirectFixture::new();
    let delivery = unaccepted_handoff(&fixture, &settings);
    let corrupt = settings.clone();
    let after_acceptance = direct_merge::WriteBoundary { write: direct_merge::Write::Acceptance, after: true };
    let mut adapter = fresh_adapter(&fixture, &settings).with_write_boundary(move |boundary| {
        if boundary != after_acceptance { return; }
        let mut store = Store::open(&corrupt).unwrap();
        let snapshot = store.load().unwrap();
        let mut commit = tl3_store::Commit::new("fixture-corrupt-world", json!({"fixture": "corrupt"}), &snapshot);
        commit.rows.push(tl3_store::RowChange::Put { table: "nodes".into(), id: "0".into(), value: json!("corrupt") });
        store.commit(&commit).unwrap();
    });
    let result = adapter.execute(&delivery);
    let accepted = accepted_records(&fixture.root.path().join("direct"));
    assert_eq!(accepted.len(), 1, "the fatal error must follow durable acceptance");
    match result {
        Err(tl3_controller::Error::StoredJson(_)) => {}
        Err(error) => panic!("a corrupt stored world must stay fatal, got {error}"),
        Ok(completion) => panic!("a fatal stored error became an acceptance: {completion:?}"),
    }
    assert!(accepted[0]["waiting_through"].is_null(), "a fatal error must not consume the merge authorization");
    assert!(fixture.direct.lock().unwrap().puts.is_empty());
}

/// Review MF1/MF2(d): a permanent refusal after durable acceptance keeps the
/// existing conversion to DeliveryAccepted with its diagnostic, from both the
/// attempt and the look.
#[test]
fn permanent_refusal_after_acceptance_is_an_accepted_diagnostic() {
    let mut database = support::Database::start();
    let settings = database.create("direct_accepted_permanent");
    let fixture = DirectFixture::new();
    let delivery = unaccepted_handoff(&fixture, &settings);
    fixture.direct.lock().unwrap().no_merge_method = true;
    let completion = fresh_adapter(&fixture, &settings).execute(&delivery).unwrap();
    assert_eq!(completion.ack, tl3_kernel::Ack::DeliveryAccepted);
    assert_eq!(completion.failure_class.as_deref(), Some("direct_merge_method"));
    assert!(completion.diagnostic.is_some(), "a permanent refusal keeps its diagnostic");
    assert_eq!(accepted_records(&fixture.root.path().join("direct")).len(), 1);
    let tl3_controller::executor::Look::Found(found) = fresh_adapter(&fixture, &settings).look(&delivery).unwrap() else {
        panic!("a permanent refusal in the look must stay an accepted diagnostic");
    };
    assert_eq!(found.ack, tl3_kernel::Ack::DeliveryAccepted);
    assert_eq!(found.failure_class.as_deref(), Some("direct_merge_method"));
    assert!(found.diagnostic.is_some());
    assert!(fixture.direct.lock().unwrap().puts.is_empty());
}

/// Drives a fresh request to its Publishing Enqueue handoff and stops while that
/// effect is still Intended, so Direct has not yet attempted or accepted it.
fn issued_handoff(fixture: &DirectFixture, settings: &tl3_config::Store) -> (Controller, String) {
    let mut controller = fixture.open(settings);
    controller.intake(LabelledIssue { delivery:"direct-label".into(), owner:"owner".into(), repo:"sandbox".into(), installation:3, number:12, title:"change".into(), actor:42, label:"tl3".into(), labels:vec![] }).unwrap();
    for _ in 0..100 {
        let node = controller.world().unwrap().nodes[0];
        if node.phase == Phase::Publishing && node.effect.is_some_and(|effect| effect.kind == EffectKind::Enqueue) {
            let effect = controller.snapshot().unwrap().pending_effects.into_iter()
                .find(|effect| matches!(serde_json::from_value::<Delivery>(effect.payload.clone()).unwrap().output,
                    Out::Effect { kind: EffectKind::Enqueue, .. }))
                .expect("pending Enqueue handoff");
            assert_eq!(effect.state, tl3_store::EffectState::Intended, "the handoff must not be dispatched yet");
            return (controller, effect.key);
        }
        assert!(controller.drive_once().unwrap(), "the fixture must reach the Publishing Enqueue handoff");
    }
    panic!("the fixture did not reach the Publishing Enqueue handoff");
}
/// Silence looks only, for a fixed number of Q7 counting periods.
fn quiet_looks(fixture: &DirectFixture, controller: &mut Controller) {
    for _ in 0..6 {
        fixture.clock.0.set(fixture.clock.0.get() + Duration::from_secs(2));
        controller.look_due().unwrap();
        drain(controller);
    }
}
fn never_accepted(fixture: &DirectFixture, controller: &mut Controller, key: &str) {
    let node = controller.world().unwrap().nodes[0];
    assert_eq!(node.phase, Phase::Publishing, "the handoff is never accepted");
    assert_eq!(node.effect.map(|effect| effect.kind), Some(EffectKind::Enqueue));
    assert!(controller.effect(key).unwrap().unwrap().state.is_pending(), "nothing acknowledges the handoff");
    assert!(accepted_records(&fixture.root.path().join("direct")).is_empty(), "Direct must never accept the handoff");
    assert!(fixture.direct.lock().unwrap().puts.is_empty());
    assert!(controller.world().unwrap().valid());
}
fn allowance_healthy(controller: &mut Controller, key: &str) -> bool {
    controller.snapshot().unwrap().rows.iter()
        .find(|row| row.table == "allowances" && row.id == key)
        .is_some_and(|row| row.value["healthy"] == true && row.value["read_failures"] == 0)
}

/// Nit-4 ruling (i), state-table "Looks and stalls" (docs/state-table.md:861):
/// a never-acknowledged enqueue whose fence refusal no deliberate hold explains
/// ends in the `.read` incident through Q7 read accounting.
/// No valid kernel transition reaches this refusal in Publishing, so the fixture
/// injects an adversarial stored state at the fence boundary: after the kernel
/// issues the Enqueue, the stored handoff payload's head_sha is rewritten (trigger
/// disabled, as in input_error_followups.rs) so it disagrees with the approved
/// commit. The fixture PR reports that same head. The world stays valid and the
/// node is not paused or held.
#[test]
fn unexplained_never_accepted_handoff_ends_in_read_incident() {
    let mut database = support::Database::start();
    let settings = database.create("direct_never_accepted_unready");
    let fixture = DirectFixture::new();
    let (mut controller, key) = issued_handoff(&fixture, &settings);
    postgres::Config::new().host(&settings.host).port(settings.port).user(&settings.user).dbname(&settings.database)
        .password("tl3-disposable-test-password").connect(postgres::NoTls).unwrap()
        .batch_execute(&format!(
            "ALTER TABLE tl3_effects DISABLE TRIGGER tl3_effect_guard; \
             UPDATE tl3_effects SET payload = jsonb_set(payload, '{{head_sha}}', '\"{}\"') WHERE effect_key = '{key}'; \
             ALTER TABLE tl3_effects ENABLE TRIGGER tl3_effect_guard;",
            "f".repeat(40)))
        .unwrap();
    // GitHub reports the same head, so only the kernel's approved commit disagrees.
    fixture.api.state.lock().unwrap().pull["head"]["sha"] = json!("f".repeat(40));
    drain(&mut controller);
    let node = controller.world().unwrap().nodes[0];
    assert!(!node.paused && !node.waiting_incident, "no deliberate hold explains the refusal");
    assert!(controller.snapshot().unwrap().rows.iter().all(|row| row.table != "incidents" || row.value["open"] != true));
    never_accepted(&fixture, &mut controller, &key);
    let incidents = look_until_read_incident(&fixture, &mut controller);
    assert_eq!(incidents.len(), 1, "an unexplained never-acknowledged enqueue must end in a read incident: rows={:?}",
        controller.snapshot().unwrap().rows.iter().filter(|r| matches!(r.table.as_str(), "allowances" | "incidents")).collect::<Vec<_>>());
    let (id, incident) = incidents[0].clone();
    assert_eq!(incident["signature"]["upstream"], "Delivery");
    assert_eq!(incident["effect"], json!(key));
    never_accepted(&fixture, &mut controller, &key);
    // Re-review MF1: the handoff's own .read incident is not a deliberate hold. It
    // stays open while nothing is repaired, with no close/reopen flap.
    for _ in 0..6 {
        fixture.clock.0.set(fixture.clock.0.get() + Duration::from_secs(2));
        controller.look_due().unwrap();
        drain(&mut controller);
        let rows: Vec<_> = controller.snapshot().unwrap().rows.into_iter()
            .filter(|row| row.table == "incidents").map(|row| (row.id, row.value)).collect();
        assert_eq!(rows.len(), 1, "no second incident may be raised: {rows:?}");
        assert_eq!(rows[0].0, id);
        assert_eq!(rows[0].1["open"], json!(true), "the unrepaired handoff's incident must never close: {rows:?}");
    }
    never_accepted(&fixture, &mut controller, &key);
}

/// Nit-4 ruling (ii): a paused, never-accepted Publishing handoff is a deliberate
/// hold. It raises no incident and its looks stay healthy.
#[test]
fn paused_never_accepted_handoff_stays_healthy() {
    let mut database = support::Database::start();
    let settings = database.create("direct_never_accepted_paused");
    let fixture = DirectFixture::new();
    let (mut controller, key) = issued_handoff(&fixture, &settings);
    controller.observe("fixture-pause", tl3_controller::executor::Observation::Steering {
        node: NodeId(0), command: tl3_kernel::Command::Pause }).unwrap();
    drain(&mut controller);
    assert!(controller.world().unwrap().nodes[0].paused);
    never_accepted(&fixture, &mut controller, &key);
    quiet_looks(&fixture, &mut controller);
    let incidents: Vec<_> = controller.snapshot().unwrap().rows.into_iter()
        .filter(|row| row.table == "incidents" && row.value["open"] == true).collect();
    assert!(incidents.is_empty(), "a paused handoff must not page anyone: {incidents:?}");
    assert!(allowance_healthy(&mut controller, &key), "a paused handoff stays a healthy look");
    assert!(controller.world().unwrap().nodes[0].paused);
    never_accepted(&fixture, &mut controller, &key);
}

/// Nit-4 ruling (iii): an open incident on the node is a deliberate hold. Here a
/// real rejected input (a raw command id at u64::MAX, as in
/// input_error_followups.rs) opens a non-blocking incident on node 0 after the
/// Enqueue is issued. The never-accepted handoff raises no `.read` incident and
/// its looks stay healthy.
#[test]
fn open_node_incident_never_accepted_handoff_stays_healthy() {
    let mut database = support::Database::start();
    let settings = database.create("direct_never_accepted_incident");
    let fixture = DirectFixture::new();
    let (mut controller, key) = issued_handoff(&fixture, &settings);
    let input: tl3_controller::cli::Input = serde_json::from_value(json!({
        "Observation": {"key": "command-id-jump", "observation": {"Kernel": {"Command": {
            "node": 0, "id": u64::MAX, "command": "Pause"}}}}
    })).unwrap();
    ingress::apply(&mut controller, input).unwrap();
    drain(&mut controller);
    let open: Vec<_> = controller.snapshot().unwrap().rows.into_iter()
        .filter(|row| row.table == "incidents" && row.value["open"] == true).collect();
    assert_eq!(open.len(), 1, "the rejected input must open one incident: {open:?}");
    assert_eq!(open[0].value["node"], json!(0), "the incident must be on the handoff's node");
    let node = controller.world().unwrap().nodes[0];
    assert!(!node.paused && !node.waiting_incident, "only the open incident holds the node");
    never_accepted(&fixture, &mut controller, &key);
    quiet_looks(&fixture, &mut controller);
    assert!(open_read_incidents(&mut controller).is_empty(), "an open node incident must not add a read incident");
    let still_open: Vec<_> = controller.snapshot().unwrap().rows.into_iter()
        .filter(|row| row.table == "incidents" && row.value["open"] == true).map(|row| row.id).collect();
    assert_eq!(still_open, vec![open[0].id.clone()]);
    assert!(allowance_healthy(&mut controller, &key), "a held handoff stays a healthy look");
    never_accepted(&fixture, &mut controller, &key);
}

/// Program-lead ruling on the post-acceptance Permanent nit: a permanent refusal
/// first seen by a look on a settled handoff (AwaitingDelivery) must reach a
/// person. Here a restart look (in-memory wakes lost, `look_now` before any
/// drive) meets a repository that allows no merge method. Exactly one incident
/// is raised through the existing accepted-delivery refusal path and carries the
/// publication diagnostic.
#[test]
fn settled_permanent_refusal_seen_by_a_look_raises_one_incident() {
    let mut database = support::Database::start();
    let settings = database.create("direct_settled_permanent_look");
    let mut fixture = DirectFixture::new();
    fixture.check(None);
    let (mut controller, delivery) = fixture.start(&settings);
    drain(&mut controller);
    assert_eq!(controller.world().unwrap().nodes[0].phase, Phase::AwaitingDelivery);
    assert!(controller.snapshot().unwrap().rows.iter().all(|row| row.table != "incidents"));
    drop(controller);
    fixture.wakes = direct_merge::Wakes::default();
    let mut controller = fixture.open(&settings);
    fixture.direct.lock().unwrap().no_merge_method = true;
    controller.look_now().unwrap();
    drain(&mut controller);
    quiet_looks(&fixture, &mut controller);
    let open: Vec<_> = controller.snapshot().unwrap().rows.into_iter()
        .filter(|row| row.table == "incidents" && row.value["open"] == true).collect();
    assert_eq!(open.len(), 1, "a settled permanent refusal must raise exactly one incident: rows={:?}",
        controller.snapshot().unwrap().rows.iter().filter(|r| matches!(r.table.as_str(), "allowances" | "incidents")).collect::<Vec<_>>());
    let incident = &open[0].value;
    assert_eq!(incident["effect"], json!(delivery.key));
    assert_eq!(incident["signature"]["upstream"], "Delivery");
    let summary = incident["summary"].as_str().unwrap();
    assert!(summary.contains("Publication diagnostic:") && summary.contains(&delivery.key),
        "the incident must carry the diagnostic: {summary}");
    assert!(controller.snapshot().unwrap().rows.iter().any(|row| row.table == "publication_failures" && row.id == delivery.key));
    assert_eq!(controller.world().unwrap().nodes[0].phase, Phase::AwaitingDelivery);
    assert!(fixture.direct.lock().unwrap().puts.is_empty());
}

fn m4_record(controller: &mut Controller) -> Value {
    controller.snapshot().unwrap().rows.into_iter()
        .find(|row| row.table == "merge_records")
        .expect("completed merge must retain its landed commit and separate assurance facts").value
}
fn m4_notices(controller: &mut Controller) -> Vec<Value> {
    controller.snapshot().unwrap().rows.into_iter()
        .filter(|row| row.table == "incidents" && row.value["signature"]["class"] == "merge_audit")
        .map(|row| { assert_eq!(row.value["blocking"], false); row.value }).collect()
}
#[test]
fn m4_squash_records_landed_commit_and_tree() {
    let mut database = support::Database::start();
    let settings = database.create("m4_squash");
    let fixture = DirectFixture::new();
    fixture.check(Some("success"));
    let (mut controller, delivery) = fixture.start(&settings);
    drain(&mut controller);
    let record = m4_record(&mut controller);
    assert_ne!(record["landed"]["commit"], json!(delivery.head_sha));
    assert_eq!(record["landed"]["commit"], "dddddddddddddddddddddddddddddddddddddddd");
    assert_eq!(record["landed"]["tree"], "1".repeat(40));
    assert_eq!(record["landed"]["head_tree"], "1".repeat(40));
    assert_eq!(record["landed"]["parents"][0], record["landed"]["base_before_merge"]);
    assert_eq!(record["landed"]["tree_matches"], true);
    assert_eq!(record["landed"]["parent_matches"], true);
}
#[test]
fn m4_different_landed_tree_raises_notice() {
    let mut database = support::Database::start();
    let settings = database.create("m4_tree");
    let fixture = DirectFixture::new();
    fixture.check(Some("success"));
    fixture.direct.lock().unwrap().different_tree = true;
    let (mut controller, _) = fixture.start(&settings);
    drain(&mut controller);
    let notices = m4_notices(&mut controller);
    assert_eq!(notices.len(), 1, "landed tree differs from the checked head must raise a notice");
    let summary = notices[0]["summary"].as_str().unwrap();
    assert!(summary.contains("landed tree differs from the checked head"));
    assert!(summary.contains(&"1".repeat(40)) && summary.contains(&"2".repeat(40)));
    assert_eq!(m4_record(&mut controller)["landed"]["tree_matches"], false);
    assert!(matches!(controller.world().unwrap().nodes[0].phase, Phase::Closed(Outcome::Merged)));
}
#[test]
fn m4_zero_required_checks_records_count_and_notice() {
    let mut database = support::Database::start();
    let settings = database.create("m4_zero");
    let fixture = DirectFixture::new();
    let (mut controller, _) = fixture.start(&settings);
    drain(&mut controller);
    let notices = m4_notices(&mut controller);
    assert_eq!(notices.len(), 1, "merged with no required GitHub checks must raise a notice");
    assert!(notices[0]["summary"].as_str().unwrap().contains("merged with no required GitHub checks"));
    assert_eq!(m4_record(&mut controller)["required_checks"], json!({"passed":true,"count":0}));
}
#[test]
fn m4_external_merge_reads_failing_required_checks() {
    let mut database = support::Database::start();
    let settings = database.create("m4_external");
    let fixture = DirectFixture::new();
    fixture.check(None);
    let (mut controller, _) = fixture.start(&settings);
    drain(&mut controller);
    fixture.check(Some("failure"));
    fixture.api.state.lock().unwrap().merged(false);
    let mut body = webhook_fixture();
    body["action"] = json!("closed");
    body["repository"]["owner"]["login"] = json!("owner");
    body["repository"]["name"] = json!("sandbox");
    body["installation"] = json!({"id":3});
    body["pull_request"] = fixture.api.state.lock().unwrap().pull.clone();
    deliver(&settings, &fixture.api, "pull_request", &body);
    drain(&mut controller);
    let record = m4_record(&mut controller);
    assert_eq!(record["required_checks"], json!({"passed":false,"count":1}));
    assert_eq!(record["tl3_checks_passed"], true);
    assert_eq!(record["review_approved"], true);
    assert!(m4_notices(&mut controller).iter().any(|n| n["summary"].as_str().unwrap().contains("required GitHub checks not passed")));
    assert!(matches!(controller.world().unwrap().nodes[0].phase, Phase::Closed(Outcome::Merged)));
}
#[test]
fn m4_ordinary_merge_records_three_facts_without_notice() {
    let mut database = support::Database::start();
    let settings = database.create("m4_ordinary");
    let fixture = DirectFixture::new();
    fixture.check(Some("success"));
    let (mut controller, delivery) = fixture.start(&settings);
    drain(&mut controller);
    let record = m4_record(&mut controller);
    assert_eq!(record["head"], json!(delivery.head_sha));
    assert_eq!(record["tl3_checks_passed"], true);
    assert_eq!(record["review_approved"], true);
    assert_eq!(record["required_checks"], json!({"passed":true,"count":1}));
    assert!(m4_notices(&mut controller).is_empty());
    assert!(!controller.snapshot().unwrap().rows.iter().any(|row| row.table == "incidents"));
    let merged_facts: Vec<_> = r1_kernel_facts(&settings).into_iter().filter_map(|fact| fact.get("PrMerged").cloned()).collect();
    assert_eq!(merged_facts.len(), 1);
    assert_eq!(merged_facts[0]["checks_passed"], true);
    let world = controller.world().unwrap();
    assert_eq!(world.nodes[0].merged, world.nodes[0].approved);
    assert!(matches!(world.nodes[0].phase, Phase::Closed(Outcome::Merged)));
}

#[test]
fn m4_different_first_parent_is_a_nonblocking_notice() {
    let mut database = support::Database::start();
    let settings = database.create("m4_parent");
    let fixture = DirectFixture::new();
    fixture.check(Some("success"));
    fixture.direct.lock().unwrap().different_parent = true;
    let (mut controller, _) = fixture.start(&settings);
    drain(&mut controller);
    let record = m4_record(&mut controller);
    assert_eq!(record["landed"]["tree_matches"], true);
    assert_eq!(record["landed"]["parent_matches"], false);
    let notices = m4_notices(&mut controller);
    assert_eq!(notices.len(), 1);
    let summary = notices[0]["summary"].as_str().unwrap();
    assert!(summary.contains(&"3".repeat(40)));
    assert!(summary.contains(record["landed"]["base_before_merge"].as_str().unwrap()));
    assert_eq!(notices[0]["open"], false);
    assert!(matches!(controller.world().unwrap().nodes[0].phase, Phase::Closed(Outcome::Merged)));
}
#[test]
fn m4_lost_reply_retains_landed_commit_and_saved_base() {
    let mut database = support::Database::start();
    let settings = database.create("m4_lost_reply");
    let fixture = DirectFixture::new();
    fixture.check(Some("success"));
    fixture.direct.lock().unwrap().lost_reply = true;
    let (mut controller, _) = fixture.start(&settings);
    drain(&mut controller);
    let record = m4_record(&mut controller);
    assert_eq!(record["landed"]["commit"], "dddddddddddddddddddddddddddddddddddddddd");
    assert_eq!(record["landed"]["parent_matches"], true);
    assert!(record["landed"]["base_before_merge"].is_string());
    assert_eq!(record["landed"]["merge_base"], record["landed"]["base_before_merge"]);
    assert!(m4_notices(&mut controller).is_empty());
    assert_eq!(fixture.direct.lock().unwrap().puts.len(), 1);
}

struct M4BaseChecks {
    inner: adapters::flux_checks::Fake,
    base: String,
    moved_base: Option<String>,
    api: Arc<Mutex<State>>,
}
impl Executor for M4BaseChecks {
    fn execute(&mut self, delivery: &Delivery) -> tl3_controller::Result<tl3_controller::executor::Completion> {
        let mut completion = self.inner.execute(delivery)?;
        if matches!(delivery.output, Out::Effect { kind: EffectKind::SubmitChecks, .. }) {
            completion.observations = vec![tl3_controller::executor::Observation::Checks {
                node: delivery.node(), generation: delivery.generation,
                sha: delivery.head_sha.clone().unwrap(), changed_paths: vec![], failed_tests: vec![],
                report: true, pass: true, results: vec![], base_sha: self.base.clone(),
                action_key: format!("base-check:{}", delivery.key),
            }];
            if let Some(base) = &self.moved_base {
                self.api.lock().unwrap().live_base = Some(base.clone());
            }
        }
        Ok(completion)
    }
    fn reconcile(&mut self, delivery: &Delivery) -> tl3_controller::Result<Reconciliation> {
        self.inner.reconcile(delivery)
    }
}
fn m4_base_record_case(moved: bool) {
    let mut database = support::Database::start();
    let settings = database.create(if moved {"m4_base_moved"} else {"m4_base_ordinary"});
    let fixture = DirectFixture::new();
    fixture.check(Some("success"));
    let checked_base = "4".repeat(40);
    let merge_base = if moved {"5".repeat(40)} else {checked_base.clone()};
    fixture.api.state.lock().unwrap().pull["base"]["sha"] = json!(checked_base);
    let mut registry = adapters::simulation(fixture.root.path()).unwrap();
    registry.register(Upstream::Delivery, fixture.adapter(&settings));
    registry.register(Upstream::PrismaBuild, M4BaseChecks {
        inner: adapters::flux_checks::Fake::new(fixture.root.path()), base: checked_base.clone(),
        moved_base: moved.then(|| merge_base.clone()), api: fixture.api.state.clone(),
    });
    let controller = Controller::open_with_clock(Store::open(&settings).unwrap(), registry, bootstrap(), fixture.clock.clone()).unwrap();
    let (mut controller, _) = fixture.start_controller(&settings, controller);
    let before = controller.snapshot().unwrap();
    let report = before.rows.iter().find(|row| row.table == "check_reports").expect("passing check report is persisted before merge closure");
    assert_eq!(report.value["base_sha"], checked_base);
    assert_eq!(report.value["pass"], true);
    drain(&mut controller);
    let record = m4_record(&mut controller);
    assert_eq!(record["landed"]["checked_base"], checked_base, "checked base must come from the passing check report");
    assert_eq!(record["landed"]["merge_base"], merge_base, "merge base must come from the live branch tip despite stale PR metadata");
    assert_eq!(record["landed"]["base_moved"], moved);
    assert_eq!(record["landed"]["pr_base_sha"], checked_base);
    assert_eq!(record["landed"]["checked_parent_matches"], !moved);
    assert!(matches!(controller.world().unwrap().nodes[0].phase, Phase::Closed(Outcome::Merged)));
    assert_eq!(fixture.direct.lock().unwrap().puts.len(), 1, "base audit fields must not change the merge decision");
    assert!(m4_notices(&mut controller).is_empty(), "base movement alone is record-only in M-4");
}
#[test]
fn m4_base_moved_records_checked_and_merge_bases() { m4_base_record_case(true); }
#[test]
fn m4_base_ordinary_records_no_movement() { m4_base_record_case(false); }

fn m4_live_external_parent_case(parent_matches: bool) {
    let mut database = support::Database::start();
    let settings = database.create(if parent_matches {"m4_live_external_match"} else {"m4_live_external_mismatch"});
    let fixture = DirectFixture::new();
    fixture.check(None);
    let checked_base = "4".repeat(40);
    let live_base = "6".repeat(40);
    fixture.api.state.lock().unwrap().pull["base"]["sha"] = json!(checked_base);
    let mut registry = adapters::simulation(fixture.root.path()).unwrap();
    registry.register(Upstream::Delivery, fixture.adapter(&settings));
    registry.register(Upstream::PrismaBuild, M4BaseChecks {
        inner: adapters::flux_checks::Fake::new(fixture.root.path()), base: checked_base.clone(),
        moved_base: None, api: fixture.api.state.clone(),
    });
    let mut controller = Controller::open_with_clock(Store::open(&settings).unwrap(), registry, bootstrap(), fixture.clock.clone()).unwrap();
    controller.register_github_translator("delivery", direct_merge::Translator::new(fixture.api.clients()).unwrap().with_wakes(fixture.wakes.clone()));
    let (mut controller, _) = fixture.start_controller(&settings, controller);
    drain(&mut controller);
    {
        let mut state = fixture.api.state.lock().unwrap();
        state.merged(false);
        state.live_base = Some(live_base.clone());
    }
    fixture.direct.lock().unwrap().different_parent = !parent_matches;
    fixture.check(Some("success"));
    let mut body = webhook_fixture();
    body["action"] = json!("closed");
    body["repository"]["owner"]["login"] = json!("owner");
    body["repository"]["name"] = json!("sandbox");
    body["installation"] = json!({"id":3});
    body["pull_request"] = fixture.api.state.lock().unwrap().pull.clone();
    deliver(&settings, &fixture.api, "pull_request", &body);
    drain(&mut controller);
    let record = m4_record(&mut controller);
    assert_eq!(record["landed"]["merge_base"], live_base, "outside merge must record the live tip at observation, not the stale PR base");
    assert_eq!(record["landed"]["pr_base_sha"], checked_base);
    assert_eq!(record["landed"]["checked_base"], checked_base);
    assert_eq!(record["landed"]["base_moved"], true);
    assert_eq!(record["landed"]["checked_parent_matches"], parent_matches, "the landed parent must be compared with the checked base, not the later live tip");
    assert!(fixture.direct.lock().unwrap().puts.is_empty());
    assert!(m4_notices(&mut controller).is_empty());
    assert!(matches!(controller.world().unwrap().nodes[0].phase, Phase::Closed(Outcome::Merged)));
}
#[test]
fn m4_live_external_parent_matches_checked_base() { m4_live_external_parent_case(true); }
#[test]
fn m4_live_external_parent_differs_from_checked_base() { m4_live_external_parent_case(false); }

fn m4_review_closed_event(fixture: &DirectFixture) -> Value {
    let mut body = webhook_fixture();
    body["action"] = json!("closed");
    body["repository"]["owner"]["login"] = json!("owner");
    body["repository"]["name"] = json!("sandbox");
    body["installation"] = json!({"id":3});
    body["pull_request"] = fixture.api.state.lock().unwrap().pull.clone();
    body
}
fn m4_review_one_merge(controller: &mut Controller, settings: &tl3_config::Store) {
    assert!(matches!(controller.world().unwrap().nodes[0].phase, Phase::Closed(Outcome::Merged)), "GitHub already merged; evidence reads must not keep TL3 open");
    assert_eq!(r1_kernel_facts(settings).iter().filter(|f| f.get("PrMerged").is_some()).count(), 1, "record the unchanged merged fact exactly once");
}
fn m4_review_evidence_failure(webhook: bool, status: u16) {
    let mut database = support::Database::start();
    let settings = database.create(&format!("m4_review_{webhook}_{status}"));
    let mut fixture = DirectFixture::new();
    fixture.check(if webhook {None} else {Some("success")});
    fixture.direct.lock().unwrap().landed_status = Some(status);
    let (mut controller, _) = fixture.start(&settings);
    drain(&mut controller);
    if webhook {
        fixture.check(Some("success"));
        fixture.api.state.lock().unwrap().merged(false);
        deliver(&settings, &fixture.api, "pull_request", &m4_review_closed_event(&fixture));
        drain(&mut controller);
    }
    m4_review_one_merge(&mut controller, &settings);
    let record = m4_record(&mut controller);
    assert!(record["landed"]["tree"].is_null());
    assert!(record["landed"]["parents"].is_null());
    assert!(record["landed"]["tree_matches"].is_null());
    assert!(record["landed"]["parent_matches"].is_null());
    assert_eq!(record["evidence_errors"], json!([{"read":"landed_commit","class":format!("http_{status}")} ]));
    let notices = m4_notices(&mut controller);
    assert_eq!(notices.len(), 1);
    assert_eq!(notices[0]["summary"], "merge evidence incomplete: landed_commit");
    assert_eq!(notices[0]["open"], false);
    assert_eq!(fixture.direct.lock().unwrap().puts.len(), usize::from(!webhook));
    let counts = adapters::delivery_counts(fixture.root.path()).unwrap();
    let notice_keys: Vec<_> = counts.keys().filter(|k| k.starts_with("incident:merge-audit:")).cloned().collect();
    assert_eq!(notice_keys.len(), 1, "one notice was delivered, not just scheduled");
    assert_eq!(counts[&notice_keys[0]], 1);
    drop(controller);
    fixture.wakes = direct_merge::Wakes::default();
    let mut controller = fixture.open(&settings);
    let body = m4_review_closed_event(&fixture);
    deliver(&settings, &fixture.api, "pull_request", &body);
    drain(&mut controller);
    controller.look_now().unwrap();
    drain(&mut controller);
    m4_review_one_merge(&mut controller, &settings);
    assert_eq!(m4_notices(&mut controller).len(), 1);
    assert_eq!(adapters::delivery_counts(fixture.root.path()).unwrap()[&notice_keys[0]], 1, "restart/replay must not post the same audit notice twice");
}
#[test]
fn m4_review_tl3_landed_404_still_records_merge() { m4_review_evidence_failure(false, 404); }
#[test]
fn m4_review_tl3_landed_500_still_records_merge() { m4_review_evidence_failure(false, 500); }
#[test]
fn m4_review_webhook_landed_404_still_records_merge() { m4_review_evidence_failure(true, 404); }
#[test]
fn m4_review_webhook_landed_500_still_records_merge() { m4_review_evidence_failure(true, 500); }
#[test]
fn m4_review_missing_report_has_unknown_base_comparisons() {
    let mut database = support::Database::start();
    let settings = database.create("m4_review_no_report");
    let fixture = DirectFixture::new();
    fixture.check(Some("success"));
    let (mut controller, _) = fixture.start(&settings);
    drain(&mut controller);
    let record = m4_record(&mut controller);
    assert!(record["landed"]["checked_base"].is_null());
    assert!(record["landed"]["base_moved"].is_null());
    assert!(record["landed"]["checked_parent_matches"].is_null());
    m4_review_one_merge(&mut controller, &settings);
}
fn m4_review_own_reconciliation(status: u16) {
    let mut database = support::Database::start();
    let settings = database.create(&format!("m4_review_own_{status}"));
    let fixture = DirectFixture::new();
    fixture.check(Some("success"));
    {
        let mut direct = fixture.direct.lock().unwrap();
        direct.merge_error_after_success = Some(status);
        direct.checks_fail_after_merge = true;
    }
    let (mut controller, _) = fixture.start(&settings);
    drain(&mut controller);
    m4_review_one_merge(&mut controller, &settings);
    let record = m4_record(&mut controller);
    assert_eq!(record["observed"], false, "a saved merge intent identifies TL3's own merge");
    assert_eq!(record["required_checks"]["passed"], false);
    assert!(!m4_notices(&mut controller).iter().any(|n| n["summary"].as_str().unwrap().contains("someone else's merge")));
}
#[test]
fn m4_review_lost_reply_is_not_an_outside_merge() { m4_review_own_reconciliation(503); }
#[test]
fn m4_review_409_is_not_an_outside_merge() { m4_review_own_reconciliation(409); }
#[test]
fn m4_review_webhook_uses_saved_intent() {
    let mut database = support::Database::start();
    let settings = database.create("m4_review_webhook_intent");
    let fixture = DirectFixture::new();
    fixture.check(Some("success"));
    let fired = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let flag = fired.clone();
    let adapter = fixture.adapter(&settings).with_write_boundary(move |b| {
        if b == (direct_merge::WriteBoundary {write:direct_merge::Write::Merge, after:true}) && !flag.swap(true, std::sync::atomic::Ordering::SeqCst) { panic!("stop after GitHub merged, before TL3 receipt"); }
    });
    let controller = fixture.open_adapter(&settings, adapter);
    assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(||fixture.start_controller(&settings, controller))).is_err());
    fixture.check(Some("failure"));
    let mut controller = fixture.open(&settings);
    deliver(&settings, &fixture.api, "pull_request", &m4_review_closed_event(&fixture));
    drain(&mut controller);
    m4_review_one_merge(&mut controller, &settings);
    let record = m4_record(&mut controller);
    assert_eq!(record["observed"], false);
    assert_eq!(record["landed"]["parent_matches"], true);
    assert!(record["landed"]["base_before_merge"].is_string());
    assert!(!m4_notices(&mut controller).iter().any(|n| n["summary"].as_str().unwrap().contains("someone else's merge")));
}
#[test]
fn m4_review_rebase_skips_parent_comparisons() {
    let mut database = support::Database::start();
    let settings = database.create("m4_review_rebase");
    let fixture = DirectFixture::new();
    fixture.check(Some("success"));
    { let mut direct = fixture.direct.lock().unwrap(); direct.rebase_method = true; direct.different_parent = true; }
    let (mut controller, _) = fixture.start(&settings);
    drain(&mut controller);
    m4_review_one_merge(&mut controller, &settings);
    let record = m4_record(&mut controller);
    assert_eq!(record["landed"]["method"], "rebase");
    assert!(record["landed"]["parent_matches"].is_null());
    assert!(record["landed"]["checked_parent_matches"].is_null());
    assert_eq!(record["landed"]["parent_comparison_reason"], "rebase merge method");
    assert_eq!(record["landed"]["tree_matches"], true);
    assert!(m4_notices(&mut controller).is_empty());
}
fn m4_review_missing_commit(missing_pr: bool) {
    let mut database = support::Database::start();
    let settings = database.create(if missing_pr {"m4_review_missing_both"} else {"m4_review_missing_reply"});
    let fixture = DirectFixture::new();
    fixture.check(Some("success"));
    { let mut direct = fixture.direct.lock().unwrap(); direct.reply_without_sha = true; direct.missing_merge_commit = missing_pr; }
    let (mut controller, _) = fixture.start(&settings);
    drain(&mut controller);
    m4_review_one_merge(&mut controller, &settings);
    let record = m4_record(&mut controller);
    if missing_pr {
        assert!(record["landed"]["commit"].is_null());
        assert!(record["evidence_errors"].as_array().unwrap().iter().any(|e| e["read"] == "merge_commit_sha" && e["class"] == "missing"));
        assert_eq!(m4_notices(&mut controller).len(), 1);
    } else { assert_eq!(record["landed"]["commit"], "d".repeat(40)); assert!(record["evidence_errors"].as_array().unwrap().is_empty()); }
}
#[test]
fn m4_review_missing_reply_sha_uses_fresh_pull() { m4_review_missing_commit(false); }
#[test]
fn m4_review_missing_both_commit_ids_still_records_merge() { m4_review_missing_commit(true); }
#[test]
fn m4_review_outside_protection_refusal_is_evidence_error() {
    let mut database = support::Database::start();
    let settings = database.create("m4_review_protection");
    let fixture = DirectFixture::new();
    fixture.check(None);
    let (mut controller, _) = fixture.start(&settings);
    drain(&mut controller);
    fixture.api.state.lock().unwrap().merged(false);
    fixture.direct.lock().unwrap().protection_refused_after_merge = true;
    deliver(&settings, &fixture.api, "pull_request", &m4_review_closed_event(&fixture));
    drain(&mut controller);
    m4_review_one_merge(&mut controller, &settings);
    let record = m4_record(&mut controller);
    assert!(record["required_checks"].is_null());
    assert!(record["evidence_errors"].as_array().unwrap().iter().any(|e| e["read"] == "required_checks" && e["class"] == "direct_protection_read"));
    assert_eq!(m4_notices(&mut controller).len(), 1);
}
#[test]
fn m4_review_observed_branch_failure_is_evidence_error() {
    let mut database = support::Database::start();
    let settings = database.create("m4_review_branch");
    let fixture = DirectFixture::new();
    fixture.check(None);
    let (mut controller, _) = fixture.start(&settings);
    drain(&mut controller);
    fixture.check(Some("success"));
    fixture.api.state.lock().unwrap().merged(false);
    fixture.direct.lock().unwrap().branch_status_after_merge = Some(500);
    deliver(&settings, &fixture.api, "pull_request", &m4_review_closed_event(&fixture));
    drain(&mut controller);
    m4_review_one_merge(&mut controller, &settings);
    let record = m4_record(&mut controller);
    assert!(record["landed"]["merge_base"].is_null());
    assert!(record["evidence_errors"].as_array().unwrap().iter().any(|e| e["read"] == "live_merge_base"));
    assert_eq!(m4_notices(&mut controller).len(), 1);
}
