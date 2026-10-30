use super::*;
use tl3_kernel::{Command as KernelCommand, Event};

struct Daemon {
    child: std::process::Child,
    root: tempfile::TempDir,
    api: Server,
    store: Store,
    address: SocketAddr,
    marker: std::path::PathBuf,
    runs: std::path::PathBuf,
}
impl Daemon {
    fn start(settings: &tl3_config::Store, mode: &str) -> Self {
        let root = tempfile::tempdir().unwrap();
        let api = Server::new();
        let (config, marker, runs) = worker_daemon_config(settings, root.path(), mode);
        let mut store = Store::open(settings).unwrap();
        store.put_worker_materials("0", &json!({
            "issue": {"number": 12, "title": "Add a blue button", "body": "Return hello.", "author": "owner", "comments": []},
            "repo": {"readme": "Greeting library", "file_list": ["greeting.py"], "materials": [{"path": "greeting.py", "content": "def greet(): return 'hello'\n"}]}
        })).unwrap();
        if matches!(mode, "steering-queue" | "steering-credit") {
            fs::create_dir_all(root.path().join("fakes")).unwrap();
            fs::write(root.path().join("fakes/hold-merges"), "hold").unwrap();
        }
        let mut child = Command::new(env!("CARGO_BIN_EXE_tl3d"))
            .arg("--config")
            .arg(config)
            .arg("--simulate")
            .arg(root.path().join("fakes"))
            .args([
                "--workers",
                "--github-listen",
                "127.0.0.1:0",
                "--github-api",
            ])
            .arg(api.uri())
            .stdin(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let stderr = child.stderr.take().unwrap();
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            for line in BufReader::new(stderr).lines() {
                let line = line.unwrap();
                eprintln!("steering daemon: {line}");
                if let Some(address) = line.strip_prefix("GitHub webhooks listening on ") {
                    let _ = tx.send(address.parse().unwrap());
                }
            }
        });
        let address = rx.recv_timeout(Duration::from_secs(90)).unwrap();
        assert_eq!(
            post(address, "steering-label", false, fixture(false), true),
            202
        );
        Self {
            child,
            root,
            api,
            store,
            address,
            marker,
            runs,
        }
    }
    fn store_settings(&self) -> tl3_config::Store {
        tl3_config::Config::load(self.root.path().join("config.toml"))
            .unwrap()
            .store
    }
    fn node(&mut self) -> Value {
        self.store
            .load()
            .unwrap()
            .rows
            .into_iter()
            .find(|r| r.table == "nodes" && r.id == "0")
            .map(|r| r.value)
            .unwrap_or(Value::Null)
    }
    fn wait(&mut self, mut condition: impl FnMut(&mut Self) -> bool) {
        let start = Instant::now();
        while !condition(self) {
            assert!(self.child.try_wait().unwrap().is_none(), "daemon exited");
            assert!(
                start.elapsed() < Duration::from_secs(90),
                "steering fixture did not finish; node={}",
                self.node()
            );
            thread::sleep(Duration::from_millis(10));
        }
    }
    fn send(&mut self, id: u64, body: &str) {
        let mut fixture: Value =
            serde_json::from_str(include_str!("../fixtures/steering/comment.json")).unwrap();
        fixture["comment"]["id"] = json!(id);
        fixture["comment"]["body"] = json!(body);
        assert_eq!(
            post(self.address, &format!("comment-{id}"), true, fixture, true),
            202
        );
    }
    fn classified(&mut self, id: u64) {
        self.wait(|s| {
            s.store
                .comment_retired(&format!("owner/sandbox/3/{id}"))
                .unwrap()
                || s.store.load().unwrap().rows.iter().any(|r| {
                    r.table == "github_comments"
                        && r.value["comment"]["id"] == id
                        && r.value.get("classification").is_some()
                })
        });
    }
    fn requests(&self) -> Vec<Value> {
        fs::read_to_string(self.marker.with_extension("requests"))
            .unwrap()
            .split_inclusive('\n')
            .filter(|line| line.ends_with('\n'))
            .map(|line| {
                let record: Value = serde_json::from_str(line).unwrap();
                record["request"].clone()
            })
            .collect()
    }
    fn input(&mut self, key: &str, event: Event) {
        let input = Input::Observation {
            key: key.into(),
            observation: Observation::Kernel(event),
        };
        let stdin = self.child.stdin.as_mut().unwrap();
        serde_json::to_writer(&mut *stdin, &input).unwrap();
        writeln!(stdin).unwrap();
        stdin.flush().unwrap();
    }
    fn restart(&mut self) {
        self.child.kill().unwrap();
        self.child.wait().unwrap();
        let mut child = Command::new(env!("CARGO_BIN_EXE_tl3d"))
            .arg("--config")
            .arg(self.root.path().join("config.toml"))
            .arg("--simulate")
            .arg(self.root.path().join("fakes"))
            .args([
                "--workers",
                "--github-listen",
                "127.0.0.1:0",
                "--github-api",
            ])
            .arg(self.api.uri())
            .stdin(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let stderr = child.stderr.take().unwrap();
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            for line in BufReader::new(stderr).lines() {
                let line = line.unwrap();
                eprintln!("recovered steering daemon: {line}");
                if let Some(address) = line.strip_prefix("GitHub webhooks listening on ") {
                    let _ = tx.send(address.parse().unwrap());
                }
            }
        });
        self.address = rx.recv_timeout(Duration::from_secs(90)).unwrap();
        self.child = child;
    }
    fn settled(&mut self) {
        self.wait(|s| {
            let snapshot = s.store.load().unwrap();
            snapshot.pending_effects.is_empty()
                && s.store.worker_progress(None).unwrap().is_empty()
                && !snapshot.rows.iter().any(|r| {
                    matches!(
                        r.table.as_str(),
                        "inbox" | "watches" | "comment_jobs" | "unclassified_comments"
                    )
                })
        });
    }
}
impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        // A controller crash leaves supervisors alive by design. End this fixture's runs.
        let config = tl3_config::Config::load(self.root.path().join("config.toml")).unwrap();
        let supervisor =
            adapters::workers::Supervisor::new(&config, &self.root.path().join("fakes")).unwrap();
        let epoch = self.store.load().unwrap().epoch.get();
        for progress in self.store.worker_progress(None).unwrap() {
            supervisor.stop_run(&progress.key, epoch).unwrap();
        }
    }
}

