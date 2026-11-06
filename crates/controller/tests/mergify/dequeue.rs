//! 4g / trustladderiii #864, found by live probe. A Mergify dequeue must end
//! TL3's queue attempt and route through #854's attribution and hold.
//!
//! PQ #3309 (request 400, PR RobTand/prismaquant#3333): two CHECKS_TIMEOUT
//! dequeues; TL3's single requeue (04:14:51Z) was answered (queued 04:15:36Z,
//! dequeued 04:46:15Z). A same-head re-publish (effect 400:19) found
//! `tl3-queue` present, reused that spent receipt and recorded "enqueue request
//! sent" with no write. Its allowance then failed as `queue_requeue`.
//!
//! PQ #3318 (request 392, PR RobTand/prismaquant#3343): TL3's no-progress stall
//! (15:13:32.494Z) and Mergify's 30-minute checks timeout (left_at
//! 15:13:32.238Z, queue check completed 15:13:35Z) raced. The stall read a
//! still-running queue check and raised `queue_unfinished`; the dequeue
//! arrived afterwards.
//!
//! The upstreams are the captured PR states, unmodified except for the
//! harness's repository, PR number, head and App login.
use super::*;

pub(super) struct Fixture;
impl Fixture {
    pub(super) fn respond(&self, path: &str) -> Option<ResponseTemplate> {
        if path.ends_with("/actions/jobs/127697164599/logs") {
            return Some(ResponseTemplate::new(200).set_body_string(include_str!(
                "../fixtures/mergify/pq3309/remote-bazel-job.log"
            )));
        }
        if path.ends_with("/actions/jobs/127715175818/logs") {
            return Some(ResponseTemplate::new(200).set_body_string(include_str!(
                "../fixtures/mergify/pq3309/pr-gate-job.log"
            )));
        }
        if path.ends_with("/pulls/100/files") {
            return Some(ResponseTemplate::new(200).set_body_json(json!([])));
        }
        None
    }
}

pub(super) const MODES: &[&str] = &[
    "pq3309",
    "pq3309-restart",
    "pq3318",
    "pq3318-running",
    "witness",
];

/// Part 2 of 4g: the slot rule is Rob's Q27 call (options A/B/C). These modes
/// assert the slot is released and stay out of the passing suite until then.
pub(super) const PENDING: &[(&str, &str)] = &[
    (
        "pq3309-slot",
        "4g part 2: slot release pending Q27 (A/B/C), https://github.com/RobTand/trustladderiii/issues/864",
    ),
    (
        "pq3318-slot",
        "4g part 2: slot release pending Q27 (A/B/C), https://github.com/RobTand/trustladderiii/issues/864",
    ),
];

const RULE: &str = "Rule: Queue TL3-approved PRs (queue)";

fn split(checks: &str) -> (Value, Value) {
    let checks: Vec<Value> = serde_json::from_str(checks).unwrap();
    let (rule, checks): (Vec<_>, Vec<_>) = checks.into_iter().partition(|c| c["name"] == RULE);
    (rule.into_iter().next().unwrap(), json!(checks))
}

fn common(state: &mut State, base: &str) {
    state.lag = true;
    state.pull["base"]["sha"] = json!(base);
    state.pull["mergeable"] = json!(true);
    state.pull["mergeable_state"] = json!("unstable");
    state.dequeue_fixture = Some(Fixture);
}

/// PQ #3309 at 06:28Z: both dequeues done, `tl3-queue` and `dequeued`, TL3's
/// one answered `@mergifyio queue`, and the head's exit-75 Remote Bazel run.
fn pq3309(state: &mut State) {
    let (rule, checks) = split(include_str!("../fixtures/mergify/pq3309/checks.json"));
    state.queue_rule = rule;
    state.checks = checks;
    let mut events: Vec<Value> =
        serde_json::from_str(include_str!("../fixtures/mergify/pq3309/events.json")).unwrap();
    for event in &mut events {
        event.as_object_mut().unwrap().remove("issue");
    }
    state.events = events;
    state.comments =
        serde_json::from_str(include_str!("../fixtures/mergify/pq3309/comments.json")).unwrap();
    state.label = true;
    state.dequeued = true;
    common(state, "ef817b9d48f4e2f55a3b7d8e83caef25573ad544");
}

