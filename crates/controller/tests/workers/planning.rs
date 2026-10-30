use super::*;

pub(super) fn planning(settings: &tl3_config::Store, mode: &str) {
    let root = tempfile::tempdir().unwrap();
    let mut config = config(settings, root.path(), mode);
    config.models.planner.push(config.models.audit[0].clone());
    if mode == "plan-split" || mode == "plan-bad-split" {
        config.limits.fit_tokens = 31_000;
    }
    let mut store = Store::open(settings).unwrap();
    materials(&mut store);
    let mut inputs = store.worker_materials("0").unwrap().unwrap();
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
        adapters::simulation_with_workers(&root.path().join("fakes"), &config).unwrap();
    let mut controller = Controller::open(
        Store::open(settings).unwrap(),
        registry,
        Bootstrap::from_config(&config).unwrap(),
    )
    .unwrap();
    controller.intake(labelled()).unwrap();
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
        assert_eq!(
            snapshot
                .rows
                .iter()
                .filter(|r| r.table == "plan_reports")
                .count(),
            1
        );
        drop(controller);
        let (registry, _) =
            adapters::simulation_with_workers(&root.path().join("fakes"), &config).unwrap();
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
        assert_eq!(n.question.unwrap().kind, tl3_kernel::QKind::CantMeet);
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
                .filter(|r| r.table == "plan_reports")
                .count(),
            2
        );
    }
}

pub(super) fn stop(settings: &tl3_config::Store) {
    let root = tempfile::tempdir().unwrap();
    let config = config(settings, root.path(), "plan-blocked");
    let mut store = Store::open(settings).unwrap();
    materials(&mut store);
    let (registry, mut source) =
        adapters::simulation_with_workers(&root.path().join("fakes"), &config).unwrap();
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