pub(super) fn commands(settings: &tl3_config::Store) {
    let mut d = Daemon::start(settings, "steering-commands");
    d.wait(|s| {
        s.node()["phase"] == "Auditing"
            && s.node()["effect"].is_null()
            && !s.node()["run"].is_null()
    });
    let before = d.node();
    d.send(10001, "Thanks for the update");
    d.classified(10001);
    assert_eq!(d.node(), before);
    let mut unauthorized: Value =
        serde_json::from_str(include_str!("../fixtures/steering/comment.json")).unwrap();
    unauthorized["comment"]["id"] = json!(9999);
    unauthorized["comment"]["body"] = json!("Please stop");
    unauthorized["comment"]["user"]["id"] = json!(43);
    assert_eq!(
        post(d.address, "outsider-comment", true, unauthorized, true),
        202
    );
    d.send(10002, "Please pause");
    d.classified(10002);
    assert_eq!(d.node()["paused"], true);
    assert!(
        !d.store
            .load()
            .unwrap()
            .rows
            .iter()
            .any(|r| r.table == "github_comments" && r.value["comment"]["id"] == 9999)
    );
    d.send(10002, "Please pause");
    d.send(10003, "Please resume");
    d.classified(10003);
    assert_eq!(d.node()["paused"], false);
    let before_unsure = d.node();
    d.send(10004, "Maybe do that");
    d.classified(10004);
    d.wait(|s| {
        s.api.state.lock().unwrap().comments.iter().any(|c| {
            c["body"]
                .as_str()
                .unwrap()
                .contains("Would you like me to stop or continue?")
        })
    });
    assert_eq!(
        d.node(),
        before_unsure,
        "comment uncertainty never changes the kernel"
    );
    let questions = d
        .store
        .load()
        .unwrap()
        .rows
        .into_iter()
        .filter(|r| r.table == "comment_questions")
        .collect::<Vec<_>>();
    assert_eq!(questions.len(), 1);
    assert!(questions[0].id.ends_with("/10004"));
    d.send(10004, "Maybe do that");
    d.restart();
    assert_eq!(d.node(), before_unsure);
    d.send(10005, "Please stop");
    d.classified(10005);
    d.wait(|s| s.node()["phase"] == json!({"Closed": "Stopped"}));
    d.settled();
    let classifies = d
        .requests()
        .iter()
        .filter(|r| r["role"] == "classifier")
        .count();
    assert_eq!(
        classifies, 5,
        "redelivered and unauthorized comments never launch another classifier"
    );
    let state = d.api.state.lock().unwrap();
    assert_eq!(
        state
            .comments
            .iter()
            .filter(|c| c["body"]
                .as_str()
                .unwrap()
                .contains("Would you like me to stop or continue?"))
            .count(),
        1
    );
    drop(state);
    assert!(
        !d.store
            .load()
            .unwrap()
            .rows
            .iter()
            .any(|r| r.table == "comment_questions")
    );
    let requests = d.requests();
    assert!(requests.iter().any(|r| {
        r["role"] == "classifier"
            && r["inputs"]["comment"]["body"] == "Please stop"
            && r["inputs"]["conversation"]["comment_id"]
                .as_str()
                .is_some_and(|id| id.ends_with("/10004"))
    }));
    assert_eq!(fs::read_dir(&d.runs).unwrap().count(), 1);
    assert_eq!(fs::read_dir(d.runs.join(".ready")).unwrap().count(), 0);
    println!("test steering_commands_remark_authorization_and_dedup ... ok");
}

