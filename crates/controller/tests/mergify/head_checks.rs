use super::*;

pub(super) fn scenario(settings: &tl3_config::Store, mode: &str) {
    if matches!(mode, "grant-retry" | "request-retry") {
        refused_then_retry(mode);
        return;
    }
    let api = Server::new();
    let root = tempfile::tempdir().unwrap();
    let mut controller = queued(settings, &api, root.path(), true);
    let conclusion = match mode {
        "timed_out" | "startup_failure" | "failure" => mode,
        _ => "cancelled",
    };
    {
        let mut state = api.state.lock().unwrap();
        state.checks = fixture("live-dequeued", "checks");
        state.queue_check_mut()["completed_at"] = json!("2026-10-31T19:40:48Z");
        state.pull["mergeable"] = json!(true);
        state.pull["mergeable_state"] = json!("clean");
        let head = if mode == "otherhead" {
            json!("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb")
        } else {
            state.pull["head"]["sha"].clone()
        };
        state.head_checks = vec![json!({
            "id":900,"name":"Sandbox tests","head_sha":head,
            "status":"completed","conclusion":conclusion,
            "app":{"id":15368,"slug":"github-actions"},"check_suite":{"id":800}
        })];
        state.workflow = Some(json!({"id":700,"check_suite_id":800,"head_sha":head,
            "status":"completed","conclusion":conclusion,"run_attempt":1}));
        state.required = mode != "optional";
        if mode == "permission" || mode.starts_with("forbidden-") {
            state.rerun_status = 403;
        }
        match mode {
            "workflow-running" => state.workflow.as_mut().unwrap()["status"] = json!("in_progress"),
            "forbidden-running" => {
                state.rerun_message =
                    "Cannot rerun a workflow run that is already in progress.".into()
            }
            "forbidden-old" => {
                state.rerun_message =
                    "Cannot rerun a workflow run that is older than 30 days.".into()
            }
            "forbidden-other" => {
                state.rerun_message = "Workflow runs are disabled for this repository.".into()
            }
            _ => {}
        }
        state.rerun_lost_response = mode == "restart";
    }
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
    if mode == "failure" {
        assert_eq!(
            controller.world().unwrap().nodes[0].phase,
            Phase::Making,
            "a required test failure must enter a normal fix round"
        );
        assert_eq!(controller.world().unwrap().nodes[0].round, 1);
        assert_eq!(api.state.lock().unwrap().reruns, 0);
        assert_eq!(api.state.lock().unwrap().posts, 1);
        println!("test head_check_{mode} ... ok");
        return;
    }
    drain(&mut controller);
    if matches!(mode, "workflow-running" | "forbidden-running") {
        assert_eq!(
            api.state.lock().unwrap().posts,
            1,
            "a finishing workflow must not be re-enqueued"
        );
        assert_eq!(
            controller.world().unwrap().nodes[0].phase,
            Phase::Publishing,
            "a finishing workflow must remain pending"
        );
        assert_eq!(
            api.state.lock().unwrap().reruns,
            usize::from(mode == "forbidden-running")
        );
        let pending = controller
            .snapshot()
            .unwrap()
            .pending_effects
            .into_iter()
            .find(|effect| effect.payload["output"]["Effect"]["kind"] == "Enqueue")
            .unwrap();
        let delivery: Delivery = serde_json::from_value(pending.payload).unwrap();
        let mut adapter = api.adapter(&root.path().join("mergify"), 0);
        assert!(matches!(
            adapter.reconcile(&delivery).unwrap(),
            Reconciliation::Pending
        ));
        {
            let mut state = api.state.lock().unwrap();
            state.workflow.as_mut().unwrap()["status"] = json!("completed");
            state.rerun_status = 201;
        }
        controller.look_key(&pending.key).unwrap();
        drain(&mut controller);
    }
    if matches!(mode, "permission" | "forbidden-old" | "forbidden-other") {
        let snapshot = controller.snapshot().unwrap();
        let incident = snapshot
            .rows
            .iter()
            .find(|row| row.table == "incidents")
            .expect("missing Actions write must raise a setup incident");
        let text = incident.value.to_string();
        let expected = match mode {
            "forbidden-old" => "older than 30 days",
            "forbidden-other" => "refused the workflow request",
            _ => "Actions: Read and write",
        };
        assert!(text.contains(expected), "expected {expected}: {incident:?}");
        if mode != "permission" {
            assert!(
                !text.contains("Actions: Read and write"),
                "a workflow restriction is not an App permission failure"
            );
        }
        assert_eq!(api.state.lock().unwrap().posts, 1);
        assert!(matches!(
            controller.world().unwrap().nodes[0].phase,
            Phase::AwaitingRob
        ));
        println!("test head_check_{mode} ... ok");
        return;
    }
    if matches!(mode, "otherhead" | "optional") {
        assert_eq!(
            api.state.lock().unwrap().reruns,
            0,
            "only required checks on this PR head may be rerun"
        );
        println!("test head_check_{mode} ... ok");
        return;
    }
    if mode == "restart" {
        drop(controller);
        let mut registry = adapters::simulation(root.path()).unwrap();
        registry.register(
            Upstream::Mergify,
            api.adapter(&root.path().join("mergify"), 0),
        );
        controller =
            Controller::open(Store::open(settings).unwrap(), registry, bootstrap()).unwrap();
        controller.register_github_translator("mergify", Translator::new(api.clients()).unwrap());
        drain(&mut controller);
    }
    let attempts = if mode == "forbidden-running" { 2 } else { 1 };
    assert_eq!(
        api.state.lock().unwrap().reruns,
        attempts,
        "rerun the cancelled head workflow once"
    );
    assert!(
        !api.state.lock().unwrap().queued_before_rerun,
        "rerun must precede the next queue label write"
    );
    assert_eq!(
        api.state.lock().unwrap().posts,
        1,
        "rerun acceptance must wait for successful head checks before re-enqueueing"
    );
    {
        let mut state = api.state.lock().unwrap();
        state.head_checks[0]["id"] = json!(901);
        state.head_checks[0]["conclusion"] = json!("success");
    }
    controller.retry_held_effects(Upstream::Mergify);
    drain(&mut controller);
    assert_eq!(
        api.state.lock().unwrap().posts,
        2,
        "re-enqueue after the rerun passes"
    );
    assert_eq!(controller.world().unwrap().nodes[0].phase, Phase::Queued);
    {
        let mut state = api.state.lock().unwrap();
        state.head_checks[0]["id"] = json!(902);
        state.head_checks[0]["conclusion"] = json!(conclusion);
        state.checks = fixture("live-dequeued", "checks");
        state.queue_check_mut()["completed_at"] = json!("2026-10-31T19:51:00Z");
        let run = state.workflow.as_mut().unwrap();
        run["status"] = json!("completed");
        run["conclusion"] = json!(conclusion);
    }
    drain(&mut controller);
    assert_eq!(
        api.state.lock().unwrap().reruns,
        attempts,
        "the existing retry bound forbids a second rerun"
    );
    assert_eq!(api.state.lock().unwrap().posts, 2);
    assert!(
        controller
            .snapshot()
            .unwrap()
            .rows
            .iter()
            .any(|row| row.table == "incidents"),
        "the second infrastructure failure must raise the usual incident"
    );
    println!("test head_check_{mode} ... ok");
}

