use super::*;

pub(super) fn planning(settings: &tl3_config::Store, mode: &str) {
    let root = tempfile::tempdir().unwrap();
    let mut config = config(settings, root.path(), mode);
    if !matches!(mode, "normal" | "plan-invalid") {
        config.workers.command[0] = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/worker_transport.py")
            .to_str()
            .unwrap()
            .into();
    }
    config.github.repos[0].always_tests = vec!["tests/test_rules.py".into()];
    config.models.planner.push(config.models.audit[0].clone());
    if mode == "plan-split" || mode == "plan-bad-split" {
        config.limits.fit_tokens = 31_000;
    }
    let mut store = Store::open(settings).unwrap();
    materials(&mut store);
    let mut inputs = store.worker_materials("0").unwrap().unwrap();
    if mode == "normal" {
        inputs["repo"]["file_list"]
            .as_array_mut()
            .unwrap()
            .push(json!("tests/test_greeting.py"));
        inputs["repo"]["materials"]
            .as_array_mut()
            .unwrap()
            .push(json!({
                "path": "tests/test_greeting.py",
                "content": "def test_hello(): assert True\ndef test_control(): assert True\n"
            }));
    }
    inputs["repo"]["file_list"]
        .as_array_mut()
        .unwrap()
        .push(json!("image.png"));
    store.put_worker_materials("0", &inputs).unwrap();
    if mode == "plan-split" || mode == "plan-bad-split" {
        let mut inputs = store.worker_materials("0").unwrap().unwrap();
        inputs["repo"]["file_list"] = json!(["greeting.py", "tests/test_greeting.py"]);
        inputs["repo"]["materials"][0]["content"] = json!("hello world\n".repeat(1000));
        inputs["repo"]["materials"].as_array_mut().unwrap().push(
            json!({"path": "tests/test_greeting.py", "content": "hello world\n".repeat(1000)}),
        );
        store.put_worker_materials("0", &inputs).unwrap();
    }
    let (registry, mut source) =
        worker_simulation(&root.path().join("fakes"), &config, false).unwrap();
    let mut controller = Controller::open(
        Store::open(settings).unwrap(),
        registry,
        Bootstrap::from_config(&config).unwrap(),
    )
    .unwrap();
    controller.intake(labelled()).unwrap();
    if mode == "normal" {
        let mut commit = Commit::new(
            "reviewed-decision-fixture",
            json!({}),
            &store.load().unwrap(),
        );
        commit.rows.push(RowChange::Put {
            table: "request_decisions".into(),
            id: "0".into(),
            value: serde_json::from_str(include_str!("../fixtures/decisions/pq2662.json")).unwrap(),
        });
        store.commit(&commit).unwrap();
    }
    wait(|| {
        drive(&mut controller, &mut source, &mut store);
        let n = &controller.world().unwrap().nodes[0];
        n.phase.is_closed()
            || n.question.is_some()
            || (mode == "plan-bad-schema" && n.run_incidents > 0)
    });
    let world = controller.world().unwrap();
    let n = &world.nodes[0];
    let plan = store.worker_document("0", "plans").unwrap();
    let snapshot = store.load().unwrap();
    let requests: Vec<Value> = recorded_runs(root.path())
        .into_iter()
        .map(|value| value["request"].clone())
        .filter(|v: &Value| v["role"] == "planner")
        .collect();
    for request in &requests {
        assert_eq!(
            request["inputs"]["always_tests"],
            json!(["tests/test_rules.py"])
        );
    }
    assert_retired(&config, &mut store);
    if mode == "plan-bad-schema" {
        assert!(matches!(n.phase, Phase::Planning { .. }));
        assert_eq!(n.run_incidents, 1);
        assert!(n.question.is_none());
        assert_eq!(requests.len(), 2);
        assert!(
            plan.is_none(),
            "malformed output must not be stored as a plan"
        );
        assert!(!snapshot.rows.iter().any(|r| r.table == "plan_reports"));
        return;
    }
    let plan = plan.unwrap();
    let mut reader = tl3_store::Reader::open(settings).unwrap();
    let status = tl3_controller::status::load(&mut reader).unwrap();
    assert_eq!(
        status.requests[0].details.last_progress.is_some(),
        !n.phase.is_closed(),
        "progress survives retirement while waiting for Rob, and is removed on closure"
    );
    if mode == "normal" {
        // #422: Hindsight is down in this fixture, so recall degrades and work proceeds.
        let context: Value =
            serde_json::from_str(include_str!("../fixtures/decisions/pq2662.json")).unwrap();
        for role in ["ingest", "audit", "planner"] {
            let run = recorded_runs(root.path())
                .into_iter()
                .find(|r| r["request"]["role"] == role)
                .unwrap();
            assert_eq!(
                run["request"]["inputs"]["decision_context"], context,
                "{role} needs explicit decisions even with recall down"
            );
        }
        assert_eq!(requests[0]["inputs"]["memories"], json!([]));
        assert!(requests[0]["inputs"]["memory_note"].is_string());
        let turns: Vec<Value> = fs::read_to_string(root.path().join("entered.consistency-turns"))
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(
            turns,
            vec![json!({"tools": true}), json!({"tools": false})],
            "a consistency-only draft gets exactly one tool-free correction in the production planner run"
        );
        assert_eq!(plan["edits_tests"], json!([]));
        assert!(n.phase.is_closed());
        assert_eq!(requests.len(), 1);
        assert_eq!(plan["kind"], "leaf");
        assert_eq!(plan["forecast"]["fits"], true);
        let declared: Vec<_> = snapshot
            .interned
            .iter()
            .filter(|i| i.kind == tl3_store::InternKind::Path)
            .map(|i| i.value.as_str())
            .collect();
        assert!(declared.contains(&"greeting.py") && declared.contains(&"tests/test_greeting.py"));
    } else if mode == "plan-split" {
        let question = n.question.unwrap();
        assert_eq!(question.kind, tl3_kernel::QKind::NeedsSplitting);
        assert_eq!(requests.len(), 1);
        assert_eq!(
            n.credit.start, 5,
            "root forecast above limit gives two leaves plus reserve"
        );
        let context = store.question_context(question.id).unwrap().unwrap();
        assert_eq!(context["pieces"], plan["pieces"]);
        assert_eq!(context["pieces"][0]["brief"], "Fix greeting.");
        // #322: the question carries the planner's split as findings for Rob.
        assert_eq!(context["findings"]["source"], "planner");
        assert_eq!(context["findings"]["pieces"][0]["brief"], "Fix greeting.");
        assert!(
            context["findings"]["decision"]
                .as_str()
                .unwrap()
                .ends_with('?')
        );
        assert_eq!(
            snapshot
                .rows
                .iter()
                .filter(|r| matches!(r.table.as_str(), "plan_reports" | "reviews"))
                .count(),
            0
        );
        drop(controller);
        let (registry, _) = worker_simulation(&root.path().join("fakes"), &config, false).unwrap();
        let mut reopened = Controller::open(
            Store::open(settings).unwrap(),
            registry,
            Bootstrap::from_config(&config).unwrap(),
        )
        .unwrap();
        assert_eq!(
            reopened.world().unwrap().nodes[0].question.unwrap().id,
            question.id
        );
        assert_eq!(
            store.question_context(question.id).unwrap().unwrap(),
            context
        );
    } else {
        assert!(
            n.question.is_some(),
            "rejected plans must open a decision question"
        );
        assert_eq!(n.question.unwrap().kind, tl3_kernel::QKind::CantMeet);
        // #403: a question to Rob owes a postmortem in the commit that asks it.
        let owed = store.postmortems("0").unwrap();
        assert!(
            owed.iter().any(|record| record.trigger
                == format!("question:{}", n.question.unwrap().id)
                && record.reason == "escalation"),
            "{owed:?}"
        );
        // #322: the question carries the planner's last report for Rob.
        let context = store
            .question_context(n.question.unwrap().id)
            .unwrap()
            .unwrap();
        assert_eq!(context["findings"]["source"], "planner");
        assert!(context["findings"]["report"].as_str().is_some());
        assert_eq!(requests.len(), 2, "the kernel retries a rejected plan once");
        let retry = requests
            .iter()
            .find(|r| !r["inputs"]["findings"].as_array().unwrap().is_empty())
            .expect("retry includes the previous report");
        assert_eq!(retry["model"]["name"], "alternate-model");
        assert!(
            retry["inputs"]["findings"]
                .as_array()
                .unwrap()
                .iter()
                .any(|s| s.as_str().unwrap().contains(if mode == "plan-invalid" {
                    "protected path"
                } else if mode == "plan-bad-split" {
                    "at least two pieces"
                } else {
                    "Need a decision"
                }))
        );
        assert_eq!(
            snapshot
                .rows
                .iter()
                .filter(|r| matches!(r.table.as_str(), "plan_reports" | "reviews"))
                .count(),
            0
        );
    }
}