pub(super) fn answers(settings: &tl3_config::Store) {
    let mut d = Daemon::start(settings, "steering-answers");
    d.wait(|s| !s.node()["question"].is_null());
    let original = d.node()["question"]["id"].as_u64().unwrap();
    let before_unsure = d.node();
    d.send(11000, "Maybe do that");
    d.classified(11000);
    assert_eq!(
        d.node(),
        before_unsure,
        "unsure comments are distinct from unclear answers"
    );
    d.send(11001, "My answer is unclear");
    d.classified(11001);
    let clarified = d.node()["question"]["id"].as_u64().unwrap();
    assert_ne!(original, clarified);
    assert_eq!(d.node()["question"]["clarified"], true);
    d.send(11002, "My answer is unclear");
    d.classified(11002);
    assert_eq!(d.node()["question"]["id"], clarified);
    d.settled();
    let asks = d
        .api
        .state
        .lock()
        .unwrap()
        .comments
        .iter()
        .filter(|c| {
            c["body"]
                .as_str()
                .unwrap()
                .contains("Could you clarify your answer?")
        })
        .count();
    assert_eq!(asks, 1);
    // Start a classifier against this question, then supersede it while the model waits.
    // The stale answer must not turn into an answer to a later question.
    d.send(11003, "Answer the old question");
    d.wait(|s| s.marker.with_extension("classifying").exists());
    let next_command = d.node()["last_cmd"].as_u64().unwrap() + 1;
    d.input(
        "stop-question",
        Event::Command {
            node: NodeId(0),
            id: next_command,
            command: KernelCommand::Stop,
        },
    );
    d.wait(|s| s.node()["phase"] == json!({"Closed": "Stopped"}));
    let before = d.node();
    fs::write(d.marker.with_extension("answer"), "go").unwrap();
    d.classified(11003);
    assert_eq!(d.node(), before, "stale answers never enter the kernel");
    d.settled();
    println!("test steering_question_ids_one_clarification_and_stale_answer ... ok");
}

pub(super) fn replacement(settings: &tl3_config::Store) {
    let mut d = Daemon::start(settings, "steering-replacement");
    d.wait(|s| s.node()["phase"] == "Auditing");
    d.send(12001, "Change the request to return goodbye");
    d.classified(12001);
    d.wait(|s| s.node()["phase"] == "Auditing");
    d.send(12001, "Change the request to return goodbye");
    d.classified(12001);
    replacement_assertions(&mut d);
    println!("test steering_replacement_reuses_issue_and_latest_binding ... ok");
}

