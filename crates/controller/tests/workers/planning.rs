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
        n.phase.is_closed() || n.question.is_some()
    });
    let world = controller.world().unwrap();
    let n = &world.nodes[0];
    let plan = store.worker_document("0", "plans").unwrap().unwrap();
    let snapshot = store.load().unwrap();
    let requests: Vec<Value> = fs::read_dir(&config.workers.runs)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.is_dir())
        .map(|p| {
            serde_json::from_slice(&fs::read(p.join("attempt-00000000/request.json")).unwrap())
                .unwrap()
        })
        .filter(|v: &Value| v["role"] == "planner")
        .collect();
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