pub(super) fn stop(settings: &tl3_config::Store) {
    let root = tempfile::tempdir().unwrap();
    let config = config(settings, root.path(), "plan-blocked");
    let mut store = Store::open(settings).unwrap();
    materials(&mut store);
    let (registry, mut source) =
        worker_simulation(&root.path().join("fakes"), &config, false).unwrap();
    let mut controller = Controller::open(
        Store::open(settings).unwrap(),
        registry,
        Bootstrap::from_config(&config).unwrap(),
    )
    .unwrap();
    controller.intake(labelled()).unwrap();
    wait(|| {
        drive(&mut controller, &mut source, &mut store);
        root.path().join("entered").exists()
    });
    assert!(matches!(
        controller.world().unwrap().nodes[0].phase,
        Phase::Planning { .. }
    ));
    let worker_pid = fs::read_to_string(root.path().join("entered")).unwrap();
    controller
        .observe(
            "stop-planner",
            Observation::Kernel(Event::Command {
                node: NodeId(0),
                id: 1,
                command: tl3_kernel::Command::Stop,
            }),
        )
        .unwrap();
    wait(|| {
        drive(&mut controller, &mut source, &mut store);
        controller.world().unwrap().nodes[0].phase.is_closed()
    });
    assert_eq!(
        controller.world().unwrap().nodes[0].phase,
        Phase::Closed(tl3_kernel::Outcome::Stopped)
    );
    assert!(store.worker_document("0", "plans").unwrap().is_none());
    let stat = fs::read_to_string(format!("/proc/{worker_pid}/stat"));
    assert!(
        stat.is_err()
            || stat
                .unwrap()
                .rsplit_once(')')
                .unwrap()
                .1
                .split_whitespace()
                .next()
                == Some("Z")
    );
}