/// PQ #3318 while the queue ran (14:42:50Z to 15:13:32Z): Mergify's queue
/// check still "Running merge queue checks", no dequeue evidence anywhere.
fn pq3318_running(state: &mut State) {
    pq3318_dequeued(state);
    let check = state
        .checks
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|c| c["name"] == tl3_mergify::QUEUE_CHECK)
        .unwrap();
    check["status"] = json!("in_progress");
    check["conclusion"] = Value::Null;
    check["completed_at"] = Value::Null;
    check["output"]["title"] = json!("Running merge queue checks");
    check["output"]["summary"] = json!("");
    state.events.truncate(2);
    state.comments.clear();
    state.label = false;
    state.dequeued = false;
}

/// PQ #3318 after 15:14:25Z: the captured dequeue (comment, labels and
/// Mergify's queue check "Dequeued — checks timeout").
fn pq3318_dequeued(state: &mut State) {
    let (rule, checks) = split(include_str!("../fixtures/mergify/pq3318/checks.json"));
    state.queue_rule = rule;
    state.checks = checks;
    state.events =
        serde_json::from_str(include_str!("../fixtures/mergify/pq3318/events.json")).unwrap();
    state.comments =
        serde_json::from_str(include_str!("../fixtures/mergify/pq3318/comments.json")).unwrap();
    state.label = true;
    state.dequeued = true;
    common(state, "ef817b9d48f4e2f55a3b7d8e83caef25573ad544");
}

/// TL3's 04:14:51Z command carried the per-PR/head requeue key. Rebind it to
/// the harness's PR, head and App login.
fn bind_command(state: &mut State, delivery: &Delivery) {
    let head = delivery.head_sha.clone().unwrap();
    state.pull["head"]["sha"] = json!(head);
    let key = requeue_key(state, &head);
    for comment in &mut state.comments {
        if comment["user"]["login"] == "tl3-robtand[bot]" {
            comment["user"] = json!({"id":123,"login":"factory[bot]"});
            comment["body"] = json!(format!("@mergifyio queue\n\n{}", GitHub::marker(&key)));
        }
    }
}

// The PR binding is hydrated after BeforeDispatch; the harness PR is #100.
fn requeue_key(state: &State, head: &str) -> String {
    format!(
        "mergify-requeue:owner/sandbox:{}:{head}",
        state.pull["number"].as_u64().unwrap()
    )
}

fn open(
    settings: &tl3_config::Store,
    api: &Server,
    root: &Path,
    clock: &head_checks::Clock,
    config: &Bootstrap,
) -> Controller {
    let mut registry = adapters::simulation(root).unwrap();
    registry.register(
        Upstream::Mergify,
        api.adapter(&root.join("mergify"), 0)
            .with_store(settings.clone())
            .with_clock(clock.clone()),
    );
    let mut controller = Controller::open_with_clock(
        Store::open(settings).unwrap(),
        registry,
        config.clone(),
        clock.clone(),
    )
    .unwrap();
    controller.register_github_translator("mergify", Translator::new(api.clients()).unwrap());
    controller
}

/// Drive one request from intake through publishing against `upstream`.
/// `enqueue` runs at the Enqueue effect's BeforeDispatch boundary.
fn publish(
    settings: &tl3_config::Store,
    api: &Server,
    root: &Path,
    clock: &head_checks::Clock,
    config: &Bootstrap,
    upstream: fn(&mut State),
    enqueue: fn(&mut State, &Delivery),
) -> Controller {
    let mut barrier = postgres::Config::new()
        .host(&settings.host)
        .port(settings.port)
        .dbname(&settings.database)
        .user(&settings.user)
        .password("tl3-disposable-test-password")
        .connect(postgres::NoTls)
        .unwrap();
    support::controller_restart::wait_for_owner_release(&mut barrier);
    upstream(&mut api.state.lock().unwrap());
    let repository = config.repos[0].clone();
    let mut controller = open(settings, api, root, clock, config);
    controller
        .intake(LabelledIssue {
            delivery: "label-1".into(),
            owner: repository.owner,
            repo: repository.name,
            installation: 3,
            number: 12,
            title: "change".into(),
            actor: 42,
            label: "tl3".into(),
            labels: Vec::new(),
        })
        .unwrap();
    for _ in 0..300 {
        scenario_progress("dequeue: controller step");
        let progressed = controller
            .drive_once_with(|boundary, key| {
                if boundary != Boundary::BeforeDispatch {
                    return;
                }
                let effect = Store::open(settings).unwrap().effect(key).unwrap().unwrap();
                let delivery: Delivery = serde_json::from_value(effect.payload).unwrap();
                if matches!(
                    delivery.output,
                    Out::Effect {
                        kind: EffectKind::Enqueue,
                        ..
                    }
                ) {
                    enqueue(&mut api.state.lock().unwrap(), &delivery);
                }
            })
            .unwrap();
        if !progressed {
            break;
        }
    }
    controller
}

