use super::*;

fn publishing(settings: &tl3_config::Store, api: &Server, root: &Path) -> Controller {
    let mut registry = adapters::simulation(root).unwrap();
    registry.register(Upstream::Mergify, api.adapter(&root.join("mergify"), 0));
    let mut controller =
        Controller::open(Store::open(settings).unwrap(), registry, bootstrap()).unwrap();
    controller.register_github_translator("mergify", Translator::new(api.clients()).unwrap());
    controller
        .intake(LabelledIssue {
            delivery: "classic-label".into(),
            owner: "owner".into(),
            repo: "sandbox".into(),
            installation: 3,
            number: 12,
            title: "change".into(),
            actor: 42,
            label: "tl3".into(),
        })
        .unwrap();
    for _ in 0..300 {
        if let Some(effect) = controller
            .snapshot()
            .unwrap()
            .pending_effects
            .iter()
            .find(|effect| effect.payload["output"]["Effect"]["kind"] == "Enqueue")
        {
            let delivery: Delivery = serde_json::from_value(effect.payload.clone()).unwrap();
            api.state.lock().unwrap().pull["head"]["sha"] = json!(delivery.head_sha);
            return controller;
        }
        assert!(controller.drive_once().unwrap());
    }
    panic!("publishing did not reach enqueue");
}

fn finish_head(api: &Server) {
    let mut state = api.state.lock().unwrap();
    let head = state.pull["head"]["sha"].clone();
    state.head_checks = vec![json!({"id":901,"name":"Sandbox tests","head_sha":head,
        "status":"completed","conclusion":"success", "completed_at":"2026-11-01T03:00:01Z",
        "app":{"id":15368,"slug":"github-actions"},"check_suite":{"id":800}})];
}

fn wake(controller: &mut Controller, api: &Server) {
    let mut body = webhook_fixture();
    body["action"] = json!("completed");
    body["check_run"] = api.state.lock().unwrap().head_checks[0].clone();
    for input in Translator::new(api.clients())
        .unwrap()
        .webhook(
            &parsed("check_run", body),
            &controller.snapshot().unwrap(),
            &bootstrap().repos,
        )
        .unwrap()
    {
        ingress::apply(controller, input).unwrap();
    }
    drain(controller);
}

pub(super) fn scenario(settings: &tl3_config::Store, mode: &str) {
    if let Some(mode) = mode.strip_prefix("restricted-") {
        restricted(settings, mode);
        return;
    }
    let api = Server::new();
    let root = tempfile::tempdir().unwrap();
    let mut controller = publishing(settings, &api, root.path());
    {
        let mut state = api.state.lock().unwrap();
        state.classic = true;
        assert!(
            !state.required,
            "this repository has no ruleset requirements"
        );
        let head = state.pull["head"]["sha"].clone();
        if mode == "stale-failure" {
            state.checks = fixture("failed", "checks");
            state.label = true;
        }
        if mode != "missing" && mode != "status" {
            state.head_checks = vec![json!({"id":900,"name":"Sandbox tests","head_sha":head,
                "status":if mode == "pass" {"in_progress"} else {"completed"},
                "conclusion":if mode == "pass" {Value::Null} else if matches!(mode, "action_required" | "stale") {json!(mode)} else {json!("failure")},
                "completed_at":"2026-11-01T03:00:00Z",
                "app":{"id":if mode == "app" {99} else {15368},"slug":"github-actions"},
                "check_suite":{"id":800}})];
        }
        if matches!(mode, "runner" | "runner-code") {
            state.annotations = json!([{"annotation_level":"failure",
                "message":"The runner has lost communication with the server"}]);
            state.workflow = Some(json!({"id":700,"check_suite_id":800,"head_sha":head,
                "status":"completed","conclusion":"failure","run_attempt":1}));
        }
        if mode == "status" {
            state.classic_context_only = true;
            state.statuses = json!([{"id":50,"context":"Sandbox tests","state":"pending"}]);
        }
    }
    assert!(controller.drive_once().unwrap());
    if mode == "runner-code" {
        for _ in 0..20 {
            if controller.world().unwrap().nodes[0]
                .effect
                .is_some_and(|effect| effect.cancelling)
            {
                break;
            }
            assert!(controller.drive_once().unwrap());
        }
        assert!(
            controller.world().unwrap().nodes[0]
                .effect
                .unwrap()
                .cancelling
        );
        finish_head(&api);
        let check: tl3_github::CheckRun = {
            let mut state = api.state.lock().unwrap();
            state.head_checks[0]["conclusion"] = json!("failure");
            state.annotations = json!([]);
            serde_json::from_value(state.head_checks[0].clone()).unwrap()
        };
        controller
            .observe(
                "failure-during-enqueue-cancellation",
                tl3_controller::executor::Observation::Queue(
                    tl3_controller::adapters::mergify::QueueFact {
                        repository: bootstrap().repos[0].clone(),
                        number: 100,
                        evidence: tl3_controller::adapters::mergify::Evidence::CiFailed {
                            check: check.id,
                            completed_at: check.completed_at.map(|time| time.to_rfc3339()),
                            head: check.head_sha,
                            infra: false,
                            head_check: true,
                        },
                    },
                ),
            )
            .unwrap();
        assert!(controller.drive_once().unwrap());
    }
    if matches!(mode, "failure" | "stale-failure" | "runner-code") {
        // Consume only until the fix round starts, so the fake maker cannot finish it.
        for _ in 0..20 {
            if controller.world().unwrap().nodes[0].phase == Phase::Making {
                break;
            }
            if !controller.drive_once().unwrap() {
                break;
            }
        }
        assert_eq!(
            controller.world().unwrap().nodes[0].phase,
            Phase::Making,
            "classic required test failures must enter the existing fix round"
        );
        assert_eq!(controller.world().unwrap().nodes[0].round, 1);
        assert_eq!(
            api.state.lock().unwrap().posts,
            0,
            "failed heads must never queue"
        );
        assert_eq!(api.state.lock().unwrap().reruns, 0);
        if mode == "stale-failure" {
            for _ in 0..20 {
                if !controller
                    .snapshot()
                    .unwrap()
                    .pending_effects
                    .iter()
                    .any(|effect| effect.payload["output"]["Effect"]["kind"] == "Enqueue")
                {
                    break;
                }
                assert!(controller.drive_once().unwrap());
            }
            let state = api.state.lock().unwrap();
            assert!(
                !state.label,
                "cancelled enqueue must remove its queue label"
            );
            assert_eq!(state.deletes, 1);
        }
        println!("test classic_head_{mode} ... ok");
        return;
    }
    drain(&mut controller);
    assert_eq!(
        api.state.lock().unwrap().posts,
        0,
        "required head checks must pass before the queue label is added"
    );
    assert_eq!(
        controller.world().unwrap().nodes[0].phase,
        Phase::Publishing
    );
    if mode == "runner" {
        assert_eq!(
            api.state.lock().unwrap().reruns,
            1,
            "lost runners must be rerun"
        );
        assert_eq!(controller.world().unwrap().nodes[0].round, 0);
    }
    // A successful check on the approved head wakes the pending enqueue.
    if mode == "status" {
        api.state.lock().unwrap().statuses = json!([
            {"id":51,"context":"Sandbox tests","state":"success"},
            {"id":50,"context":"Sandbox tests","state":"pending"}
        ]);
        let key = controller
            .snapshot()
            .unwrap()
            .pending_effects
            .into_iter()
            .find(|effect| effect.payload["output"]["Effect"]["kind"] == "Enqueue")
            .unwrap()
            .key;
        controller.look_key(&key).unwrap();
        drain(&mut controller);
    } else {
        finish_head(&api);
        wake(&mut controller, &api);
    }
    assert_eq!(api.state.lock().unwrap().posts, 1);
    assert_eq!(controller.world().unwrap().nodes[0].phase, Phase::Queued);
    assert_eq!(controller.world().unwrap().nodes[0].round, 0);
    println!("test classic_head_{mode} ... ok");
}

