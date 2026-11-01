use super::*;

fn open(
    settings: &tl3_config::Store,
    api: &Server,
    root: &Path,
    clock: head_checks::Clock,
) -> Controller {
    let mut registry = adapters::simulation(root).unwrap();
    registry.register(Upstream::Mergify, api.adapter(&root.join("mergify"), 0));
    let mut controller =
        Controller::open_with_clock(Store::open(settings).unwrap(), registry, bootstrap(), clock)
            .unwrap();
    controller.register_github_translator("mergify", Translator::new(api.clients()).unwrap());
    controller
}

pub(super) fn scenario(settings: &tl3_config::Store, mode: &str) {
    let api = Server::new();
    let root = tempfile::tempdir().unwrap();
    drop(queued(settings, &api, root.path(), true));
    let clock = head_checks::Clock(std::rc::Rc::new(std::cell::Cell::new(
        std::time::SystemTime::now(),
    )));
    let mut controller = open(settings, &api, root.path(), clock.clone());
    if mode == "comment-first" {
        api.state.lock().unwrap().comments.push(json!({
            "id":999,"body":"Left the queue · checks interrupted",
            "user":{"id":37929162,"login":"mergify[bot]"},
            "created_at":"2026-10-31T22:19:33Z","updated_at":"2026-10-31T22:19:33Z",
            "issue_url":"local"
        }));
        ingress::apply(&mut controller, tl3_controller::cli::Input::GitHubWake).unwrap();
        drain(&mut controller);
        assert_eq!(
            api.state.lock().unwrap().requeues,
            0,
            "a comment alone is not a completed dequeue"
        );
    }
    {
        let mut state = api.state.lock().unwrap();
        state.lag = true;
        state
            .events
            .push(label_event(20, "queued", "2026-10-31T22:18:30Z"));
        if mode == "stale-label" {
            state.dequeued = true;
            state
                .events
                .push(label_event(10, "dequeued", "2026-10-31T22:18:00Z"));
        }
        if mode == "label-first" {
            state.dequeued = true;
            state
                .events
                .push(label_event(30, "dequeued", "2026-10-31T22:19:33Z"));
            state.requeue_order.push("dequeued");
            state.queue_rule["status"] = json!("in_progress");
            state.queue_rule["conclusion"] = Value::Null;
        }
        state.checks = fixture("interrupted", "checks");
        state.queue_check_mut()["completed_at"] = json!("2026-10-31T22:19:34Z");
        // A concluded rule from the preceding queue attempt is not completion evidence.
        state.queue_rule["completed_at"] = json!("2026-10-31T22:18:00Z");
        state.pull["mergeable"] = json!(true);
        state.pull["mergeable_state"] = json!("clean");
    }
    let mut body = webhook_fixture();
    body["action"] = json!("completed");
    body["check_run"] = api.state.lock().unwrap().queue_check();
    for input in Translator::new(api.clients())
        .unwrap()
        .webhook(
            &parsed("check_run", body),
            &controller.snapshot().unwrap(),
            &bootstrap().repos,
        )
        .unwrap()
    {
        ingress::apply(&mut controller, input).unwrap();
    }
    drain(&mut controller);
    assert_eq!(
        api.state.lock().unwrap().posts,
        if mode == "label-first" { 2 } else { 1 },
        "a dequeue comment and queue-check update must not trigger an early requeue"
    );
    assert_eq!(
        controller.world().unwrap().nodes[0].phase,
        Phase::Publishing
    );
    let key = controller
        .snapshot()
        .unwrap()
        .pending_effects
        .into_iter()
        .find(|effect| effect.payload["output"]["Effect"]["kind"] == "Enqueue")
        .unwrap()
        .key;
    drop(controller);
    let mut controller = open(settings, &api, root.path(), clock.clone());
    drain(&mut controller);
    assert_eq!(
        api.state.lock().unwrap().posts,
        if mode == "label-first" { 2 } else { 1 },
        "restart must preserve the wait"
    );
    if mode == "stale-label" {
        api.state.lock().unwrap().dequeued = false;
        ingress::apply(&mut controller, tl3_controller::cli::Input::GitHubWake).unwrap();
        drain(&mut controller);
        assert_eq!(
            api.state.lock().unwrap().requeues,
            0,
            "removing a stale label is not dequeue completion"
        );
    }
    if mode != "label-first" {
        let mut state = api.state.lock().unwrap();
        if mode == "delayed-rule" {
            state.queue_rule["completed_at"] = json!("2026-10-31T22:20:02Z");
            state.requeue_order.push("rule completed");
        } else {
            state.dequeued = true;
            state
                .events
                .push(label_event(30, "dequeued", "2026-10-31T22:20:03Z"));
            state.queue_rule["status"] = json!("in_progress");
            state.queue_rule["conclusion"] = Value::Null;
            state.queue_rule["completed_at"] = Value::Null;
            state.requeue_order.push("dequeued");
        }
        state.lag = true;
        state.command_lost_response = mode == "lost-response";
        state.command_status = match mode {
            "refusal" | "refusal-bound" => 409,
            "rate-limit" => 429,
            "permission" => 403,
            _ => 201,
        };
    }
    ingress::apply(&mut controller, tl3_controller::cli::Input::GitHubWake).unwrap();
    drain(&mut controller);
    if matches!(
        mode,
        "refusal" | "refusal-bound" | "rate-limit" | "permission"
    ) {
        refused(&mut controller, &api, &clock, &key, mode);
        println!("test requeue_{mode} ... ok");
        return;
    }
    assert_eq!(
        api.state.lock().unwrap().requeues,
        1,
        "post one App queue command"
    );
    assert_eq!(
        api.state.lock().unwrap().deletes,
        0,
        "a command does not toggle labels"
    );
    assert_eq!(
        api.state.lock().unwrap().requeue_order,
        [
            if mode == "delayed-rule" {
                "rule completed"
            } else {
                "dequeued"
            },
            "queue"
        ]
    );
    assert_eq!(
        controller.world().unwrap().nodes[0].phase,
        Phase::Publishing,
        "a command receipt is not queue acceptance"
    );
    drop(controller);
    let mut controller = open(settings, &api, root.path(), clock.clone());
    drain(&mut controller);
    assert_eq!(
        api.state.lock().unwrap().requeues,
        1,
        "restart must preserve command dispatch"
    );
    if mode == "no-acceptance" {
        for _ in 0..2 {
            clock.0.set(clock.0.get() + Duration::from_secs(100_000));
            controller.look_now().unwrap();
            drain(&mut controller);
        }
    } else {
        api.state.lock().unwrap().checks = fixture("running", "checks");
        controller.look_key(&key).unwrap();
        drain(&mut controller);
        assert_eq!(controller.world().unwrap().nodes[0].phase, Phase::Queued);
        let mut state = api.state.lock().unwrap();
        state.checks = fixture("interrupted", "checks");
        state.queue_check_mut()["completed_at"] = json!("2026-10-31T22:40:00Z");
        drop(state);
        drain(&mut controller);
    }
    assert!(
        controller
            .snapshot()
            .unwrap()
            .rows
            .iter()
            .any(|row| row.table == "incidents"),
        "missing acceptance or a second dequeue must raise the usual incident"
    );
    assert_eq!(
        api.state.lock().unwrap().requeues,
        1,
        "the retry allowance bounds requeues"
    );
    println!("test requeue_{mode} ... ok");
}

