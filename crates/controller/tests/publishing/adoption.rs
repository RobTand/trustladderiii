use super::*;
use tl3_controller::ingress;

fn proposal(f: &Fixture, head: &str, body: &str) -> Value {
    json!({"number":17,"title":"An author's proposal","body":body,"labels":[{"name":"tl3"}],
        "user":{"id":7,"login":"author"},"state":"open","merged_at":null,"updated_at":"2026-10-31T00:00:00Z",
        "head":{"ref":"author","sha":head,"repo":{"id":1,"name":"tl3-sandbox","owner":{"id":1,"login":"RobTand"}}},
        "base":{"ref":"main","sha":git(&f.remote, &["rev-parse","main"]),"repo":{"id":1,"name":"tl3-sandbox","owner":{"id":1,"login":"RobTand"}}}})
}

fn label(c: &mut Controller, server: &Server, settings: &tl3_config::Store, actor: u64, id: &str) {
    let body = json!({"action":"labeled", "label":{"name":"tl3"},
        "repository":{"id":1,"name":"tl3-sandbox","owner":{"id":1,"login":"RobTand"}},
        "installation":{"id":1},"sender":{"id":actor,"login":"labeler"},
        "pull_request":server.api.state.lock().unwrap().proposed.clone().unwrap()});
    let bytes = serde_json::to_vec(&body).unwrap();
    // The authenticated receiver persists these bytes before acknowledging GitHub.
    let mut store = Store::open(settings).unwrap();
    store
        .receive_ingress("github-webhook", id, "pull_request", &bytes)
        .unwrap();
    ingress::drain(c).unwrap();
}