fn replacement_assertions(d: &mut Daemon) {
    d.wait(|s| {
        s.node()["phase"] == json!({"Closed": "Replaced"})
            && s.store
                .load()
                .unwrap()
                .rows
                .iter()
                .filter(|r| r.table == "requests")
                .count()
                == 2
    });
    d.wait(|s| {
        s.requests()
            .iter()
            .any(|r| r["role"] == "ingest" && r["operation"]["node"] == 1)
    });
    let requests = d.requests();
    assert!(requests.iter().any(|r| r["role"] == "ingest"
        && r["operation"]["node"] == 1
        && r["inputs"]["issue"]["number"] == 12
        && r["inputs"]["issue"]["body"] == "Return goodbye instead of hello."));
    let snapshot = d.store.load().unwrap();
    assert!(
        snapshot
            .rows
            .iter()
            .filter(|r| r.table == "requests")
            .all(|r| r.value["issue"] == 12)
    );
    assert_eq!(
        snapshot
            .rows
            .iter()
            .find(|r| r.table == "requests" && r.id == "1")
            .unwrap()
            .value["source"],
        "Return goodbye instead of hello."
    );
    assert!(d.api.state.lock().unwrap().linked.is_empty());
    assert!(!d.api.state.lock().unwrap().closed);
    let mut label = fixture(false);
    label["issue"]["number"] = json!(12);
    assert_eq!(
        post(d.address, "duplicate-active-intake", false, label, true),
        202
    );
    d.send(12002, "Please pause");
    d.classified(12002);
    let snapshot = d.store.load().unwrap();
    assert_eq!(
        snapshot
            .rows
            .iter()
            .filter(|r| r.table == "requests")
            .count(),
        2
    );
    assert_eq!(
        snapshot
            .rows
            .iter()
            .find(|r| r.table == "nodes" && r.id == "1")
            .unwrap()
            .value["paused"],
        true
    );
    assert_eq!(d.node()["phase"], json!({"Closed": "Replaced"}));
    let mut reader = tl3_store::Reader::open(&d.store_settings()).unwrap();
    let status = tl3_controller::status::load(&mut reader).unwrap();
    assert_eq!(status.requests.len(), 1);
    assert_ne!(status.requests[0].phase, "replaced");
}

pub(super) fn correction(settings: &tl3_config::Store) {
    let mut d = Daemon::start(settings, "steering-correction");
    d.wait(|s| s.node()["phase"] == json!({"Closed": "Merged"}));
    d.settled();
    let closed = d.node();
    d.api.state.lock().unwrap().lose_issue_reply = true;
    d.send(13001, "The result is broken, revert it");
    d.classified(13001);
    d.wait(|s| s.api.state.lock().unwrap().linked.len() == 1);
    d.wait(|s| {
        s.store
            .load()
            .unwrap()
            .rows
            .iter()
            .any(|r| r.table == "request_links" && r.value["authorized_by"] == 42)
    });
    d.wait(|s| {
        s.requests()
            .iter()
            .any(|r| r["role"] == "ingest" && r["inputs"]["issue"]["number"] == 1000)
    });
    let state = d.api.state.lock().unwrap();
    assert!(
        state.linked[0]["body"]
            .as_str()
            .unwrap()
            .contains("requested a revert")
    );
    assert!(
        state.linked[0]["body"]
            .as_str()
            .unwrap()
            .contains("issues/12#issuecomment-13001")
    );
    assert!(
        state.linked[0]["body"]
            .as_str()
            .unwrap()
            .contains("The result is broken, revert it")
    );
    drop(state);
    d.wait(|s| s.store.comment_retired("owner/sandbox/3/13001").unwrap());
    assert!(
        !d.store
            .load()
            .unwrap()
            .rows
            .iter()
            .any(|r| r.table == "github_comments" && r.value["comment"]["id"] == 13001)
    );
    d.restart();
    d.send(13001, "The result is broken, revert it");
    d.send(13002, "Thanks for the update");
    d.classified(13002);
    assert_eq!(d.api.state.lock().unwrap().linked.len(), 1);
    assert_eq!(d.node(), closed, "complaints never reach a closed node");
    println!("test steering_closed_complaint_files_one_correction_with_revert ... ok");
}