fn label_event(id: u64, label: &str, time: &str) -> Value {
    json!({"id":id,"event":"labeled","actor":{"id":37929162,"login":"mergify[bot]"},
        "label":{"name":label},"created_at":time})
}

fn refused(
    controller: &mut Controller,
    api: &Server,
    clock: &head_checks::Clock,
    key: &str,
    mode: &str,
) {
    assert_eq!(api.state.lock().unwrap().command_attempts, 1);
    assert_eq!(
        api.state.lock().unwrap().requeues,
        0,
        "a refused command was not accepted"
    );
    if mode == "permission" {
        let snapshot = controller.snapshot().unwrap();
        let incident = snapshot
            .rows
            .iter()
            .find(|row| row.table == "incidents" && row.value["open"] == true)
            .expect("a command permission refusal must raise an incident");
        let summary = incident.value["summary"].as_str().unwrap();
        assert!(
            summary.contains("GitHub App") && summary.contains("Issues: Read and write"),
            "{summary}"
        );
        drain(controller);
        assert_eq!(api.state.lock().unwrap().command_attempts, 1);
        return;
    }
    if mode != "refusal-bound" {
        let mut state = api.state.lock().unwrap();
        state.command_status = 201;
        state.lag = false;
    }
    clock.0.set(clock.0.get() + Duration::from_secs(2));
    controller.look_key(key).unwrap();
    drain(controller);
    assert_eq!(
        api.state.lock().unwrap().command_attempts,
        2,
        "a definitive refusal permits another attempt"
    );
    if mode == "refusal-bound" {
        let pending = controller
            .snapshot()
            .unwrap()
            .pending_effects
            .into_iter()
            .find(|effect| effect.payload["output"]["Effect"]["kind"] == "Enqueue")
            .unwrap();
        controller.look_key(&pending.key).unwrap();
        drain(controller);
        assert!(
            controller
                .snapshot()
                .unwrap()
                .rows
                .iter()
                .any(|row| row.table == "incidents")
        );
        assert_eq!(
            api.state.lock().unwrap().command_attempts,
            2,
            "refusals exhaust the existing retry allowance"
        );
    } else {
        assert_eq!(api.state.lock().unwrap().requeues, 1);
        assert_eq!(controller.world().unwrap().nodes[0].phase, Phase::Queued);
    }
}
