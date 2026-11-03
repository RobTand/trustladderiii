//! #608: a repaired head after a verified dequeue needs one bounded queue command.
use super::*;
use tl3_controller::executor::Look;

struct Fixture {
    api: Server,
    root: tempfile::TempDir,
    clock: head_checks::Clock,
    settings: tl3_config::Store,
    delivery: Delivery,
    old_head: String,
}
impl Fixture {
    fn adapter(&self) -> Adapter {
        self.api
            .adapter(&self.root.path().join("mergify"), 0)
            .with_store(self.settings.clone())
            .with_clock(self.clock.clone())
    }
    fn open(&self) -> Controller {
        let mut registry = adapters::simulation(self.root.path()).unwrap();
        registry.register(Upstream::Mergify, self.adapter());
        Controller::open_with_clock(
            Store::open(&self.settings).unwrap(),
            registry,
            bootstrap(),
            self.clock.clone(),
        )
        .unwrap()
    }
    fn advance(&self, seconds: u64) {
        self.clock
            .0
            .set(self.clock.0.get() + Duration::from_secs(seconds));
    }
    fn look(&self, controller: &mut Controller) {
        controller.look_key(&self.delivery.key).unwrap();
        drain(controller);
    }
    fn commands(&self) -> usize {
        self.api.state.lock().unwrap().command_attempts
    }
    fn grace(&self, controller: &mut Controller) {
        self.look(controller);
        self.advance(299);
        self.look(controller);
        assert_eq!(self.commands(), 0, "wait five minutes before re-queue");
        self.advance(2);
    }
}

fn waiting_payload() -> Value {
    json!({"version":1,"state":"waiting","queue_rule_name":null,"queued_at":null,
    "speculative_check_pr":null,"required_conditions":[
        {"description":"-closed","match":true},
        {"description":"-draft","match":true},
        {"description":"check-success=issue-link","match":true},
        {"description":"label=tl3-queue","match":true},
        {"description":"-queue-paused","match":true},
        {"description":"-label=hold","match":true}
    ]})
}
fn set_payload(state: &mut State, payload: Value) {
    let bytes = serde_json::to_vec(&payload).unwrap();
    let alphabet = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut encoded = String::new();
    for chunk in bytes.chunks(3) {
        let word = chunk
            .iter()
            .fold(0_u32, |word, byte| (word << 8) | u32::from(*byte))
            << (8 * (3 - chunk.len()));
        for index in 0..4 {
            encoded.push(if index <= chunk.len() {
                alphabet[((word >> (18 - 6 * index)) & 63) as usize] as char
            } else {
                '='
            });
        }
    }
    state.checks = fixture("waiting", "checks");
    state.queue_check_mut()["output"]["summary"] =
        json!(format!("<span itemprop=\"mergify.v1.{encoded}\"></span>"));
}

fn setup(prior: bool) -> (support::Database, Fixture, Controller) {
    let mut database = support::Database::start();
    let settings = database.create("waiting_requeue");
    let api = Server::new();
    let root = tempfile::tempdir().unwrap();
    let mut controller = queued(&settings, &api, root.path(), true);
    let old_head = api.state.lock().unwrap().pull["head"]["sha"]
        .as_str()
        .unwrap()
        .to_owned();
    if prior {
        {
            let mut state = api.state.lock().unwrap();
            state.checks = fixture("failed", "checks");
            state.pull["mergeable"] = json!(true);
            state.pull["mergeable_state"] = json!("clean");
            state.dequeued = true;
        }
        // Read a real Mergify failure through the translator, then run the fix round.
        let mut body = webhook_fixture();
        body["action"] = json!("completed");
        body["check_run"] = api.state.lock().unwrap().queue_check();
        let inputs = Translator::new(api.clients())
            .unwrap()
            .webhook(
                &parsed("check_run", body),
                &controller.snapshot().unwrap(),
                &bootstrap().repos,
            )
            .unwrap();
        for input in inputs {
            ingress::apply(&mut controller, input).unwrap();
        }
        assert!(controller.drive_once().unwrap());
        assert_eq!(controller.world().unwrap().nodes[0].phase, Phase::Making);
        assert!(
            !queue_facts(&mut controller).is_empty(),
            "verified dequeue history is durable"
        );
    }
    let clock = head_checks::Clock(std::rc::Rc::new(std::cell::Cell::new(
        std::time::SystemTime::now(),
    )));
    let mut delivery = None;
    if prior {
        for _ in 0..300 {
            let progressed = controller
                .drive_once_with(|boundary, key| {
                    if boundary == Boundary::BeforeDispatch {
                        let effect = Store::open(&settings)
                            .unwrap()
                            .effect(key)
                            .unwrap()
                            .unwrap();
                        let d: Delivery = serde_json::from_value(effect.payload).unwrap();
                        if matches!(
                            d.output,
                            Out::Effect {
                                kind: EffectKind::Enqueue,
                                ..
                            }
                        ) {
                            let mut state = api.state.lock().unwrap();
                            state.pull["head"]["sha"] = json!(d.head_sha);
                            state.dequeued = false;
                            state.lag = true;
                            state.queue_rule["completed_at"] = Value::Null;
                            set_payload(&mut state, waiting_payload());
                            delivery = Some(d);
                        }
                    }
                })
                .unwrap();
            if !progressed {
                break;
            }
        }
    } else {
        let snapshot = controller.snapshot().unwrap();
        delivery = Some(
            serde_json::from_value(
                snapshot
                    .rows
                    .iter()
                    .find(|row| row.table == "watches")
                    .unwrap()
                    .value
                    .clone(),
            )
            .unwrap(),
        );
        let mut state = api.state.lock().unwrap();
        state.lag = true;
        set_payload(&mut state, waiting_payload());
    }
    drop(controller);
    let fixture = Fixture {
        api,
        root,
        clock,
        settings,
        delivery: delivery.unwrap(),
        old_head,
    };
    let controller = fixture.open();
    (database, fixture, controller)
}