pub(super) fn queued_pause(settings: &tl3_config::Store) {
    let mut d = Daemon::start(settings, "steering-queue");
    d.wait(|s| s.node()["phase"] == "Queued");
    d.send(14001, "Please pause");
    d.classified(14001);
    assert_eq!(d.node()["phase"], "Queued");
    assert_eq!(d.node()["paused"], true);
    let head = d.node()["head"].as_u64().unwrap();
    let sha = d
        .store
        .load()
        .unwrap()
        .interned
        .into_iter()
        .find(|i| i.kind == tl3_store::InternKind::Commit && i.id == head as i64)
        .unwrap()
        .value;
    let input = Input::Observation {
        key: "queued-merge".into(),
        observation: Observation::Merged {
            node: NodeId(0),
            sha,
            checks_passed: true,
        },
    };
    let stdin = d.child.stdin.as_mut().unwrap();
    serde_json::to_writer(&mut *stdin, &input).unwrap();
    writeln!(stdin).unwrap();
    stdin.flush().unwrap();
    d.wait(|s| s.node()["phase"] == json!({"Closed": "Merged"}));
    d.settled();
    println!("test steering_pause_allows_queued_pr_to_merge ... ok");
}

pub(super) fn answer_recovery(settings: &tl3_config::Store) {
    let mut d = Daemon::start(settings, "steering-answers");
    d.wait(|s| !s.node()["question"].is_null());
    d.send(15001, "Answer the old question");
    d.wait(|s| s.marker.with_extension("classifying").exists());
    d.restart();
    let jobs: Vec<_> = d
        .store
        .worker_progress(None)
        .unwrap()
        .into_iter()
        .filter(|p| p.key.starts_with("classify:"))
        .collect();
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].attempt, 0);
    fs::write(d.marker.with_extension("answer"), "go").unwrap();
    d.classified(15001);
    d.wait(|s| s.node()["phase"] == json!({"Closed": "Merged"}));
    d.settled();
    assert_eq!(
        d.requests()
            .iter()
            .filter(|r| r["role"] == "classifier")
            .count(),
        1,
        "recovery reattaches the existing classifier"
    );
    println!("test steering_named_answer_applies_after_controller_recovery ... ok");
}

pub(super) fn linked_reconciliation() {
    let api = Server::new();
    let root = tempfile::tempdir().unwrap();
    let mut adapter = adapter(&api.uri(), root.path());
    let mut correction = delivery(Notice::FileReplacement);
    correction.output = tl3_controller::executor::Output::Comment {
        node: NodeId(0),
        kind: tl3_controller::executor::CommentKind::FileCorrection,
    };
    correction.steering = Some(tl3_controller::steering::Task::FileRequest {
        text: "Correct the greeting.".into(),
        comment: 16001,
        correction: true,
        revert: false,
        since: "2026-10-30T01:00:00Z".into(),
        comment_issue: 25,
    });
    api.state.lock().unwrap().lose_issue_reply = true;
    assert!(adapter.execute(&correction).is_err());
    api.state.lock().unwrap().fail_reads = true;
    assert!(matches!(
        adapter.reconcile(&correction).unwrap(),
        Reconciliation::Pending
    ));
    api.state.lock().unwrap().fail_reads = false;
    let Reconciliation::Found(completion) = adapter.reconcile(&correction).unwrap() else {
        panic!("created issue was not reconciled")
    };
    assert!(matches!(
        completion.observations.as_slice(),
        [Observation::LinkedRequest {
            issue: tl3_github::Issue { number: 1000, .. },
            original: NodeId(0),
            ..
        }]
    ));
    adapter.execute(&correction).unwrap();
    let state = api.state.lock().unwrap();
    assert_eq!(state.linked.len(), 1);
    assert!(
        state.linked[0]["body"]
            .as_str()
            .unwrap()
            .contains("pull/25#issuecomment-16001")
    );
    assert!(
        !state.linked[0]["body"]
            .as_str()
            .unwrap()
            .contains("requested a revert")
    );
    drop(state);
    correction.epoch = -1;
    assert!(adapter.execute(&correction).is_err());
    println!("test steering_linked_issue_uncertain_post_reconciles_without_duplicate ... ok");
}