fn bind_head(state: &mut State, delivery: &Delivery) {
    state.pull["head"]["sha"] = json!(delivery.head_sha);
}

fn pending_enqueue(controller: &mut Controller) -> Option<String> {
    controller
        .snapshot()
        .unwrap()
        .pending_effects
        .into_iter()
        .find(|effect| effect.payload["output"]["Effect"]["kind"] == "Enqueue")
        .map(|effect| effect.key)
}

/// Every Enqueue effect key, oldest first.
fn enqueues(settings: &tl3_config::Store) -> Vec<String> {
    Store::open(settings)
        .unwrap()
        .effects()
        .unwrap()
        .into_iter()
        .filter(|effect| effect.payload["output"]["Effect"]["kind"] == "Enqueue")
        .map(|effect| effect.key)
        .collect()
}

fn record(root: &Path, key: &str, extension: &str) -> std::path::PathBuf {
    let encoded: String = key.bytes().map(|b| format!("{b:02x}")).collect();
    root.join("mergify").join(format!("{encoded}.{extension}"))
}

fn incident_rows(controller: &mut Controller) -> Vec<Value> {
    controller
        .snapshot()
        .unwrap()
        .rows
        .into_iter()
        .filter(|row| row.table == "incidents")
        .map(|row| {
            json!({"id":row.id,"open":row.value["open"],"needs_rob":row.value["needs_rob"],
                "class":row.value["signature"]["class"],"cause":row.value["cause"]})
        })
        .collect()
}

fn causes(controller: &mut Controller) -> Vec<Value> {
    controller
        .snapshot()
        .unwrap()
        .rows
        .into_iter()
        .filter(|row| row.table == "queue_facts" && row.value["evidence"].get("CiFailed").is_some())
        .map(|row| row.value["evidence"]["CiFailed"]["details"]["cause"].clone())
        .collect()
}

fn held(controller: &mut Controller) -> bool {
    controller
        .snapshot()
        .unwrap()
        .rows
        .iter()
        .any(|row| row.table == "queue_holds")
}

fn deliver(controller: &mut Controller, api: &Server, config: &Bootstrap, event: &str, body: Value) {
    for input in Translator::new(api.clients())
        .unwrap()
        .webhook(
            &parsed(event, body),
            &controller.snapshot().unwrap(),
            &config.repos,
        )
        .unwrap()
    {
        ingress::apply(controller, input).unwrap();
    }
    drain(controller);
}

fn queue_check_completed(controller: &mut Controller, api: &Server, config: &Bootstrap) {
    let mut body = webhook_fixture();
    body["action"] = json!("completed");
    body["check_run"] = api.state.lock().unwrap().queue_check();
    deliver(controller, api, config, "check_run", body);
}

/// A real merge on the held PR's destination after the dequeue, at a newer base.
fn health(controller: &mut Controller, api: &Server, config: &Bootstrap) {
    let mut body = webhook_fixture();
    let mut pull = api.state.lock().unwrap().pull.clone();
    pull["number"] = json!(101);
    pull["head"]["sha"] = json!("cccccccccccccccccccccccccccccccccccccccc");
    pull["base"]["ref"] = json!("main");
    pull["state"] = json!("closed");
    pull["merged"] = json!(true);
    pull["merged_at"] = json!("2026-11-06T15:30:00Z");
    pull["merge_commit_sha"] = json!("ffffffffffffffffffffffffffffffffffffffff");
    body["pull_request"] = pull;
    body["action"] = json!("closed");
    deliver(controller, api, config, "pull_request", body);
}