fn restricted(settings: &tl3_config::Store, mode: &str) {
    let api = Server::new();
    let root = tempfile::tempdir().unwrap();
    let mut controller = publishing(settings, &api, root.path());
    let read = if mode.starts_with("branch") {
        "branch"
    } else if mode.starts_with("rules") {
        "rules"
    } else {
        "status"
    };
    let status = if mode.ends_with("403") { 403 } else { 404 };
    {
        let mut state = api.state.lock().unwrap();
        state.required = read == "branch";
        state.classic = read != "branch";
        state.classic_context_only = read == "status";
        state.restricted = Some((read.into(), status));
    }
    finish_head(&api);
    // Keep an observable check-run gate even when legacy statuses are unavailable.
    if read == "status" {
        api.state.lock().unwrap().head_checks[0]["conclusion"] = Value::Null;
        api.state.lock().unwrap().head_checks[0]["status"] = json!("in_progress");
        drain(&mut controller);
        assert_eq!(api.state.lock().unwrap().posts, 0);
        finish_head(&api);
        wake(&mut controller, &api);
    } else {
        drain(&mut controller);
    }
    assert_eq!(
        controller.world().unwrap().nodes[0].phase,
        Phase::Queued,
        "an unavailable optional read must not block Publishing"
    );
    assert_eq!(api.state.lock().unwrap().posts, 1);
    assert_eq!(controller.world().unwrap().nodes[0].round, 0);
    let snapshot = controller.snapshot().unwrap();
    let incidents: Vec<_> = snapshot
        .rows
        .iter()
        .filter(|row| row.table == "incidents")
        .collect();
    assert_eq!(incidents.len(), 1, "one deduplicated setup incident");
    assert_eq!(incidents[0].value["blocking"], false);
    assert_eq!(incidents[0].value["signature"]["class"], "setup");
    let permission = if read == "branch" {
        "Contents: read"
    } else if read == "rules" {
        "Metadata: read"
    } else {
        "Commit statuses: read"
    };
    assert!(
        incidents[0].value["summary"]
            .as_str()
            .unwrap()
            .contains(permission)
    );
    api.state.lock().unwrap().checks = fixture("failed", "checks");
    let key = snapshot
        .rows
        .iter()
        .find(|row| {
            row.table == "allowances"
                && row.value["delivery"]["output"]["Effect"]["kind"] == "Enqueue"
        })
        .unwrap()
        .id
        .clone();
    controller.look_key(&key).unwrap();
    for _ in 0..30 {
        if controller.world().unwrap().nodes[0].phase == Phase::Making {
            break;
        }
        if !controller.drive_once().unwrap() {
            break;
        }
    }
    assert_eq!(
        controller.world().unwrap().nodes[0].phase,
        Phase::Making,
        "restricted reads must not discard queued CI failures"
    );
    assert_eq!(
        controller
            .snapshot()
            .unwrap()
            .rows
            .iter()
            .filter(|row| row.table == "incidents")
            .count(),
        1
    );
    println!("test restricted_head_{mode} ... ok");
}
