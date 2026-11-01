use super::*;

// Q52: a required check that will never report on the approved head raises a
// nonblocking setup incident instead of a silent wait in Publishing.
pub(super) fn scenario(settings: &tl3_config::Store, mode: &str) {
    let api = Server::new();
    let root = tempfile::tempdir().unwrap();
    let mut controller = publishing(settings, &api, root.path());
    let head = api.state.lock().unwrap().pull["head"]["sha"]
        .as_str()
        .unwrap()
        .to_owned();
    let before = controller.world().unwrap().nodes[0].clone();
    {
        let mut state = api.state.lock().unwrap();
        if mode == "wrong-app" {
            // The ruleset names an app that never posts this context; Actions does.
            state.required_app = 99;
            state.head_checks = vec![check_run(&head, 901, "Sandbox tests", "completed", 15368)];
        } else {
            // A path filter skipped the required workflow; another workflow ran.
            state.head_checks = vec![check_run(&head, 902, "Lint", "completed", 15368)];
        }
        state.suites = json!([suite(&head, 801, "completed"), suite(&head, 802, "in_progress")]);
    }
    deliver_suite(&mut controller, &api, &head, 801);
    assert!(
        missing_incidents(&mut controller).is_empty(),
        "a running check suite may still produce the required context"
    );
    api.state.lock().unwrap().suites[1]["status"] = json!("completed");
    deliver_suite(&mut controller, &api, &head, 802);
    let expected_app = if mode == "wrong-app" { "99" } else { "15368" };
    let incident = only_incident(&mut controller);
    let summary = incident.value["summary"].as_str().unwrap();
    for part in [head.as_str(), "Sandbox tests", expected_app] {
        assert!(
            summary.contains(part),
            "the incident must name the head, context and expected app: {summary}"
        );
    }
    assert_eq!(incident.value["open"], true);
    assert_eq!(incident.value["blocking"], false);
    assert_waiting(&mut controller, &api, &before);
    // Further completed suites and check runs on the same head add no duplicate.
    deliver_suite(&mut controller, &api, &head, 802);
    let lint = check_run(&head, 902, "Lint", "completed", 15368);
    deliver_run(&mut controller, &api, lint);
    assert_eq!(missing_incidents(&mut controller).len(), 1);
    assert_eq!(only_incident(&mut controller).value["open"], true);
    assert_waiting(&mut controller, &api, &before);
    match mode {
        "path-filter" => {}
        "wrong-app" => {
            // A rerun from the wrong app is not the required report.
            let rerun = check_run(&head, 903, "Sandbox tests", "completed", 15368);
            api.state.lock().unwrap().head_checks.push(rerun.clone());
            deliver_run(&mut controller, &api, rerun);
            assert_eq!(only_incident(&mut controller).value["open"], true);
            assert_waiting(&mut controller, &api, &before);
        }
        "late-report" => {
            let mut late = check_run(&head, 904, "Sandbox tests", "in_progress", 15368);
            api.state.lock().unwrap().head_checks.push(late.clone());
            deliver_run(&mut controller, &api, late.clone());
            assert_eq!(
                only_incident(&mut controller).value["open"],
                false,
                "a late report of the required context from the expected app clears the incident"
            );
            assert_waiting(&mut controller, &api, &before);
            late["status"] = json!("completed");
            late["conclusion"] = json!("success");
            api.state.lock().unwrap().head_checks[1] = late.clone();
            deliver_run(&mut controller, &api, late);
            assert_enqueued(&mut controller, &api, &before);
        }
        "rob-clear" => {
            let id = only_incident(&mut controller).id;
            clear(&mut controller, &id);
            assert_eq!(only_incident(&mut controller).value["open"], false);
            assert_waiting(&mut controller, &api, &before);
            deliver_suite(&mut controller, &api, &head, 802);
            assert_eq!(
                only_incident(&mut controller).value["open"],
                false,
                "Rob's clear stands for this head"
            );
            assert_waiting(&mut controller, &api, &before);
            let report = check_run(&head, 905, "Sandbox tests", "completed", 15368);
            api.state.lock().unwrap().head_checks.push(report.clone());
            deliver_run(&mut controller, &api, report);
            assert_enqueued(&mut controller, &api, &before);
        }
        _ => panic!("unknown missing-check mode {mode}"),
    }
    println!("test missing_check_{mode} ... ok");
}