fn looks(controller: &mut Controller, clock: &head_checks::Clock, step: u64, count: usize) {
    for look in 1..=count {
        scenario_progress(format!("dequeue: look {look}"));
        clock.0.set(clock.0.get() + Duration::from_secs(step));
        controller.look_now().unwrap();
        drain(controller);
    }
}

struct Verdict(Vec<String>);
impl Verdict {
    fn check(&mut self, ok: bool, violation: &str) {
        if !ok {
            self.0.push(violation.to_owned());
        }
    }
    fn finish(self, mode: &str) {
        for violation in &self.0 {
            println!("RED 4g {mode}: {violation}");
        }
        assert!(self.0.is_empty(), "4g {mode}: {} violation(s)", self.0.len());
        println!("test dequeue_{mode} ... ok");
    }
}

/// The ended attempt's common green state, after the hold's no-progress bound.
fn ended(
    verdict: &mut Verdict,
    controller: &mut Controller,
    api: &Server,
    before: tl3_kernel::Node,
    posts: usize,
) {
    let node = controller.world().unwrap().nodes[0];
    let incidents = incident_rows(controller);
    let causes = causes(controller);
    let enqueue = pending_enqueue(controller);
    let (sent, attempts, requeues) = {
        let state = api.state.lock().unwrap();
        (state.posts, state.command_attempts, state.requeues)
    };
    println!(
        "4g after the bound: phase={:?} holds={} pending_enqueue={enqueue:?} posts={sent} command_attempts={attempts} requeues={requeues} causes={} incidents={}",
        node.phase,
        node.holds,
        serde_json::to_string(&causes).unwrap(),
        serde_json::to_string(&incidents).unwrap()
    );
    verdict.check(
        enqueue.is_none() && node.phase == Phase::Queued,
        "attempt never ended: the enqueue is still pending in merge queue admission",
    );
    verdict.check(held(controller), "the dequeue did not reach the #854 hold");
    verdict.check(
        causes.iter().any(|cause| cause == "Unattributed"),
        "the CHECKS_TIMEOUT dequeue was not attributed Unattributed",
    );
    verdict.check(
        sent == posts && attempts == 0 && requeues == 0,
        "an upstream write was sent; the single shared requeue must not be re-sent",
    );
    verdict.check(
        !incidents.iter().any(|incident| incident["needs_rob"] == true),
        "a Rob-only incident was raised for an environmental dequeue",
    );
    verdict.check(
        !incidents
            .iter()
            .any(|incident| incident["class"] == "queue_requeue"),
        "the answered requeue was reported as permanently failed queue_requeue",
    );
    verdict.check(
        !incidents.iter().any(|incident| {
            incident["class"] == "queue_unfinished" && incident["open"] == true
        }),
        "a 'Mergify has not finished' incident stays open after the dequeue",
    );
    verdict.check(
        incidents.iter().any(|incident| {
            incident["class"] == "queue_environment"
                && incident["open"] == true
                && incident["cause"] == "tl3"
        }),
        "the hold's bound raised no visible TL3-side Q51 incident",
    );
    verdict.check(
        node.credit.spent == before.credit.spent
            && node.round == before.round
            && node.run_failures == before.run_failures,
        "the dequeue was charged",
    );
}

/// PQ #3309: the captured 06:28Z state; the same-head enqueue executes now.
fn scenario_3309(settings: &tl3_config::Store, mode: &str) {
    let api = Server::new();
    let root = tempfile::tempdir().unwrap();
    let config = bootstrap();
    let clock = head_checks::Clock(std::rc::Rc::new(std::cell::Cell::new(
        std::time::SystemTime::now(),
    )));
    let mut controller = publish(settings, &api, root.path(), &clock, &config, pq3309, bind_command);
    let mut verdict = Verdict(vec![]);
    let key = enqueues(settings).pop().unwrap();
    let applied = record(root.path(), &key, "applied").exists();
    let before = controller.world().unwrap().nodes[0];
    println!(
        "4g pq3309 after enqueue: phase={:?} pending_enqueue={:?} applied_marker={applied}",
        before.phase,
        pending_enqueue(&mut controller)
    );
    verdict.check(
        !applied,
        "false sent: no label or command reached the PR, yet the enqueue wrote its applied marker",
    );
    drop(controller);
    let mut controller = open(settings, &api, root.path(), &clock, &config);
    looks(&mut controller, &clock, 100_000, 5);
    ended(&mut verdict, &mut controller, &api, before, 0);
    if mode == "pq3309-slot" {
        verdict.check(
            !controller.world().unwrap().nodes[0].holds,
            "change slot still held on the hold",
        );
    }
    verdict.finish(mode);
}