/// #422: a seeded bank reaches ingest and the planner, and a planner's verified
/// flag hides a wrong memory from later recall.
pub(super) fn memories(settings: &tl3_config::Store) {
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{method, path, path_regex},
    };
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let server = runtime.block_on(MockServer::start());
    let (recalled, recalls) = std::sync::mpsc::channel();
    runtime.block_on(
        Mock::given(method("POST"))
            .and(path("/v1/default/banks/tl3/memories/recall"))
            .respond_with(move |request: &wiremock::Request| {
                let body: Value = request.body_json().unwrap();
                recalled.send(body.clone()).unwrap();
                let results = if body.get("tags").is_some() {
                    json!([{"id": "lesson-greeting", "text": "greet_user() returns 'hi'.", "tags": ["kind:lesson", "repo:RobTand/tl3-sandbox"]}])
                } else {
                    json!([{"id": "run-earlier", "text": "An earlier run read greeting.py.", "tags": ["role:maker"]}])
                };
                ResponseTemplate::new(200).set_body_json(json!({"results": results}))
            })
            .mount(&server),
    );
    let (curated, sent) = std::sync::mpsc::channel();
    runtime.block_on(
        Mock::given(method("PATCH"))
            .and(path_regex("^/v1/default/banks/tl3/memories/[^/]+$"))
            .respond_with(move |request: &wiremock::Request| {
                let body: Value = request.body_json().unwrap();
                curated.send((request.url.path().to_owned(), body)).unwrap();
                ResponseTemplate::new(200).set_body_json(json!({}))
            })
            .mount(&server),
    );
    runtime.block_on(
        Mock::given(method("POST"))
            .and(path("/v1/default/banks/tl3/memories"))
            .respond_with(|request: &wiremock::Request| {
                let body: Value = request.body_json().unwrap();
                ResponseTemplate::new(200).set_body_json(json!({"success": true, "async": true,
                    "bank_id": "tl3", "items_count": 1, "operation_id": body["operation_id"]}))
            })
            .mount(&server),
    );
    let root = tempfile::tempdir().unwrap();
    let mut config = config(settings, root.path(), "plan-memories");
    config.hindsight.endpoint = server.uri().parse().unwrap();
    let mut store = Store::open(settings).unwrap();
    materials(&mut store);
    // Evidence must share an identifier with the claim; `greet` alone is too generic.
    let mut inputs = store.worker_materials("0").unwrap().unwrap();
    inputs["repo"]["materials"][0]["content"] = json!("def greet_user(): return 'hello'\n");
    store.put_worker_materials("0", &inputs).unwrap();
    let (registry, mut source) =
        worker_simulation(&root.path().join("fakes"), &config, false).unwrap();
    let mut controller = Controller::open(
        Store::open(settings).unwrap(),
        registry,
        Bootstrap::from_config(&config).unwrap(),
    )
    .unwrap();
    controller.intake(labelled()).unwrap();
    wait(|| {
        drive(&mut controller, &mut source, &mut store);
        controller.world().unwrap().nodes[0].phase.is_closed()
    });
    let requests: Vec<Value> = recorded_runs(root.path())
        .into_iter()
        .map(|value| value["request"].clone())
        .collect();
    let role = |name: &str| {
        requests
            .iter()
            .find(|r| r["role"] == name)
            .unwrap_or_else(|| panic!("no {name} run"))
            .clone()
    };
    let expected = json!([
        {"id": "lesson-greeting", "kind": "lesson", "repo": "RobTand/tl3-sandbox", "content": "greet_user() returns 'hi'."},
        {"id": "run-earlier", "kind": "run", "content": "An earlier run read greeting.py."},
    ]);
    let ingest = role("ingest");
    assert_eq!(ingest["inputs"]["memories"], expected);
    assert!(ingest["inputs"].get("memory_note").is_none());
    let planner = role("planner");
    assert_eq!(planner["inputs"]["memories"], expected);
    let brief = store.worker_document("0", "briefs").unwrap().unwrap();
    assert_eq!(
        brief["memory_ids"],
        json!(["lesson-greeting"]),
        "ingest cites what it was given"
    );
    let plan = store.worker_document("0", "plans").unwrap().unwrap();
    assert_eq!(plan["memory_ids"], json!(["lesson-greeting"]));

    let bodies: Vec<Value> = recalls.try_iter().collect();
    let queries: Vec<&str> = bodies
        .iter()
        .map(|b| b["query"].as_str().unwrap())
        .collect();
    assert!(
        queries
            .iter()
            .any(|q| q.contains("Fix greeting") && q.contains("Return hello.")),
        "ingest recalls on the issue: {queries:?}"
    );
    assert!(
        queries
            .iter()
            .any(|q| q.contains(brief["brief"].as_str().unwrap())
                && q.contains(brief["goals"][0]["description"].as_str().unwrap())
                && q.contains("RobTand/tl3-sandbox")),
        "the planner recalls on the brief, goals and repo: {queries:?}"
    );
    for request in &requests {
        assert!(!request["inputs"].to_string().contains("hindsight_key"));
    }

    assert_retired(&config, &mut store);
    // One verified flag hides the memory from TL3 at once; retiring it in
    // Hindsight waits for a second, independent signal.
    let curations = store.memory_curations().unwrap();
    assert_eq!(curations.len(), 1);
    assert_eq!(curations[0]["memory_id"], "lesson-greeting");
    assert_eq!(curations[0]["source"], "planner");
    assert_eq!(curations[0]["verdict"], "accepted");
    assert_eq!(curations[0]["state"], "hidden");
    assert!(
        sent.try_recv().is_err(),
        "one signal never retires the memory"
    );
}