fn prior_dequeue_survives_repaired_head_and_removed_label() {
    let (_db, f, mut c) = setup(true);
    assert_ne!(f.delivery.head_sha.as_deref(), Some(f.old_head.as_str()));
    assert!(!f.api.state.lock().unwrap().dequeued);
    f.grace(&mut c);
    f.look(&mut c);
    assert_eq!(
        f.commands(),
        1,
        "PB #2705: eligible repaired head must re-queue once after grace"
    );
    assert_eq!(
        c.world().unwrap().nodes[0].phase,
        Phase::Publishing,
        "command receipt is not acceptance"
    );
}

fn requeue_intent_and_marker_survive_restart_lost_response_and_effect_retirement() {
    let (_db, f, mut c) = setup(true);
    f.api.state.lock().unwrap().command_lost_response = true;
    f.grace(&mut c);
    drop(c);
    let mut c = f.open();
    f.look(&mut c);
    assert_eq!(f.commands(), 1, "dispatch saved intent after restart");
    let comments = f.api.state.lock().unwrap().comments.clone();
    let body = comments.last().unwrap()["body"].as_str().unwrap();
    assert!(body.starts_with("@mergifyio queue\n\n<!-- tl3:effect:"));
    assert!(
        body.contains(&hex::encode(f.delivery.head_sha.as_ref().unwrap())),
        "marker binds the exact head"
    );
    drop(c);
    let mut adapter = f.adapter();
    adapter.retire_delivery(&f.delivery).unwrap();
    let mut another = f.delivery.clone();
    another.key.push_str("-another-effect");
    for _ in 0..4 {
        let _ = adapter.look(&another);
    }
    assert_eq!(
        f.commands(),
        1,
        "same repository/PR/head must never dispatch twice"
    );
}

fn dispatch_rechecks_head_conditions_and_human_decisions() {
    for mode in [
        "unmet",
        "unknown-payload",
        "running",
        "active",
        "paused",
        "hold",
        "human",
        "closed",
        "merged",
        "head",
        "missing-proof",
        "permission-read",
    ] {
        let (_db, f, mut c) = setup(true);
        f.grace(&mut c);
        {
            let mut state = f.api.state.lock().unwrap();
            match mode {
                "unmet" | "paused" | "hold" => {
                    let mut payload = waiting_payload();
                    payload["required_conditions"][if mode == "paused" {
                        4
                    } else if mode == "hold" {
                        5
                    } else {
                        2
                    }]["match"] = json!(false);
                    set_payload(&mut state, payload);
                }
                "unknown-payload" => {
                    state.queue_check_mut()["output"]["summary"] = Value::Null;
                }
                "running" => {
                    state.required = true;
                    state.head_checks = vec![
                        json!({"id":900,"name":"Sandbox tests","head_sha":f.delivery.head_sha,"status":"in_progress","conclusion":null,"app":{"id":15368,"slug":"github-actions"}}),
                    ];
                }
                "active" => {
                    state.checks = fixture("running", "checks");
                }
                "human" => {
                    state.events.push(json!({"id":100,"event":"unlabeled","actor":{"id":42,"login":"RobTand"},"label":{"name":"tl3-queue"},"created_at":"2026-11-03T10:00:00Z"}));
                }
                "closed" => state.pull["state"] = json!("closed"),
                "merged" => state.merged(false),
                "head" => {
                    state.pull["head"]["sha"] = json!("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb")
                }
                "permission-read" => {
                    state.required = true;
                    state.restricted = Some(("rules".into(), 403));
                }
                "missing-proof" => {}
                _ => unreachable!(),
            }
        }
        let mut d = f.delivery.clone();
        if mode == "missing-proof" {
            d.request_text = Some("Publish PR with evidence".into());
        }
        let mut adapter = f.adapter();
        let _ = adapter.look(&d);
        assert_eq!(f.commands(), 0, "dispatch guard: {mode}");
    }
    let (_db, f, mut c) = setup(true);
    f.grace(&mut c);
    f.look(&mut c);
    assert_eq!(
        f.commands(),
        1,
        "an eligible head still dispatches after guard checks"
    );
}