#[derive(Clone)]
pub(super) struct Clock(pub(super) std::rc::Rc<std::cell::Cell<std::time::SystemTime>>);
impl tl3_controller::clock::Clock for Clock {
    fn now(&self) -> std::time::SystemTime {
        self.0.get()
    }
}

pub(super) fn label_present(settings: &tl3_config::Store, lag: bool) {
    let api = Server::new();
    let root = tempfile::tempdir().unwrap();
    drop(queued(settings, &api, root.path(), true));
    let clock = Clock(std::rc::Rc::new(std::cell::Cell::new(
        std::time::SystemTime::now(),
    )));
    let mut registry = adapters::simulation(root.path()).unwrap();
    registry.register(
        Upstream::Mergify,
        api.adapter(&root.path().join("mergify"), 0),
    );
    let mut controller = Controller::open_with_clock(
        Store::open(settings).unwrap(),
        registry,
        bootstrap(),
        clock.clone(),
    )
    .unwrap();
    {
        let mut state = api.state.lock().unwrap();
        state.checks = fixture("live-dequeued", "checks");
        state.pull["mergeable"] = json!(true);
        state.pull["mergeable_state"] = json!("clean");
        assert!(
            state.label,
            "Mergify leaves the queue label on a dequeued PR"
        );
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
    // The queue check can return to ready while the queue rule remains failed.
    // Presence of the old label does not prove that this retry wrote anything.
    {
        let mut state = api.state.lock().unwrap();
        state.checks = fixture("waiting", "checks");
        state.lag = lag;
    }
    drain(&mut controller);
    assert_eq!(
        api.state.lock().unwrap().deletes,
        0,
        "a queue command does not remove the existing label"
    );
    assert_eq!(
        api.state.lock().unwrap().posts,
        2,
        "the retry must send a real queue command"
    );
    if lag {
        assert_eq!(
            controller.world().unwrap().nodes[0].phase,
            Phase::Publishing,
            "a label write alone never confirms queue acceptance"
        );
        for _ in 0..2 {
            clock.0.set(clock.0.get() + Duration::from_secs(100_000));
            controller.look_now().unwrap();
            drain(&mut controller);
        }
        assert!(
            controller
                .snapshot()
                .unwrap()
                .rows
                .iter()
                .any(|row| row.table == "incidents"),
            "missing queue acceptance must raise the usual incident"
        );
        assert_eq!(
            api.state.lock().unwrap().posts,
            2,
            "looks must not repeat the queue mutation"
        );
    } else {
        assert_eq!(controller.world().unwrap().nodes[0].phase, Phase::Queued);
    }
    println!("test head_check_label_present_lag_{lag} ... ok");
}

fn refused_then_retry(mode: &str) {
    let api = Server::new();
    let root = tempfile::tempdir().unwrap();
    let mut delivery = delivery();
    {
        let mut state = api.state.lock().unwrap();
        delivery.head_sha = Some(state.pull["head"]["sha"].as_str().unwrap().into());
        state.checks = fixture("live-dequeued", "checks");
        state.pull["mergeable"] = json!(true);
        state.pull["mergeable_state"] = json!("clean");
        state.head_checks = vec![json!({"id":900,"name":"Sandbox tests",
            "head_sha":delivery.head_sha,"status":"completed","conclusion":"cancelled",
            "app":{"id":15368,"slug":"github-actions"},"check_suite":{"id":800}})];
        state.workflow = Some(
            json!({"id":700,"check_suite_id":800,"head_sha":delivery.head_sha,
            "status":"completed","conclusion":"cancelled","run_attempt":1}),
        );
        state.required = true;
        state.rerun_status = if mode == "grant-retry" { 403 } else { 422 };
    }
    let mut adapter = api.adapter(root.path(), 1);
    assert!(adapter.execute(&delivery).is_err());
    assert_eq!(
        api.state.lock().unwrap().reruns,
        1,
        "the first attempt must reach the refused rerun POST"
    );
    assert_eq!(api.state.lock().unwrap().posts, 0);
    drop(adapter);
    api.state.lock().unwrap().rerun_status = 201;
    let mut adapter = api.adapter(root.path(), 1);
    assert!(
        matches!(
            adapter.reconcile(&delivery).unwrap(),
            Reconciliation::Absent
        ),
        "an explicit refusal must permit the same key to retry after access is granted"
    );
    assert!(adapter.execute(&delivery).is_err());
    assert_eq!(
        api.state.lock().unwrap().posts,
        0,
        "the accepted rerun still needs a successful head check"
    );
    {
        let mut state = api.state.lock().unwrap();
        state.head_checks[0]["id"] = json!(901);
        state.head_checks[0]["conclusion"] = json!("success");
    }
    assert_eq!(adapter.execute(&delivery).unwrap().ack, Ack::Confirmed);
    assert_eq!(
        api.state.lock().unwrap().reruns,
        2,
        "one refused POST and one accepted rerun"
    );
    assert_eq!(api.state.lock().unwrap().posts, 1);
    assert!(matches!(
        adapter.reconcile(&delivery).unwrap(),
        Reconciliation::Found(_)
    ));
    assert_eq!(api.state.lock().unwrap().reruns, 2);
    println!("test head_check_{mode} ... ok");
}