fn publishing(settings: &tl3_config::Store, api: &Server, root: &Path) -> Controller {
    api.state.lock().unwrap().required = true;
    let mut registry = adapters::simulation(root).unwrap();
    registry.register(Upstream::Mergify, api.adapter(&root.join("mergify"), 0));
    let mut controller =
        Controller::open(Store::open(settings).unwrap(), registry, bootstrap()).unwrap();
    controller.register_github_translator("mergify", Translator::new(api.clients()).unwrap());
    controller
        .intake(LabelledIssue {
            delivery: "label-1".into(),
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
        let progressed = controller
            .drive_once_with(|boundary, key| {
                if boundary == Boundary::BeforeDispatch {
                    let effect = Store::open(settings).unwrap().effect(key).unwrap().unwrap();
                    let delivery: Delivery = serde_json::from_value(effect.payload).unwrap();
                    if matches!(
                        delivery.output,
                        Out::Effect {
                            kind: EffectKind::Enqueue,
                            ..
                        }
                    ) {
                        api.state.lock().unwrap().pull["head"]["sha"] = json!(delivery.head_sha);
                    }
                }
            })
            .unwrap();
        if !progressed {
            break;
        }
    }
    assert_eq!(controller.world().unwrap().nodes[0].phase, Phase::Publishing);
    assert_eq!(
        api.state.lock().unwrap().posts,
        0,
        "the enqueue waits for the required check"
    );
    controller
}

fn check_run(head: &str, id: u64, name: &str, status: &str, app: u64) -> Value {
    json!({"id":id,"name":name,"head_sha":head,"status":status,
        "conclusion":(status == "completed").then_some("success"),
        "external_id":null,"output":null,"pull_requests":[],
        "app":{"id":app,"slug":"github-actions"},"check_suite":{"id":800 + id % 100}})
}

fn suite(head: &str, id: u64, status: &str) -> Value {
    json!({"id":id,"head_sha":head,"status":status,
        "conclusion":(status == "completed").then_some("success"),
        "pull_requests":[{"number":100}],
        "app":{"id":15368,"slug":"github-actions"}})
}

fn apply_webhook(controller: &mut Controller, api: &Server, name: &str, body: Value) {
    let snapshot = controller.snapshot().unwrap();
    for input in Translator::new(api.clients())
        .unwrap()
        .webhook(&parsed(name, body), &snapshot, &bootstrap().repos)
        .unwrap()
    {
        ingress::apply(controller, input).unwrap();
    }
    drain(controller);
}

fn deliver_suite(controller: &mut Controller, api: &Server, head: &str, id: u64) {
    let mut body = webhook_fixture();
    body["action"] = json!("completed");
    body["check_suite"] = suite(head, id, "completed");
    apply_webhook(controller, api, "check_suite", body);
}

fn deliver_run(controller: &mut Controller, api: &Server, run: Value) {
    let mut body = webhook_fixture();
    body["action"] = json!(if run["status"] == "completed" {
        "completed"
    } else {
        "created"
    });
    body["check_run"] = run;
    apply_webhook(controller, api, "check_run", body);
}

fn missing_incidents(controller: &mut Controller) -> Vec<tl3_store::WorldRow> {
    controller
        .snapshot()
        .unwrap()
        .rows
        .into_iter()
        .filter(|row| {
            row.table == "incidents"
                && row.value["summary"]
                    .as_str()
                    .is_some_and(|summary| summary.contains("Sandbox tests"))
        })
        .collect()
}

fn only_incident(controller: &mut Controller) -> tl3_store::WorldRow {
    let mut incidents = missing_incidents(controller);
    assert_eq!(incidents.len(), 1, "one deduplicated missing-check incident");
    incidents.pop().unwrap()
}

fn assert_waiting(controller: &mut Controller, api: &Server, before: &tl3_kernel::Node) {
    let world = controller.world().unwrap();
    let node = &world.nodes[0];
    assert_eq!(node.phase, Phase::Publishing);
    assert!(
        node.effect
            .is_some_and(|effect| effect.kind == EffectKind::Enqueue && !effect.cancelling),
        "the enqueue stays pending"
    );
    assert_eq!(api.state.lock().unwrap().posts, 0, "required checks still gate the enqueue");
    assert_no_charge(node, before);
}

fn assert_enqueued(controller: &mut Controller, api: &Server, before: &tl3_kernel::Node) {
    let world = controller.world().unwrap();
    assert_eq!(api.state.lock().unwrap().posts, 1);
    assert_eq!(world.nodes[0].phase, Phase::Queued);
    assert_no_charge(&world.nodes[0], before);
}

fn assert_no_charge(node: &tl3_kernel::Node, before: &tl3_kernel::Node) {
    assert_eq!(node.credit, before.credit, "no credit is charged");
    assert_eq!(node.round, before.round);
    assert_eq!(node.attempt, before.attempt);
    assert_eq!(node.fx_failures, before.fx_failures, "no failure is counted");
    assert_eq!(node.fx_incidents, before.fx_incidents);
    assert!(!node.waiting_incident);
}

fn clear(controller: &mut Controller, id: &str) {
    controller
        .observe(
            "clear-missing-check",
            tl3_controller::executor::Observation::GitHubComment {
                repository: bootstrap().repos[0].clone(),
                issue: 12,
                comment: tl3_github::Comment {
                    id: 91000,
                    body: format!("clear incident {id}"),
                    user: tl3_github::Actor {
                        id: 42,
                        login: "fixture".into(),
                    },
                    created_at: chrono::DateTime::UNIX_EPOCH,
                    updated_at: chrono::DateTime::UNIX_EPOCH,
                    issue_url: "https://api.github.com/repos/owner/sandbox/issues/12".into(),
                },
            },
        )
        .unwrap();
    drain(controller);
}
