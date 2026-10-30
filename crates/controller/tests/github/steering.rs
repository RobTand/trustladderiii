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
        if mode == "steering-queue" {
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
            s.store.load().unwrap().rows.iter().any(|r| {
                r.table == "github_comments"
                    && r.value["comment"]["id"] == id
                    && r.value.get("classification").is_some()
            })
        });
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
    fn settled(&mut self) {
        self.wait(|s| {
            let snapshot = s.store.load().unwrap();
            snapshot.pending_effects.is_empty()
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
        if let Ok(entries) = fs::read_dir(&self.runs) {
            for entry in entries.flatten().filter(|e| e.path().is_dir()) {
                if let Ok(bytes) = fs::read(entry.path().join("delivery.json")) {
                    let delivery: Delivery = serde_json::from_slice(&bytes).unwrap();
                    let config_path = self.root.path().join("config.toml");
                    let config = tl3_config::Config::load(config_path).unwrap();
                    let supervisor = adapters::workers::Supervisor::new(
                        &config,
                        &self.root.path().join("fakes"),
                    )
                    .unwrap();
                    supervisor.stop_run(&delivery.key, delivery.epoch).unwrap();
                }
            }
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
    assert_eq!(d.node()["phase"], "Auditing");
    d.send(10005, "Please stop");
    d.classified(10005);
    d.wait(|s| s.node()["phase"] == json!({"Closed": "Stopped"}));
    d.settled();
    let classifies = d
        .store
        .worker_progress(None)
        .unwrap()
        .into_iter()
        .filter(|p| p.key.starts_with("classify:"))
        .count();
    assert_eq!(
        classifies, 5,
        "redelivered and unauthorized comments never launch another classifier"
    );
    println!("test steering_commands_remark_authorization_and_dedup ... ok");
}

pub(super) fn answers(settings: &tl3_config::Store) {
    let mut d = Daemon::start(settings, "steering-answers");
    d.wait(|s| !s.node()["question"].is_null());
    let original = d.node()["question"]["id"].as_u64().unwrap();
    d.send(11001, "Maybe do that");
    d.classified(11001);
    let clarified = d.node()["question"]["id"].as_u64().unwrap();
    assert_ne!(original, clarified);
    assert_eq!(d.node()["question"]["clarified"], true);
    d.send(11002, "Maybe do that");
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
    let mut after = d.node();
    after["last_cmd"] = before["last_cmd"].clone();
    assert_eq!(after, before);
    d.settled();
    println!("test steering_question_ids_one_clarification_and_stale_answer ... ok");
}

pub(super) fn replacement(settings: &tl3_config::Store) {
    let mut d = Daemon::start(settings, "steering-replacement");
    d.wait(|s| s.node()["phase"] == "Auditing");
    d.send(12001, "Change the request to return goodbye");
    d.classified(12001);
    d.wait(|s| {
        s.node()["phase"] == json!({"Closed": "Replaced"})
            && s.api.state.lock().unwrap().linked.len() == 1
    });
    d.wait(|s| {
        s.store
            .load()
            .unwrap()
            .rows
            .iter()
            .filter(|r| r.table == "requests")
            .count()
            == 2
    });
    let state = d.api.state.lock().unwrap();
    let issue = &state.linked[0];
    assert!(
        issue["body"]
            .as_str()
            .unwrap()
            .contains("Return goodbye instead of hello.")
    );
    assert!(
        issue["body"]
            .as_str()
            .unwrap()
            .contains("issues/12#issuecomment-12001")
    );
    assert_eq!(issue["labels"][0]["name"], "tl3");
    assert!(!state.closed);
    println!("test steering_replacement_is_linked_and_authorized ... ok");
}

pub(super) fn correction(settings: &tl3_config::Store) {
    let mut d = Daemon::start(settings, "steering-correction");
    d.wait(|s| s.node()["phase"] == json!({"Closed": "Merged"}));
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
    drop(state);
    d.send(13001, "The result is broken, revert it");
    d.send(13002, "Thanks for the update");
    d.classified(13002);
    assert_eq!(d.api.state.lock().unwrap().linked.len(), 1);
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
    d.child.kill().unwrap();
    d.child.wait().unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_tl3d"))
        .arg("--config")
        .arg(d.root.path().join("config.toml"))
        .arg("--simulate")
        .arg(d.root.path().join("fakes"))
        .args([
            "--workers",
            "--github-listen",
            "127.0.0.1:0",
            "--github-api",
        ])
        .arg(d.api.uri())
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
    d.address = rx.recv_timeout(Duration::from_secs(90)).unwrap();
    d.child = child;
    fs::write(d.marker.with_extension("answer"), "go").unwrap();
    d.classified(15001);
    d.wait(|s| s.node()["phase"] == json!({"Closed": "Merged"}));
    d.settled();
    let jobs: Vec<_> = d
        .store
        .worker_progress(None)
        .unwrap()
        .into_iter()
        .filter(|p| p.key.starts_with("classify:"))
        .collect();
    assert_eq!(jobs.len(), 1);
    assert_eq!(
        jobs[0].attempt, 0,
        "recovery reattaches the existing classifier"
    );
    println!("test steering_named_answer_applies_after_controller_recovery ... ok");
}

pub(super) fn linked_reconciliation() {
    let api = Server::new();
    let root = tempfile::tempdir().unwrap();
    let mut adapter = adapter(&api.uri(), root.path());
    let mut correction = delivery(Notice::FileCorrection { revert: false });
    correction.steering = Some(tl3_controller::steering::Task::FileRequest {
        text: "Correct the greeting.".into(),
        comment: 16001,
        correction: true,
        revert: false,
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
            number: 1000,
            original: NodeId(0),
            ..
        }]
    ));
    adapter.execute(&correction).unwrap();
    let state = api.state.lock().unwrap();
    assert_eq!(state.linked.len(), 1);
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