/// PQ #3309 as the live store held it: effect 400:19 executed under the old
/// controller (intent, applied marker and the spent per-head receipt) and the
/// 04:46Z dequeue was journaled without #854 attribution. tl3d restarts.
fn scenario_restart(settings: &tl3_config::Store, mode: &str) {
    let api = Server::new();
    let root = tempfile::tempdir().unwrap();
    let config = bootstrap();
    let clock = head_checks::Clock(std::rc::Rc::new(std::cell::Cell::new(
        std::time::SystemTime::now(),
    )));
    fn unavailable(state: &mut State, delivery: &Delivery) {
        bind_command(state, delivery);
        state.fail_reads = true;
    }
    let mut controller = publish(settings, &api, root.path(), &clock, &config, pq3309, unavailable);
    let key = pending_enqueue(&mut controller).expect("the old enqueue is still pending");
    let before = controller.world().unwrap().nodes[0];
    api.state.lock().unwrap().fail_reads = false;
    // The old controller journaled the dequeue as a classified fact, no cause.
    let mut body = webhook_fixture();
    body["action"] = json!("completed");
    body["check_run"] = api.state.lock().unwrap().queue_check();
    let snapshot = controller.snapshot().unwrap();
    let mut seed = tl3_store::Commit::new("4g-legacy", json!({}), &snapshot);
    for input in Translator::new(api.clients())
        .unwrap()
        .webhook(&parsed("check_run", body), &snapshot, &config.repos)
        .unwrap()
    {
        if let tl3_controller::cli::Input::Observation {
            key,
            observation: tl3_controller::executor::Observation::Queue(fact),
        } = input
        {
            let mut value = serde_json::to_value(&fact).unwrap();
            value["evidence"]["CiFailed"]["details"]["decision"] = json!("upstream_classification");
            seed.rows.push(tl3_store::RowChange::Put {
                table: "queue_facts".into(),
                id: key.strip_prefix("mergify:").unwrap().into(),
                value,
            });
        }
    }
    assert!(!seed.rows.is_empty(), "the captured dequeue translates to a fact");
    drop(controller);
    Store::open(settings).unwrap().commit(&seed).unwrap();
    // The old executor's records for effect 400:19.
    let head = api.state.lock().unwrap().pull["head"]["sha"].as_str().unwrap().to_owned();
    let receipt = requeue_key(&api.state.lock().unwrap(), &head);
    std::fs::write(
        record(root.path(), &key, "intent"),
        br#"{"before":null,"last_event":0,"requeue":"Ready"}"#,
    )
    .unwrap();
    std::fs::write(record(root.path(), &key, "applied"), b"true").unwrap();
    let digest = format!("{:x}", Sha256::digest(receipt.as_bytes()));
    std::fs::write(
        record(root.path(), &format!("requeue:{digest}"), "requeue"),
        serde_json::to_vec(&json!({"eligible_since_ms":null,"command":"Sent","command_key":receipt}))
            .unwrap(),
    )
    .unwrap();
    let mut controller = open(settings, &api, root.path(), &clock, &config);
    drain(&mut controller);
    looks(&mut controller, &clock, 100_000, 5);
    let mut verdict = Verdict(vec![]);
    ended(&mut verdict, &mut controller, &api, before, 0);
    verdict.finish(mode);
}

