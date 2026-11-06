use super::*;

pub(super) struct Fixture {
    parsed: bool,
    draft: Option<Value>,
    checks: Option<Value>,
}
impl Fixture {
    pub(super) fn respond(&self, path: &str) -> Option<ResponseTemplate> {
        let body: Value = if path.ends_with("/pulls/3331") {
            self.draft.clone().unwrap_or_else(|| {
                serde_json::from_str(include_str!("../fixtures/mergify/pq3221/draft.json")).unwrap()
            })
        } else if path.contains("/commits/4a3d6c618043e1875de665093edfe83f87f255ea/") {
            self.checks.clone().unwrap_or_else(|| {
                serde_json::from_str(include_str!("../fixtures/mergify/pq3221/draft-checks.json"))
                    .unwrap()
            })
        } else if path.ends_with("/pulls/100/files") {
            json!([{"filename":"src/change.rs"}])
        } else if path.contains("/actions/jobs/") && path.ends_with("/logs") {
            return Some(ResponseTemplate::new(200).set_body_string(if self.parsed {
                "FAILED: tests/queue_cause.py::test_gate"
            } else {
                "plan refused at planning; upload refused\nProcess completed with exit code 75."
            }));
        } else {
            return None;
        };
        let mut body = body;
        if path.ends_with("/pulls/3331")
            && let Some(text) = body["body"].as_str()
        {
            body["body"] = json!(text.replace("number: 3330", "number: 100"));
        }
        Some(ResponseTemplate::new(200).set_body_json(body))
    }
}

/// #854b / Q50: real PQ #3221 queue refusal must hold without a failed round.
pub(super) const MODES: &[&str] = &[
    "hold",
    "restart",
    "witness-pass",
    "retry-fail-fast",
    "retry-making",
    "timeout",
    "timeout-churn",
    "own-batch",
    "in-progress",
    "change",
    "q58",
    "positive-environment",
    "gate-witness-pass",
    "q58-after-hold",
    "restart-witness",
];

fn observe(
    controller: &mut Controller,
    api: &Server,
    config: &Bootstrap,
    body: Value,
    event: &str,
) {
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
    for _ in 0..12 {
        controller.drive_once().unwrap();
        if matches!(
            controller.world().unwrap().nodes[0].phase,
            Phase::Making | Phase::Stopping(_) | Phase::Closed(_)
        ) {
            break;
        }
    }
}

fn failure(controller: &mut Controller, api: &Server, config: &Bootstrap) {
    let mut body = webhook_fixture();
    body["action"] = json!("completed");
    body["check_run"] = api.state.lock().unwrap().queue_check();
    observe(controller, api, config, body, "check_run");
}

// A real merge webhook on the held PR's destination, after the failure, advances
// that branch to this commit. No health endpoint or additional polling is used.
fn health(controller: &mut Controller, api: &Server, config: &Bootstrap, at: &str, sha: &str) {
    let mut body = webhook_fixture();
    let mut pull = api.state.lock().unwrap().pull.clone();
    pull["number"] = json!(101);
    pull["head"]["sha"] = json!("cccccccccccccccccccccccccccccccccccccccc");
    pull["base"]["ref"] = json!("main");
    pull["state"] = json!("closed");
    pull["merged"] = json!(true);
    pull["merged_at"] = json!(at);
    pull["merge_commit_sha"] = json!(sha);
    body["pull_request"] = pull;
    body["action"] = json!("closed");
    // Deliver this gate before allowing background watch reads. The next
    // queued failure can then be delivered explicitly by the test schedule.
    for input in Translator::new(api.clients())
        .unwrap()
        .webhook(
            &parsed("pull_request", body),
            &controller.snapshot().unwrap(),
            &config.repos,
        )
        .unwrap()
    {
        ingress::apply(controller, input).unwrap();
    }
    controller.drive_once().unwrap();
}