fn no_prior_dequeue_never_recovers_and_later_approved_head_has_its_own_budget() {
    let (_db, f, mut c) = setup(false);
    f.advance(1000);
    f.look(&mut c);
    assert_eq!(
        f.commands(),
        0,
        "initial queue acknowledgement is not a prior dequeue"
    );
    let (_db2, f, mut c) = setup(true);
    f.grace(&mut c);
    f.look(&mut c);
    assert_eq!(f.commands(), 1, "the prior dequeue permits one command");
    for _ in 0..4 {
        f.look(&mut c);
    }
    assert_eq!(f.commands(), 1);
}

fn refused_or_unacknowledged_requeue_uses_bounded_existing_recovery() {
    for mode in ["reply", "permission", "unknown"] {
        let (_db, f, mut c) = setup(true);
        if mode == "reply" {
            f.api.state.lock().unwrap().command_reply = Some(requeue::restricted_reply());
        }
        if mode == "permission" {
            f.api.state.lock().unwrap().command_status = 403;
        }
        f.grace(&mut c);
        f.look(&mut c);
        assert_eq!(f.commands(), 1, "one bounded dispatch: {mode}");
        for _ in 0..6 {
            f.advance(100_000);
            c.look_now().unwrap();
            drain(&mut c);
        }
        assert_eq!(f.commands(), 1, "no repeated command after {mode}");
        assert!(
            c.snapshot()
                .unwrap()
                .rows
                .iter()
                .any(|row| row.table == "incidents"),
            "existing incident path: {mode}"
        );
    }
}

fn waiting_requeue_progress_and_notice_name_the_completed_check_correctly() {
    let (_db, f, mut c) = setup(true);
    let progress = match f.adapter().look(&f.delivery).unwrap() {
        Look::Alive { progress } => progress,
        _ => panic!("eligible repaired head awaits re-queue"),
    };
    assert!(
        progress.contains("awaiting Mergify re-queue") && progress.contains("no command sent"),
        "{progress}"
    );
    assert!(
        progress.contains(f.delivery.head_sha.as_ref().unwrap()) && progress.contains("100"),
        "{progress}"
    );
    f.grace(&mut c);
    f.look(&mut c);
    let progress = match f.adapter().look(&f.delivery).unwrap() {
        Look::Alive { progress } => progress,
        _ => panic!("wait for acceptance"),
    };
    assert!(
        progress.contains("command sent/awaiting acceptance"),
        "{progress}"
    );
    for _ in 0..5 {
        f.advance(100_000);
        c.look_now().unwrap();
        drain(&mut c);
    }
    let snapshot = c.snapshot().unwrap();
    let summaries: Vec<_> = snapshot
        .rows
        .iter()
        .filter(|row| row.table == "incidents")
        .filter_map(|row| row.value["summary"].as_str())
        .collect();
    assert!(
        summaries
            .iter()
            .any(|text| text.contains("awaiting Mergify re-queue")
                && text.contains(f.delivery.head_sha.as_ref().unwrap())),
        "{summaries:?}"
    );
    assert!(
        summaries
            .iter()
            .all(|text| !text.contains("has not finished its queue check")),
        "{summaries:?}"
    );
}

pub(super) fn run() {
    let cases: &[(&str, fn())] = &[
        (
            "prior_dequeue_survives_repaired_head_and_removed_label",
            prior_dequeue_survives_repaired_head_and_removed_label,
        ),
        (
            "requeue_intent_and_marker_survive_restart_lost_response_and_effect_retirement",
            requeue_intent_and_marker_survive_restart_lost_response_and_effect_retirement,
        ),
        (
            "dispatch_rechecks_head_conditions_and_human_decisions",
            dispatch_rechecks_head_conditions_and_human_decisions,
        ),
        (
            "no_prior_dequeue_never_recovers_and_later_approved_head_has_its_own_budget",
            no_prior_dequeue_never_recovers_and_later_approved_head_has_its_own_budget,
        ),
        (
            "refused_or_unacknowledged_requeue_uses_bounded_existing_recovery",
            refused_or_unacknowledged_requeue_uses_bounded_existing_recovery,
        ),
        (
            "waiting_requeue_progress_and_notice_name_the_completed_check_correctly",
            waiting_requeue_progress_and_notice_name_the_completed_check_correctly,
        ),
    ];
    let mut failures = Vec::new();
    for (name, case) in cases {
        if std::panic::catch_unwind(*case).is_err() {
            eprintln!("FAILED #608 {name}");
            failures.push(*name);
        } else {
            println!("test {name} ... ok");
        }
    }
    assert!(failures.is_empty(), "#608 regressions: {failures:?}");
}
