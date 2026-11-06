//! 4g / trustladderiii #864, found by live probe: PQ #3309 (request 400, PR
//! RobTand/prismaquant#3333). Mergify dequeued the PR twice with CHECKS_TIMEOUT
//! (04:13:35Z, 04:46:15Z). TL3's single requeue command (04:14:51Z) had been
//! answered: queued 04:15:36Z, dequeued again 04:46:15Z. A later re-publish of
//! the same head (effect 400:19, 06:28Z) found `tl3-queue` still present, reused
//! that spent command receipt, and recorded "enqueue request sent" without
//! sending anything. Its allowance then failed permanently as `queue_requeue`
//! and the request held PQ's change slot in Publishing for 5.5 h.
//!
//! The upstream here is the captured PR state at 06:28Z, unmodified except for
//! the harness's repository, PR number, head and App login.
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

pub(super) const MODES: &[&str] = &["pq3309"];

const RULE: &str = "Rule: Queue TL3-approved PRs (queue)";

/// The captured upstream at 06:28Z: both dequeues done, labels `tl3-queue` and
/// `dequeued`, TL3's one answered `@mergifyio queue`, and the head's failed
/// Remote Bazel run (exit 75, "Remote evidence refused at shard-await").
fn captured(state: &mut State) {
    let checks: Vec<Value> =
        serde_json::from_str(include_str!("../fixtures/mergify/pq3309/checks.json")).unwrap();
    let (rule, checks): (Vec<_>, Vec<_>) = checks.into_iter().partition(|c| c["name"] == RULE);
    state.queue_rule = rule.into_iter().next().unwrap();
    state.checks = json!(checks);
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
    state.lag = true;
    state.pull["base"]["sha"] = json!("ef817b9d48f4e2f55a3b7d8e83caef25573ad544");
    state.pull["mergeable"] = json!(true);
    state.pull["mergeable_state"] = json!("unstable");
    state.dequeue_fixture = Some(Fixture);
}

/// TL3's 04:14:51Z command carried the per-PR/head requeue key. Rebind it to
/// the harness's PR and head, and to the harness App's login.
fn bind_command(state: &mut State, delivery: &Delivery) {
    let head = delivery.head_sha.clone().unwrap();
    state.pull["head"]["sha"] = json!(head);
    // The PR binding is hydrated after this boundary; the harness PR is #100.
    let key = format!(
        "mergify-requeue:{}/{}:{}:{head}",
        delivery.repository.owner.to_ascii_lowercase(),
        delivery.repository.name.to_ascii_lowercase(),
        state.pull["number"].as_u64().unwrap()
    );
    for comment in &mut state.comments {
        if comment["user"]["login"] == "tl3-robtand[bot]" {
            comment["user"] = json!({"id":123,"login":"factory[bot]"});
            comment["body"] = json!(format!("@mergifyio queue\n\n{}", GitHub::marker(&key)));
        }
    }
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

/// Drive one request from intake through publishing; its enqueue (effect
/// 400:19's role) executes against the captured upstream.
fn republish(
    settings: &tl3_config::Store,
    api: &Server,
    root: &Path,
    clock: &head_checks::Clock,
    config: &Bootstrap,
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
    captured(&mut api.state.lock().unwrap());
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
                    bind_command(&mut api.state.lock().unwrap(), &delivery);
                }
            })
            .unwrap();
        if !progressed {
            break;
        }
    }
    controller
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

pub(super) fn scenario(settings: &tl3_config::Store, mode: &str) {
    assert_eq!(mode, "pq3309");
    let api = Server::new();
    let root = tempfile::tempdir().unwrap();
    let config = bootstrap();
    let clock = head_checks::Clock(std::rc::Rc::new(std::cell::Cell::new(
        std::time::SystemTime::now(),
    )));
    let mut controller = republish(settings, &api, root.path(), &clock, &config);
    let mut violations = Vec::new();
    let (posts, attempts) = {
        let state = api.state.lock().unwrap();
        (state.posts, state.command_attempts)
    };
    let enqueue = pending_enqueue(&mut controller);
    let applied = marker_files(&root.path().join("mergify"))
        .into_iter()
        .any(|path| path.extension().is_some_and(|ext| ext == "applied"));
    println!(
        "4g pq3309 after enqueue: phase={:?} pending_enqueue={enqueue:?} posts={posts} command_attempts={attempts} applied_marker={applied}",
        controller.world().unwrap().nodes[0].phase
    );
    if posts == 0 && attempts == 0 && applied {
        violations.push(
            "false sent: no label or command reached the PR, yet the enqueue wrote its \
             applied marker and reported the request sent"
                .to_owned(),
        );
    }
    // The live controller restarted at 06:30:48Z; its looks then ran past the
    // no-progress allowance (06:59:07Z: permanently failed queue_requeue).
    drop(controller);
    let mut controller = open(settings, &api, root.path(), &clock, &config);
    for look in 1..=5 {
        scenario_progress(format!("dequeue: look {look}"));
        clock.0.set(clock.0.get() + Duration::from_secs(100_000));
        controller.look_now().unwrap();
        drain(&mut controller);
    }
    let node = controller.world().unwrap().nodes[0];
    let incidents = incident_rows(&mut controller);
    let enqueue = pending_enqueue(&mut controller);
    let requeues = api.state.lock().unwrap().requeues;
    println!(
        "4g pq3309 after the bound: phase={:?} holds={} pending_enqueue={enqueue:?} requeues={requeues} incidents={}",
        node.phase,
        node.holds,
        serde_json::to_string(&incidents).unwrap()
    );
    if node.phase == Phase::Publishing && enqueue.is_some() {
        violations.push(
            "attempt never ended: still Publishing on a pending enqueue (merge queue admission)"
                .to_owned(),
        );
    }
    if node.holds {
        violations.push("change slot still held after the no-progress bound".to_owned());
    }
    if incidents.iter().any(|incident| incident["needs_rob"] == true) {
        violations.push("a Rob-only incident was raised for an environmental dequeue".to_owned());
    }
    if incidents
        .iter()
        .any(|incident| incident["class"] == "queue_requeue")
    {
        violations.push("the answered requeue was reported as permanently failed queue_requeue".to_owned());
    }
    if !incidents.iter().any(|incident| incident["open"] == true) {
        violations.push("no visible incident classifies the ended attempt".to_owned());
    }
    if requeues != 0 {
        violations.push(format!(
            "the single shared requeue was already spent at 04:14:51Z, yet {requeues} more were sent"
        ));
    }
    for violation in &violations {
        println!("RED 4g pq3309: {violation}");
    }
    assert!(
        violations.is_empty(),
        "4g pq3309: {} violation(s)",
        violations.len()
    );
    println!("test dequeue_{mode} ... ok");
}