pub(super) fn scenario(settings: &tl3_config::Store, mode: &str) {
    if mode == "change" {
        queue_entry::scenario(settings, "repair");
        return;
    }
    if mode == "in-progress" {
        queue_entry::scenario(settings, "queue-running");
        return;
    }
    if mode == "q58" {
        ci_evidence::scenario(settings, "repeat");
        return;
    }
    let api = Server::new();
    let root = tempfile::tempdir().unwrap();
    let mut config = bootstrap();
    config.fail_fast = mode != "retry-making";
    let clock = head_checks::Clock(std::rc::Rc::new(std::cell::Cell::new(
        std::time::SystemTime::now(),
    )));
    let mut controller = queued_for_clock(
        settings,
        &api,
        root.path(),
        true,
        &[],
        config.clone(),
        Some(clock.clone()),
    );
    // PQ's failed fix round had fail_fast enabled; preserve that decision state.
    let snapshot = controller.snapshot().unwrap();
    let row = snapshot
        .rows
        .iter()
        .find(|row| row.table == "nodes" && row.id == "0")
        .unwrap();
    let mut value = row.value.clone();
    value["round"] = json!(1);
    let mut seed = tl3_store::Commit::new("pq-fix-round", json!({}), &snapshot);
    seed.rows.push(tl3_store::RowChange::Put {
        table: "nodes".into(),
        id: "0".into(),
        value,
    });
    Store::open(settings).unwrap().commit(&seed).unwrap();
    let before = controller.world().unwrap().nodes[0];
    {
        let mut state = api.state.lock().unwrap();
        let head = state.pull["head"]["sha"].clone();
        let mut captured: Value =
            serde_json::from_str(include_str!("../fixtures/mergify/pq3221/checks.json")).unwrap();
        for check in captured["check_runs"].as_array_mut().unwrap() {
            check["head_sha"] = head.clone();
            check["pull_requests"] = json!([]);
        }
        state.checks = captured["check_runs"].clone();
        state.queue_rule["conclusion"] = json!("success");
        state.pull["base"]["sha"] = json!("ef817b9d48f4e2f55a3b7d8e83caef25573ad544");
        state.cause_fixture = Some(Fixture {
            parsed: mode == "positive-environment",
            draft: None,
            checks: None,
        });
    }
    if mode == "own-batch" {
        health(
            &mut controller,
            &api,
            &config,
            "2026-11-05T01:00:00Z",
            "ef817b9d48f4e2f55a3b7d8e83caef25573ad544",
        );
    }
    if mode != "own-batch" {
        let prior = serde_json::from_str::<Value>(include_str!(
            "../fixtures/mergify/pq3221/prior-drafts.json"
        ))
        .unwrap();
        let mut checks = serde_json::from_str::<Value>(include_str!(
            "../fixtures/mergify/pq3221/prior-checks.json"
        ))
        .unwrap();
        if mode == "positive-environment" {
            for check in checks["check_runs"].as_array_mut().unwrap() {
                if check["conclusion"] == "failure" {
                    check["output"]["summary"] = json!("FAILED: tests/queue_cause.py::test_gate");
                }
            }
        }
        let changes = serde_json::from_value(json!({"repository":"owner/sandbox", "issues":[], "comments":[], "labels":[], "pull_requests":prior, "check_runs":checks["check_runs"], "check_suites":[]})).unwrap();
        for input in Translator::new(api.clients())
            .unwrap()
            .catch_up(&config.repos[0], &changes, &controller.snapshot().unwrap())
            .unwrap()
        {
            ingress::apply(&mut controller, input).unwrap();
        }
    }
    failure(&mut controller, &api, &config);
    if mode == "own-batch" {
        drain(&mut controller);
        assert_eq!(
            controller.world().unwrap().nodes[0].phase,
            Phase::Closed(Outcome::Stopped)
        );
        println!("test queue_cause_{mode} ... ok");
        return;
    }
    let after = controller.world().unwrap().nodes[0];
    assert_eq!(
        after.round, before.round,
        "opaque PQ refusal must hold, never consume a fix round"
    );
    assert_eq!(after.credit.spent, before.credit.spent);
    assert_eq!(
        after.phase,
        Phase::Queued,
        "FIRST opaque failure holds even with fail_fast enabled"
    );
    assert_eq!(api.state.lock().unwrap().requeues, 0);
    assert!(
        controller
            .snapshot()
            .unwrap()
            .rows
            .iter()
            .any(|row| row.table == "queue_holds")
    );
    assert_eq!(api.state.lock().unwrap().pull["state"], "open");
    let facts = queue_facts(&mut controller);
    assert!(facts.iter().any(|fact| if mode == "positive-environment" {
        fact["evidence"]["CiFailed"]["details"]["cause"]
            .get("Environment")
            .is_some()
    } else {
        fact["evidence"]["CiFailed"]["details"]["cause"] == "Unattributed"
    }));
    if matches!(mode, "restart" | "restart-witness") {
        drop(controller);
        let mut registry = adapters::simulation(root.path()).unwrap();
        registry.register(
            Upstream::Mergify,
            api.adapter(&root.path().join("mergify"), 0)
                .with_store(settings.clone())
                .with_clock(clock.clone()),
        );
        controller = Controller::open_with_clock(
            Store::open(settings).unwrap(),
            registry,
            config.clone(),
            clock.clone(),
        )
        .unwrap();
        controller.look_now().unwrap();
        failure(&mut controller, &api, &config);
        assert_eq!(controller.world().unwrap().nodes[0].phase, Phase::Queued);
        assert_eq!(api.state.lock().unwrap().requeues, 0);
    }
    if matches!(mode, "timeout" | "timeout-churn") {
        let mut allowances = tl3_config::Allowances::default();
        allowances.github_mergify.no_progress_secs = 3;
        allowances.github_mergify.silence_secs = 1;
        controller.set_allowances(allowances);
        clock.0.set(clock.0.get() + Duration::from_secs(3600));
        if mode == "timeout-churn" {
            // Another opaque result on the due look must not reset the hold.
            let mut state = api.state.lock().unwrap();
            state.queue_check_mut()["id"] = json!(99001);
            state.queue_check_mut()["completed_at"] = json!("2026-11-06T23:30:00Z");
        }
        controller.look_now().unwrap();
        for _ in 0..12 {
            controller.drive_once().unwrap();
        }
        let snapshot = controller.snapshot().unwrap();
        let incident = snapshot
            .rows
            .iter()
            .find(|row| {
                row.table == "incidents" && row.value["signature"]["class"] == "queue_environment"
            })
            .expect("bounded hold must raise a Q51 environment incident");
        assert_eq!(incident.value["cause"], "tl3");
        assert_eq!(incident.value["needs_rob"], false);
        assert_eq!(controller.world().unwrap().nodes[0].phase, Phase::Queued);
        assert_eq!(
            controller.world().unwrap().nodes[0].credit.spent,
            before.credit.spent
        );
    }
    if matches!(
        mode,
        "witness-pass"
            | "retry-fail-fast"
            | "retry-making"
            | "gate-witness-pass"
            | "q58-after-hold"
            | "restart-witness"
    ) {
        // Earlier or same-base merges are not a new-base health witness.
        health(
            &mut controller,
            &api,
            &config,
            "2026-11-05T01:00:00Z",
            "ef817b9d48f4e2f55a3b7d8e83caef25573ad544",
        );
        controller.look_now().unwrap();
        assert_eq!(api.state.lock().unwrap().requeues, 0);
        if mode == "gate-witness-pass" {
            let mut prior: Value =
                serde_json::from_str(include_str!("../fixtures/mergify/pq3221/prior-drafts.json"))
                    .unwrap();
            for pull in prior.as_array_mut().unwrap() {
                pull["base"]["sha"] = json!("ffffffffffffffffffffffffffffffffffffffff");
                let body = pull["body"].as_str().unwrap().replace(
                    "ef817b9d48f4e2f55a3b7d8e83caef25573ad544",
                    "ffffffffffffffffffffffffffffffffffffffff",
                );
                pull["body"] = json!(body);
            }
            let mut checks: Value =
                serde_json::from_str(include_str!("../fixtures/mergify/pq3221/prior-checks.json"))
                    .unwrap();
            for check in checks["check_runs"].as_array_mut().unwrap() {
                check["status"] = json!("completed");
                check["conclusion"] = json!("success");
                check["completed_at"] = json!("2026-11-06T03:00:00Z");
            }
            let changes = serde_json::from_value(json!({"repository":"owner/sandbox", "issues":[], "comments":[], "labels":[], "pull_requests":prior, "check_runs":checks["check_runs"], "check_suites":[]})).unwrap();
            for input in Translator::new(api.clients())
                .unwrap()
                .catch_up(&config.repos[0], &changes, &controller.snapshot().unwrap())
                .unwrap()
            {
                ingress::apply(&mut controller, input).unwrap();
            }
            for _ in 0..12 {
                controller.drive_once().unwrap();
            }
        } else {
            health(
                &mut controller,
                &api,
                &config,
                "2026-11-06T03:00:00Z",
                "ffffffffffffffffffffffffffffffffffffffff",
            );
        }
        controller.look_now().unwrap();
        controller.look_now().unwrap();
        assert_eq!(
            api.state.lock().unwrap().requeues,
            1,
            "one command after health, including duplicate looks"
        );
        if matches!(
            mode,
            "witness-pass" | "gate-witness-pass" | "restart-witness"
        ) {
            {
                let mut state = api.state.lock().unwrap();
                state.merged(false);
                let head = state.pull["head"]["sha"].clone();
                state.checks = fixture("merged", "checks");
                for check in state.checks.as_array_mut().unwrap() {
                    check["head_sha"] = head.clone();
                }
            }
            failure(&mut controller, &api, &config);
            drain(&mut controller);
            assert_eq!(
                controller.world().unwrap().nodes[0].phase,
                Phase::Closed(Outcome::Merged)
            );
        } else {
            {
                let mut state = api.state.lock().unwrap();
                let head = state.pull["head"]["sha"].clone();
                let captured: Value =
                    serde_json::from_str(include_str!("../fixtures/mergify/pq3221/checks.json"))
                        .unwrap();
                state.checks = captured["check_runs"].clone();
                for check in state.checks.as_array_mut().unwrap() {
                    check["head_sha"] = head.clone();
                    check["pull_requests"] = json!([]);
                }
                state.queue_check_mut()["completed_at"] = json!("2026-11-06T04:00:00Z");
                if mode == "q58-after-hold" {
                    state.cause_fixture.as_mut().unwrap().parsed = true;
                }
            }
            failure(&mut controller, &api, &config);
            let facts = queue_facts(&mut controller);
            assert!(
                facts
                    .iter()
                    .any(|fact| fact["evidence"]["CiFailed"]["details"]["cause"]
                        .get("Change")
                        .is_some()),
                "second failure must retain typed Change evidence"
            );
            let text = serde_json::to_string(&facts).unwrap();
            assert!(text.contains("Remote Bazel selection and shards"));
            let findings = Store::open(settings)
                .unwrap()
                .worker_document("0", "maker_findings")
                .unwrap()
                .unwrap();
            assert!(
                findings
                    .to_string()
                    .contains("Remote Bazel selection and shards"),
                "both Q54 outcomes retain actual failing checks"
            );
            if config.fail_fast {
                drain(&mut controller);
            }
            let phase = controller.world().unwrap().nodes[0].phase;
            assert_eq!(
                phase,
                if config.fail_fast {
                    Phase::Closed(Outcome::Stopped)
                } else {
                    Phase::Making
                },
                "SECOND failure is Change and follows today's Q54 failed round"
            );
        }
        assert_eq!(api.state.lock().unwrap().requeues, 1);
    }
    println!("test queue_cause_{mode} ... ok");
}