pub(super) fn classifier_failures(settings: &tl3_config::Store) {
    let mut d = Daemon::start(settings, "steering-commands");
    d.wait(|s| s.node()["phase"] == "Auditing");
    d.send(17001, "Fail classification once");
    d.classified(17001);
    assert_eq!(d.node()["paused"], true);
    for (id, body) in [
        (17002, "Always fail classification"),
        (17003, "Return an invalid classification"),
    ] {
        d.send(id, body);
        d.classified(id);
        d.wait(|s| {
            s.api.state.lock().unwrap().comments.iter().any(|c| {
                let text = c["body"].as_str().unwrap();
                text.contains(body) && text.contains("Could you rephrase it?")
            })
        });
        assert_eq!(
            d.requests()
                .iter()
                .filter(|r| r["role"] == "classifier" && r["inputs"]["comment"]["body"] == body)
                .count(),
            3
        );
    }
    let snapshot = d.store.load().unwrap();
    let attempts: Vec<_> = snapshot
        .rows
        .iter()
        .filter(|r| r.table == "github_comments" && r.value["comment"]["id"] == 17001)
        .collect();
    assert_eq!(attempts.len(), 1);
    assert_eq!(
        d.requests()
            .iter()
            .filter(|r| r["role"] == "classifier"
                && r["inputs"]["comment"]["body"] == "Fail classification once")
            .count(),
        2
    );
    let effects = d.store.effects().unwrap();
    for id in [17001, 17002, 17003] {
        let keys: Vec<_> = effects
            .iter()
            .filter(|e| e.key.starts_with(&format!("classify:owner/sandbox/3/{id}")))
            .map(|e| &e.key)
            .collect();
        assert_eq!(keys.len(), if id == 17001 { 2 } else { 3 });
        assert!(keys.iter().any(|key| key.ends_with(":attempt:2")));
    }
    println!("test steering_classifier_retries_and_rephrase_on_failure ... ok");
}

pub(super) fn concurrent_requests(settings: &tl3_config::Store) {
    let mut d = Daemon::start(settings, "steering-answers");
    d.wait(|s| !s.node()["question"].is_null());
    let mut materials = d.store.worker_materials("0").unwrap().unwrap();
    materials["issue"]["number"] = json!(13);
    d.store.put_worker_materials("1", &materials).unwrap();
    let mut label = fixture(false);
    label["issue"]["number"] = json!(13);
    assert_eq!(post(d.address, "second-request", false, label, true), 202);
    d.wait(|s| {
        s.store
            .load()
            .unwrap()
            .rows
            .iter()
            .any(|r| r.table == "requests" && r.id == "1")
    });
    d.send(18001, "Answer the old question");
    d.wait(|s| s.marker.with_extension("classifying").exists());
    let progress = d
        .store
        .worker_progress(Some("0"))
        .unwrap()
        .into_iter()
        .find(|p| p.key.starts_with("classify:"))
        .unwrap();
    assert!(progress.started_ms > 0);
    assert_eq!(progress.attempt, 0);
    assert_eq!(progress.output_offset, 0);
    // The authoritative process identity and claim survive while the classifier waits.
    assert!(
        fs::read_dir(&d.runs)
            .unwrap()
            .filter_map(|e| e.ok())
            .any(|e| e.path().join("attempt-00000000/process.json").exists())
    );
    d.send(18002, "Please pause");
    let mut other = fixture(true);
    other["issue"]["number"] = json!(13);
    other["comment"]["id"] = json!(18003);
    other["comment"]["body"] = json!("Please pause");
    assert_eq!(
        post(d.address, "other-request-comment", true, other, true),
        202
    );
    d.classified(18003);
    let snapshot = d.store.load().unwrap();
    assert_eq!(
        snapshot
            .rows
            .iter()
            .filter(|r| r.table == "comment_jobs")
            .count(),
        1
    );
    assert_eq!(
        snapshot
            .rows
            .iter()
            .find(|r| r.table == "nodes" && r.id == "1")
            .unwrap()
            .value["paused"],
        true
    );
    assert!(!d.requests().iter().any(|r| r["role"] == "classifier"
        && r["inputs"]["comment"]["body"] == "Please pause"
        && r["operation"]["node"] == 0));
    fs::write(d.marker.with_extension("answer"), "go").unwrap();
    d.classified(18002);
    println!("test steering_classifies_per_request_with_durable_liveness ... ok");
}