/// PQ #3318: the stall/timeout race, then the dequeue.
fn scenario_3318(settings: &tl3_config::Store, mode: &str) {
    let api = Server::new();
    let root = tempfile::tempdir().unwrap();
    let config = bootstrap();
    let clock = head_checks::Clock(std::rc::Rc::new(std::cell::Cell::new(
        std::time::SystemTime::now(),
    )));
    let mut controller = publish(
        settings,
        &api,
        root.path(),
        &clock,
        &config,
        pq3318_running,
        bind_head,
    );
    let before = controller.world().unwrap().nodes[0];
    let posts = api.state.lock().unwrap().posts;
    assert_eq!(before.phase, Phase::Queued, "Mergify acknowledged the enqueue");
    // TL3's 1800 s no-progress allowance equals Mergify's 30-minute checks
    // timeout. At 15:13:32Z no read showed a dequeue: the control stall.
    looks(&mut controller, &clock, 1_801, 1);
    let incidents = incident_rows(&mut controller);
    println!(
        "4g {mode} at the stall: phase={:?} incidents={}",
        controller.world().unwrap().nodes[0].phase,
        serde_json::to_string(&incidents).unwrap()
    );
    let mut verdict = Verdict(vec![]);
    if mode == "pq3318-running" {
        verdict.check(
            incidents.iter().any(|incident| {
                incident["class"] == "queue_unfinished"
                    && incident["open"] == true
                    && incident["needs_rob"] == false
            }),
            "a genuinely running queue check no longer takes the existing stall",
        );
        verdict.check(!held(&mut controller), "no dequeue, yet a hold");
        verdict.finish(mode);
        return;
    }
    // 15:13:33Z to 15:14:25Z: Mergify's dequeue lands.
    pq3318_dequeued(&mut api.state.lock().unwrap());
    queue_check_completed(&mut controller, &api, &config);
    looks(&mut controller, &clock, 100_000, 5);
    let keys = enqueues(settings);
    let applied = keys
        .iter()
        .skip(1)
        .any(|key| record(root.path(), key, "applied").exists());
    println!("4g {mode} enqueues={keys:?} later_applied={applied}");
    verdict.check(
        !applied,
        "false sent: the re-publish wrote nothing, yet recorded the enqueue applied",
    );
    ended(&mut verdict, &mut controller, &api, before, posts);
    if mode == "pq3318-slot" {
        verdict.check(
            !controller.world().unwrap().nodes[0].holds,
            "change slot still held on the hold",
        );
    }
    verdict.finish(mode);
}

/// Fresh receipt: the dequeue arrives while the enqueue still awaits Mergify's
/// queue check, so the hold is taken in Publishing. One newer-base merge
/// authorizes exactly one requeue, and the enqueue ends.
fn scenario_witness(settings: &tl3_config::Store, mode: &str) {
    let api = Server::new();
    let root = tempfile::tempdir().unwrap();
    let config = bootstrap();
    let clock = head_checks::Clock(std::rc::Rc::new(std::cell::Cell::new(
        std::time::SystemTime::now(),
    )));
    fn awaiting(state: &mut State) {
        pq3318_running(state);
        state
            .checks
            .as_array_mut()
            .unwrap()
            .retain(|c| c["name"] != tl3_mergify::QUEUE_CHECK);
    }
    let mut controller = publish(settings, &api, root.path(), &clock, &config, awaiting, bind_head);
    let mut verdict = Verdict(vec![]);
    let phase = controller.world().unwrap().nodes[0].phase;
    verdict.check(phase == Phase::Publishing, "the enqueue was acknowledged early");
    pq3318_dequeued(&mut api.state.lock().unwrap());
    queue_check_completed(&mut controller, &api, &config);
    verdict.check(held(&mut controller), "the dequeue did not reach the #854 hold");
    health(&mut controller, &api, &config);
    looks(&mut controller, &clock, 1, 2);
    let (attempts, requeues) = {
        let state = api.state.lock().unwrap();
        (state.command_attempts, state.requeues)
    };
    let node = controller.world().unwrap().nodes[0];
    println!(
        "4g {mode}: phase={:?} pending_enqueue={:?} command_attempts={attempts} requeues={requeues}",
        node.phase,
        pending_enqueue(&mut controller)
    );
    verdict.check(
        requeues == 1 && attempts == 1,
        "the witness must authorize exactly one requeue from the held Publishing attempt",
    );
    verdict.finish(mode);
}

pub(super) fn scenario(settings: &tl3_config::Store, mode: &str) {
    match mode {
        "pq3309" | "pq3309-slot" => scenario_3309(settings, mode),
        "pq3309-restart" => scenario_restart(settings, mode),
        "pq3318" | "pq3318-running" | "pq3318-slot" => scenario_3318(settings, mode),
        "witness" => scenario_witness(settings, mode),
        _ => panic!("unknown 4g mode {mode}"),
    }
}