pub fn run(settings: &tl3_config::Store, python: &Path, mode: &str) {
    let root = tempfile::tempdir().unwrap();
    let f = Arc::new(Fixture::create(root.path()));
    commit(&f.source, "value.py", "def value(): return 1\n");
    commit(
        &f.source,
        "pyproject.toml",
        "[tool.tl3.harness]\ncommand = ['python', '-m', 'pytest']\ntest-files = ['test_*.py']\n",
    );
    commit(
        &f.source,
        "test_value.py",
        "from value import value\ndef test_value(): assert value() == 2\n",
    );
    git(&f.source, &["push", f.remote.to_str().unwrap(), "main"]);
    git(&f.source, &["checkout", "-b", "author"]);
    let author_head = commit(
        &f.source,
        "value.py",
        if mode == "adoption-fix" {
            "def value(): return 3\n"
        } else {
            "def value(): return 2\n"
        },
    );
    git(
        &f.source,
        &["push", f.remote.to_str().unwrap(), "HEAD:refs/pull/17/head"],
    );
    git(&f.source, &["checkout", "main"]);
    let check_base = commit(&f.source, "main-only.txt", "main advanced independently\n");
    git(&f.source, &["push", f.remote.to_str().unwrap(), "main"]);
    let server = Server::new(f.clone());
    server.api.state.lock().unwrap().proposed = Some(proposal(&f, &author_head, "Closes #42"));
    let mut config =
        tl3_config::Config::parse(include_str!("../../../config/example.toml")).unwrap();
    config.store = settings.clone();
    config.workers.python = python.into();
    config.workers.runs = root.path().join("runs");
    config.workers.command = vec![
        format!(
            "{}/tests/publishing/reviewer.py",
            env!("CARGO_MANIFEST_DIR")
        ),
        root.path().join("reviews.jsonl").to_str().unwrap().into(),
        "merged".into(),
    ];
    config.models.key_file = root.path().join("fixture-key");
    config.hindsight.endpoint = "http://127.0.0.1:9".parse().unwrap();
    fs::write(&config.models.key_file, "fixture").unwrap();
    let supervisor = Supervisor::new(&config, &root.path().join("fakes")).unwrap();
    let mut source = supervisor.source();
    let mut registry = adapters::simulation(&root.path().join("fakes")).unwrap();
    let maker_runs = Arc::new(Mutex::new(0));
    let count = maker_runs.clone();
    let fixture = f.clone();
    let preserved = author_head.clone();
    let queue = root.path().join("queue/mode");
    registry.register(
        Upstream::Workers,
        support::workers::Workers::new(
            Some(Box::new(supervisor)),
            adapters::workers::Fake::new(&root.path().join("fakes")),
            vec![Role::Review],
            move |d, completion| {
                for o in &mut completion.observations {
                    match o {
                        Observation::Plan { paths, .. } => *paths = vec!["value.py".into()],
                        Observation::Made { sha, .. } => {
                            *count.lock().unwrap() += 1;
                            assert_eq!(
                                (d.attempt, d.round),
                                (1, 1),
                                "the adopted attempt gets its normal fix round"
                            );
                            let checkout = fixture.adapter().checkout(d);
                            assert_eq!(git(&checkout, &["rev-parse", "HEAD"]), preserved);
                            *sha = commit(&checkout, "value.py", "def value():\n    return 2\n");
                            assert_eq!(git(&checkout, &["merge-base", &preserved, sha]), preserved);
                            fs::write(&queue, "done").unwrap();
                        }
                        _ => {}
                    }
                }
                if let Some(Observation::Made { sha, .. }) = completion.observations.first() {
                    completion
                        .observations
                        .insert(0, check_observation(&fixture, d, sha.clone()));
                }
            },
        ),
    );
    registry.register(Upstream::Worktrees, f.adapter());
    registry.register(
        Upstream::GitHub,
        server.adapter(&root.path().join("github"), 0),
    );
    registry.register(Upstream::PrismaBuild, pb(root.path()));
    if mode == "adoption-fix" {
        fs::write(root.path().join("queue/mode"), "check_fail").unwrap();
    }
    registry.register(
        Upstream::Mergify,
        adapters::mergify::Adapter::new(server.clients(), 0, &root.path().join("mergify")).unwrap(),
    );
    let mut c = Controller::open(Store::open(settings).unwrap(), registry, bootstrap()).unwrap();
    c.register_github_translator(
        "adoption",
        tl3_controller::adoption::Translator::new(server.clients(), bootstrap()).unwrap(),
    );
    let mut store = Store::open(settings).unwrap();
    store.put_worker_materials("0", &json!({"issue":{"number":42,"title":"Fix the value","body":"Return two.","author":"rob","comments":[]},"repo":{"readme":"Value","file_list":["value.py"]},"standing_rules":[], "check_run":check_plan(&f, check_base.clone())})).unwrap();
    document(
        &mut store,
        "goals",
        json!([{"description":"Return two.","verification":"test"}]),
    );
    if mode == "adoption-negative" {
        label(&mut c, &server, settings, 7, "other-label");
        assert!(c.world().unwrap().nodes.is_empty());
        assert!(
            server
                .api
                .state
                .lock()
                .unwrap()
                .proposed_comments
                .is_empty()
        );
        server.api.state.lock().unwrap().proposed.as_mut().unwrap()["body"] = json!("Almost done.");
        label(&mut c, &server, settings, 1, "no-keyword");
        while c.drive_once().unwrap() {}
        label(&mut c, &server, settings, 1, "no-keyword-again");
        assert!(c.world().unwrap().nodes.is_empty());
        let state = server.api.state.lock().unwrap();
        assert_eq!(state.proposed_comments.len(), 1);
        assert!(
            state.proposed_comments[0]["body"]
                .as_str()
                .unwrap()
                .contains("Which issue")
        );
        assert_eq!(*maker_runs.lock().unwrap(), 0);
        return;
    }
    label(&mut c, &server, settings, 1, "adopt-label");
    label(&mut c, &server, settings, 1, "adopt-label-repeated");
    assert_eq!(c.world().unwrap().nodes.len(), 1);
    assert_eq!(
        c.snapshot()
            .unwrap()
            .rows
            .iter()
            .find(|r| r.table == "requests")
            .unwrap()
            .value["title"],
        "Fix the value"
    );
    drain(&mut c, &mut source, &mut store, Phase::Queued);
    let d = f.delivery(EffectKind::Push, 100);
    let landed = f.head(&d);
    assert_eq!(
        git(&f.repo, &["merge-base", &author_head, &landed]),
        author_head
    );
    assert_eq!(
        *maker_runs.lock().unwrap(),
        usize::from(mode == "adoption-fix")
    );
    assert_eq!(c.world().unwrap().nodes[0].credit.balance(), 4);
    let reviews: Vec<Value> = fs::read_to_string(root.path().join("reviews.jsonl"))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(reviews.len(), 1);
    assert_eq!(reviews[0]["inputs"]["check_base_sha"], check_base);
    let diff = reviews[0]["inputs"]["diff"].as_str().unwrap();
    assert!(diff.contains(if mode == "adoption-fix" {
        "+    return 2"
    } else {
        "+def value(): return 2"
    }));
    assert!(
        !diff.contains("main-only.txt"),
        "review must use an ancestor comparison base"
    );
    let mut state = server.api.state.lock().unwrap();
    assert!(
        state.pull.as_ref().unwrap()["body"]
            .as_str()
            .unwrap()
            .contains("pull/17")
    );
    assert!(
        !state.pull.as_ref().unwrap()["body"]
            .as_str()
            .unwrap()
            .contains('#')
    );
    state.pull.as_mut().unwrap()["state"] = json!("closed");
    state.pull.as_mut().unwrap()["merged_at"] = json!("2026-10-31T01:00:00Z");
    state.pull.as_mut().unwrap()["merge_commit_sha"] = json!("e".repeat(40));
    state.queue_fixture = "merged";
    drop(state);
    webhook(&mut c, &server, "closed");
    drain(
        &mut c,
        &mut source,
        &mut store,
        Phase::Closed(Outcome::Merged),
    );
    let state = server.api.state.lock().unwrap();
    assert!(state.closed);
    assert!(state.comments.iter().any(|c| {
        c["body"]
            .as_str()
            .unwrap()
            .contains("Your change was merged.")
    }));
    assert_eq!(state.proposed.as_ref().unwrap()["state"], "closed");
    assert_eq!(state.proposed_comments.len(), 1);
    assert!(
        state.proposed_comments[0]["body"]
            .as_str()
            .unwrap()
            .contains("https://github.com/RobTand/tl3-sandbox/pull/100")
    );
}