// Real independent-batch catch-up, with only the named check identities green.
fn batch_health(
    controller: &mut Controller,
    api: &Server,
    config: &Bootstrap,
    names: &[&str],
    base: &str,
    branch: &str,
) {
    let mut pulls: Value =
        serde_json::from_str(include_str!("../fixtures/mergify/pq3221/prior-drafts.json")).unwrap();
    for pull in pulls.as_array_mut().unwrap() {
        pull["base"]["sha"] = json!(base);
        pull["base"]["ref"] = json!(branch);
        pull["body"] = json!(
            pull["body"]
                .as_str()
                .unwrap()
                .replace("ef817b9d48f4e2f55a3b7d8e83caef25573ad544", base)
        );
    }
    let checks: Value =
        serde_json::from_str(include_str!("../fixtures/mergify/pq3221/prior-checks.json")).unwrap();
    let template = checks["check_runs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["conclusion"] == "failure")
        .unwrap();
    let checks: Vec<Value> = names
        .iter()
        .enumerate()
        .map(|(i, name)| {
            let mut c = template.clone();
            c["id"] = json!(900000 + i);
            c["name"] = json!(name);
            c["status"] = json!("completed");
            c["conclusion"] = json!("success");
            c["completed_at"] = json!("2026-11-06T03:00:00Z");
            c
        })
        .collect();
    let changes=serde_json::from_value(json!({"repository":"owner/sandbox","issues":[],"comments":[],"labels":[],"pull_requests":pulls,"check_runs":checks,"check_suites":[]})).unwrap();
    for input in Translator::new(api.clients())
        .unwrap()
        .catch_up(&config.repos[0], &changes, &controller.snapshot().unwrap())
        .unwrap()
    {
        ingress::apply(controller, input).unwrap();
    }
    controller.drive_once().unwrap();
}

pub(super) const REVIEW_MODES: &[&str] = &[
    "witness-before",
    "before-post",
    "paused-before-post",
    "retry-no-time",
    "incomplete-reporters",
    "lint-only-health",
    "reporter-overflow",
    "retargeted",
    "terminal-hold",
    "gate-retention",
    "retry-no-time-restart",
    "reporter-union",
    "reporter-revalidate",
];

/// #854b r1: real Controller/adapter schedules requested by the review.
pub(super) fn review(settings: &tl3_config::Store, mode: &str) {
    let api = Server::new();
    let root = tempfile::tempdir().unwrap();
    let mut config = bootstrap();
    config.fail_fast = true;
    let clock = head_checks::Clock(std::rc::Rc::new(std::cell::Cell::new(
        std::time::SystemTime::now(),
    )));
    let mut controller = queued_for_clock(
        settings,
        &api,
        root.path(),
        true,
        &[],
        config.clone(),
        Some(clock.clone()),
    );
    let snapshot = controller.snapshot().unwrap();
    let row = snapshot
        .rows
        .iter()
        .find(|r| r.table == "nodes" && r.id == "0")
        .unwrap();
    let mut value = row.value.clone();
    value["round"] = json!(1);
    let mut seed = tl3_store::Commit::new("r1-fix-round", json!({}), &snapshot);
    seed.rows.push(tl3_store::RowChange::Put {
        table: "nodes".into(),
        id: "0".into(),
        value,
    });
    Store::open(settings).unwrap().commit(&seed).unwrap();
    {
        let mut state = api.state.lock().unwrap();
        let head = state.pull["head"]["sha"].clone();
        let mut checks: Value =
            serde_json::from_str(include_str!("../fixtures/mergify/pq3221/checks.json")).unwrap();
        for check in checks["check_runs"].as_array_mut().unwrap() {
            check["head_sha"] = head.clone();
            check["pull_requests"] = json!([]);
        }
        state.checks = checks["check_runs"].clone();
        state.queue_rule["conclusion"] = json!("success");
        state.pull["base"]["sha"] = json!("ef817b9d48f4e2f55a3b7d8e83caef25573ad544");
        state.cause_fixture = Some(Fixture {
            parsed: false,
            draft: None,
            checks: None,
        });
    }
    match mode {
        "reporter-union" | "reporter-revalidate" => {
            // #854b r2.A: complete A first, then A plus unidentified B.
            let set_reporters = |unidentified: bool| {
                let mut checks: Value = serde_json::from_str(include_str!(
                    "../fixtures/mergify/pq3221/draft-checks.json"
                ))
                .unwrap();
                let mut a = checks["check_runs"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|c| c["conclusion"] == "failure")
                    .unwrap()
                    .clone();
                a["name"] = json!("A");
                let mut b = a.clone();
                b["name"] = json!("B");
                b["id"] = json!(99101);
                b["app"] = Value::Null;
                checks["check_runs"] = if unidentified { json!([a, b]) } else { json!([a]) };
                let mut state = api.state.lock().unwrap();
                state.cause_fixture.as_mut().unwrap().checks = Some(checks);
                if unidentified {
                    state.queue_check_mut()["id"] = json!(99102);
                }
            };
            set_reporters(false);
            failure(&mut controller, &api, &config);
            let before = controller.snapshot().unwrap();
            let original = before.rows.iter().find(|r| r.table == "queue_holds").unwrap();
            assert_eq!(original.value["reporters_complete"], true);
            assert_eq!(original.value["reporters"].as_array().unwrap().len(), 1);
            if mode == "reporter-revalidate" {
                batch_health(&mut controller, &api, &config, &["A"],
                    "ffffffffffffffffffffffffffffffffffffffff", "main");
                assert!(controller.snapshot().unwrap().rows.iter().any(|r|
                    r.table == "queue_holds" && !r.value["witness"].is_null()));
            }
            set_reporters(true);
            failure(&mut controller, &api, &config);
            if mode == "reporter-union" {
                batch_health(&mut controller, &api, &config, &["A"],
                    "ffffffffffffffffffffffffffffffffffffffff", "main");
            }
            controller.look_now().unwrap();
            controller.look_now().unwrap();
            assert_eq!(api.state.lock().unwrap().requeues, 0,
                "r2.A: a later unidentified reporter must invalidate A-only health");
            let after = controller.snapshot().unwrap();
            let hold = after.rows.iter().find(|r| r.table == "queue_holds").unwrap();
            assert!(hold.value["witness"].is_null());
            assert_eq!(hold.value["reporters_complete"], false);
            assert_eq!(hold.value["since_ms"], original.value["since_ms"]);
            assert_eq!(hold.value["retry"], original.value["retry"]);
            assert_eq!(controller.world().unwrap().nodes[0].phase, Phase::Queued);
            assert_eq!(controller.world().unwrap().nodes[0].credit.spent, 0);
        }
        "witness-before" => {
            // T2 is delivered first; the later-delivered failed check completed at T1<T2.
            health(
                &mut controller,
                &api,
                &config,
                "2026-11-06T03:00:00Z",
                "ffffffffffffffffffffffffffffffffffffffff",
            );
            failure(&mut controller, &api, &config);
            let snap = controller.snapshot().unwrap();
            let hold = snap
                .rows
                .iter()
                .find(|r| r.table == "queue_holds")
                .expect("hold");
            assert!(
                !hold.value["witness"].is_null(),
                "stored T2 gate must witness the delayed T1 failure"
            );
            controller.look_now().unwrap();
            controller.look_now().unwrap();
            assert_eq!(
                api.state.lock().unwrap().requeues,
                1,
                "r1.1: journaled T2 witness must recover delayed T1 failure"
            );
        }
        "before-post" | "paused-before-post" | "retry-no-time" | "retry-no-time-restart" => {
            failure(&mut controller, &api, &config);
            assert_eq!(controller.world().unwrap().nodes[0].phase, Phase::Queued);
            if mode == "paused-before-post" {
                controller
                    .observe(
                        "r1-pause",
                        tl3_controller::executor::Observation::Steering {
                            node: NodeId(0),
                            command: tl3_kernel::Command::Pause,
                        },
                    )
                    .unwrap();
                for _ in 0..12 {
                    controller.drive_once().unwrap();
                }
                assert!(controller.world().unwrap().nodes[0].paused);
            }
            health(
                &mut controller,
                &api,
                &config,
                "2026-11-06T03:00:00Z",
                "ffffffffffffffffffffffffffffffffffffffff",
            );
            if mode.starts_with("retry-no-time") {
                controller.look_now().unwrap();
                controller.look_now().unwrap();
                assert_eq!(
                    api.state.lock().unwrap().requeues,
                    1,
                    "test requires an actual POST"
                );
                let snap = controller.snapshot().unwrap();
                assert!(
                    snap.rows
                        .iter()
                        .any(|r| r.table == "queue_holds" && !r.value["retry"].is_null()),
                    "Sent receipt must publish the retry transition"
                );
                if mode == "retry-no-time-restart" {
                    drop(controller);
                    let mut registry = adapters::simulation(root.path()).unwrap();
                    registry.register(
                        Upstream::Mergify,
                        api.adapter(&root.path().join("mergify"), 0)
                            .with_store(settings.clone())
                            .with_clock(clock.clone()),
                    );
                    controller = Controller::open_with_clock(
                        Store::open(settings).unwrap(),
                        registry,
                        config.clone(),
                        clock.clone(),
                    )
                    .unwrap();
                }
                let mut state = api.state.lock().unwrap();
                let head = state.pull["head"]["sha"].clone();
                let captured: Value =
                    serde_json::from_str(include_str!("../fixtures/mergify/pq3221/checks.json"))
                        .unwrap();
                state.checks = captured["check_runs"].clone();
                for check in state.checks.as_array_mut().unwrap() {
                    check["head_sha"] = head.clone();
                    check["pull_requests"] = json!([]);
                }
                state.queue_check_mut()["id"] = json!(99022);
                state.queue_check_mut()["completed_at"] = Value::Null;
            } else {
                assert_eq!(
                    api.state.lock().unwrap().requeues,
                    0,
                    "failure precedes our POST"
                );
                let mut state = api.state.lock().unwrap();
                state.queue_check_mut()["id"] = json!(99021);
                state.queue_check_mut()["completed_at"] = json!("2026-11-06T04:00:00Z");
            }
            failure(&mut controller, &api, &config);
            if mode.starts_with("retry-no-time") {
                drain(&mut controller);
            }
            assert_eq!(
                controller.world().unwrap().nodes[0].phase,
                if mode.starts_with("retry-no-time") {
                    Phase::Closed(Outcome::Stopped)
                } else {
                    Phase::Queued
                },
                "r1.2: only a failure bound to our dispatched retry is Change, regardless of timestamps"
            );
            if !mode.starts_with("retry-no-time") {
                assert_eq!(api.state.lock().unwrap().requeues, 0);
                assert_eq!(controller.world().unwrap().nodes[0].credit.spent, 0);
            }
        }
        "incomplete-reporters" | "reporter-overflow" => {
            {
                let mut checks: Value = serde_json::from_str(include_str!(
                    "../fixtures/mergify/pq3221/draft-checks.json"
                ))
                .unwrap();
                let template = checks["check_runs"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|c| c["conclusion"] == "failure")
                    .unwrap()
                    .clone();
                let mut a = template.clone();
                a["name"] = json!("A");
                let mut b = template;
                b["name"] = json!("B");
                b["id"] = json!(99033);
                b["app"] = Value::Null;
                checks["check_runs"] = if mode == "reporter-overflow" {
                    json!(
                        (0..21)
                            .map(|i| {
                                let mut c = a.clone();
                                c["id"] = json!(99040 + i);
                                c["name"] = json!(format!("C{i:02}"));
                                c
                            })
                            .collect::<Vec<_>>()
                    )
                } else {
                    json!([a, b])
                };
                api.state
                    .lock()
                    .unwrap()
                    .cause_fixture
                    .as_mut()
                    .unwrap()
                    .checks = Some(checks);
            }
            failure(&mut controller, &api, &config);
            let names: Vec<String> = if mode == "reporter-overflow" {
                (0..20).map(|i| format!("C{i:02}")).collect()
            } else {
                vec!["A".into()]
            };
            batch_health(
                &mut controller,
                &api,
                &config,
                &names.iter().map(String::as_str).collect::<Vec<_>>(),
                "ffffffffffffffffffffffffffffffffffffffff",
                "main",
            );
            controller.look_now().unwrap();
            controller.look_now().unwrap();
            assert_eq!(
                api.state.lock().unwrap().requeues,
                0,
                "r1.3: A-only batch cannot witness unidentified B"
            );
            let snap = controller.snapshot().unwrap();
            let hold = snap.rows.iter().find(|r| r.table == "queue_holds").unwrap();
            assert!(hold.value["witness"].is_null());
        }
        "lint-only-health" => {
            batch_health(
                &mut controller,
                &api,
                &config,
                &["lint"],
                "ef817b9d48f4e2f55a3b7d8e83caef25573ad544",
                "main",
            );
            failure(&mut controller, &api, &config);
            assert_eq!(
                controller.world().unwrap().nodes[0].phase,
                Phase::Queued,
                "r1.4: lint does not prove our failing identities healthy"
            );
            assert_eq!(api.state.lock().unwrap().requeues, 0);
            assert_eq!(controller.world().unwrap().nodes[0].credit.spent, 0);
        }
        "retargeted" => {
            {
                let mut state = api.state.lock().unwrap();
                state.pull["base"]["ref"] = json!("release");
                state.cause_fixture.as_mut().unwrap().parsed = true;
            }
            let prior: Value =
                serde_json::from_str(include_str!("../fixtures/mergify/pq3221/prior-drafts.json"))
                    .unwrap();
            let mut checks: Value =
                serde_json::from_str(include_str!("../fixtures/mergify/pq3221/prior-checks.json"))
                    .unwrap();
            for c in checks["check_runs"].as_array_mut().unwrap() {
                if c["conclusion"] == "failure" {
                    c["output"]["summary"] = json!("FAILED: tests/queue_cause.py::test_gate");
                }
            }
            let changes=serde_json::from_value(json!({"repository":"owner/sandbox","issues":[],"comments":[],"labels":[],"pull_requests":prior,"check_runs":checks["check_runs"],"check_suites":[]})).unwrap();
            for input in Translator::new(api.clients())
                .unwrap()
                .catch_up(&config.repos[0], &changes, &controller.snapshot().unwrap())
                .unwrap()
            {
                ingress::apply(&mut controller, input).unwrap();
            }
            failure(&mut controller, &api, &config);
            let facts = queue_facts(&mut controller);
            let failed = facts
                .iter()
                .find(|f| f["evidence"].get("CiFailed").is_some())
                .unwrap();
            assert!(
                failed["evidence"]["CiFailed"]["details"]["cause"]
                    .get("Environment")
                    .is_none(),
                "r1.5: old-destination draft cannot select Environment"
            );
            assert_eq!(
                failed["evidence"]["CiFailed"]["details"]["context"]["branch"],
                "release"
            );
            health(
                &mut controller,
                &api,
                &config,
                "2026-11-06T03:00:00Z",
                "ffffffffffffffffffffffffffffffffffffffff",
            );
            controller.look_now().unwrap();
            controller.look_now().unwrap();
            assert_eq!(
                api.state.lock().unwrap().requeues,
                0,
                "old destination merge cannot authorize our retargeted PR"
            );
        }
        "terminal-hold" => {
            failure(&mut controller, &api, &config);
            {
                let mut state = api.state.lock().unwrap();
                state.merged(false);
                let head = state.pull["head"]["sha"].clone();
                state.checks = fixture("merged", "checks");
                for check in state.checks.as_array_mut().unwrap() {
                    check["head_sha"] = head.clone();
                }
            }
            failure(&mut controller, &api, &config);
            drain(&mut controller);
            assert_eq!(
                controller.world().unwrap().nodes[0].phase,
                Phase::Closed(Outcome::Merged)
            );
            assert!(
                !controller
                    .snapshot()
                    .unwrap()
                    .rows
                    .iter()
                    .any(|r| r.table == "queue_holds"),
                "r1.6: terminal cleanup must retire queue holds"
            );
        }
        "gate-retention" => {
            failure(&mut controller, &api, &config);
            for i in 0..132 {
                health(
                    &mut controller,
                    &api,
                    &config,
                    "2026-11-06T03:00:00Z",
                    &format!("{:040x}", 10000 + i),
                );
            }
            let snapshot = controller.snapshot().unwrap();
            let gates = snapshot
                .rows
                .iter()
                .filter(|r| r.table == "queue_gates")
                .count();
            assert!(
                gates <= 128,
                "r1.6: gate retention must be bounded; retained {gates}"
            );
            assert!(
                snapshot
                    .rows
                    .iter()
                    .any(|r| r.table == "queue_holds" && !r.value["witness"].is_null()),
                "retention must preserve attached hold witnesses"
            );
        }
        _ => panic!("unknown review schedule {mode}"),
    }
    println!("test queue_cause_r1_{mode} ... ok");
}