pub(super) fn guidance(settings: &tl3_config::Store) {
    let mut d = Daemon::start(settings, "steering-guidance");
    d.wait(|s| s.node()["question"]["kind"] == "CantMeet");
    d.send(19001, "Try again using the public API");
    d.classified(19001);
    d.wait(|s| s.marker.with_extension("guided").exists());
    assert!(d.requests().iter().any(|r| {
        r["role"] == "planner"
            && r["inputs"]["findings"]
                .as_array()
                .is_some_and(|f| f.contains(&json!("Use the public API.")))
    }));
    assert_eq!(
        d.node()["phase"],
        json!({"Planning": {"must_split": false}})
    );
    println!("test steering_try_again_passes_guidance_to_planner ... ok");
}

pub(super) fn credit(settings: &tl3_config::Store) {
    let mut d = Daemon::start(settings, "steering-credit");
    d.wait(|s| s.node()["phase"] == "Queued");
    for index in 0..16 {
        if d.node()["question"]["kind"] == "OutOfCredit" {
            break;
        }
        let generation = d.node()["generation"].as_u64().unwrap();
        d.input(
            &format!("credit-conflict:{index}"),
            Event::Fact {
                node: NodeId(0),
                fact: tl3_kernel::Fact::Conflict,
            },
        );
        d.wait(|s| {
            s.node()["generation"].as_u64().unwrap() > generation || !s.node()["question"].is_null()
        });
        d.wait(|s| s.node()["phase"] == "Queued" || !s.node()["question"].is_null());
    }
    assert_eq!(d.node()["question"]["kind"], "OutOfCredit");
    let before = d.node()["credit"]["added"].as_u64().unwrap();
    d.send(20001, "Add 3 credits");
    d.classified(20001);
    d.wait(|s| s.node()["phase"] == "Queued");
    assert_eq!(d.node()["credit"]["added"], before + 3);
    println!("test steering_add_credit_resumes_request ... ok");
}

pub(super) fn answer_replacement(settings: &tl3_config::Store) {
    let mut d = Daemon::start(settings, "steering-answers");
    d.wait(|s| !s.node()["question"].is_null());
    d.send(21001, "Use a smaller request");
    d.classified(21001);
    replacement_assertions(&mut d);
    println!("test steering_answer_change_request_reuses_issue ... ok");
}

pub(super) fn writer_rejection() {
    for failures in [1, usize::MAX] {
        let api = Server::new();
        let root = tempfile::tempdir().unwrap();
        let calls = Arc::new(Mutex::new(vec![]));
        let mut clients = BTreeMap::new();
        clients.insert(3, client(&api.uri()));
        let mut adapter = adapters::github::Adapter::new(
            clients,
            ScriptWriter {
                calls: calls.clone(),
                failures,
            },
            0,
            Policy::default(),
            root.path(),
        )
        .unwrap();
        let mut clarification = delivery(Notice::MeasuresPosted);
        clarification.key = "clarify:writer-rejection".into();
        clarification.output = Out::Comment {
            node: NodeId(0),
            kind: tl3_controller::executor::CommentKind::Clarify,
        };
        clarification.steering = Some(tl3_controller::steering::Task::ClarifyComment {
            question: "Would you like me to stop or continue?".into(),
            excerpt: "Maybe do that".into(),
        });
        assert_eq!(
            adapter.execute(&clarification).unwrap().ack,
            tl3_kernel::Ack::Confirmed
        );
        adapter.execute(&clarification).unwrap();
        let inputs = calls.lock().unwrap();
        assert_eq!(inputs.len(), if failures == 1 { 2 } else { 3 });
        assert!(inputs[1].violations.iter().any(|v| v.contains("leaf")));
        let state = api.state.lock().unwrap();
        assert_eq!(state.posts, 1);
        let text = state.comments[0]["body"]
            .as_str()
            .unwrap()
            .lines()
            .skip(1)
            .collect::<Vec<_>>()
            .join("\n");
        tl3_github::message::check_prose(&text, &Policy::default()).unwrap();
        if failures == usize::MAX {
            assert!(text.contains("I couldn't act on your comment. Could you rephrase it?"));
        } else {
            assert!(text.contains("Would you like me to stop or continue?"));
        }
    }
    println!("test steering_writer_reruns_then_posts_checked_fallback ... ok");
}
