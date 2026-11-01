use super::*;
use std::{io::Write, sync::mpsc::Receiver};
use tl3_controller::{Repository, adapters::worktrees::Real};

pub(super) struct Flow {
    controller: Controller,
    receiver: Receiver<Input>,
    config: tl3_config::Config,
    repository: PathBuf,
    remote: PathBuf,
    worktree: PathBuf,
    base: String,
    _checks: adapters::prismabuild::RunningSource,
}

impl Flow {
    pub(super) fn start(settings: &tl3_config::Store, root: &Path) -> Self {
        Self::start_mode(settings, root, "normal", false)
    }

    fn start_mode(settings: &tl3_config::Store, root: &Path, mode: &str, hold_queue: bool) -> Self {
        let (config, source, worktree, base) = repository_fixture(settings, root, mode);
        let remote = root.join("remote.git");
        let repository = root.join("served.git");
        git(
            root,
            &[
                "clone",
                "--bare",
                source.to_str().unwrap(),
                remote.to_str().unwrap(),
            ],
        );
        git(
            root,
            &[
                "clone",
                "--bare",
                remote.to_str().unwrap(),
                repository.to_str().unwrap(),
            ],
        );
        let mut real = Real::new(&config.worktrees_root, 0)
            .unwrap()
            .with_repositories(&config.github.repos)
            .unwrap();
        real.serve_local(
            Repository {
                owner: "RobTand".into(),
                name: "tl3-sandbox".into(),
                installation: 1,
            },
            &repository,
            &remote,
        )
        .unwrap();
        let (mut registry, source, checks_source) = registry(&config, &root.join("fakes"), false);
        registry.register(Upstream::Worktrees, real);
        if hold_queue {
            registry.register(Upstream::Mergify, HeldQueue);
        }
        let controller = Controller::open(
            Store::open(settings).unwrap(),
            registry,
            Bootstrap::from_config(&config).unwrap(),
        )
        .unwrap();
        if mode == "review-feedback" {
            let mut store = Store::open(settings).unwrap();
            let mut commit =
                tl3_store::Commit::new("review-goals", json!({}), &store.load().unwrap());
            commit.rows.push(tl3_store::RowChange::Put {
                table: "goals".into(),
                id: "0".into(),
                value: json!([{"id": "greeting", "description": "Return hello.", "verification": "test"}]),
            });
            store.commit(&commit).unwrap();
        }
        let (sender, receiver) = mpsc::channel();
        let checks = checks_source.spawn(sender.clone());
        thread::spawn(move || source.run(sender).unwrap());
        let mut flow = Self {
            controller,
            receiver,
            config,
            repository,
            remote,
            worktree,
            base,
            _checks: checks,
        };
        flow.controller.intake(labelled()).unwrap();
        flow.until(|phase| phase == Phase::Checking);
        flow
    }

    fn until(&mut self, done: impl Fn(Phase) -> bool) {
        loop {
            let world = self.controller.world().unwrap();
            assert!(
                world.nodes.iter().all(|node| !node.waiting_incident),
                "fixture raised an incident: {:?}",
                world.nodes
            );
            if let Some(node) = world.nodes.first()
                && done(node.phase)
            {
                return;
            }
            assert!(
                !world
                    .nodes
                    .first()
                    .is_some_and(|node| node.phase.is_closed()),
                "fixture closed before reaching its target phase"
            );
            if !self.controller.drive_once().unwrap() {
                assert!(
                    self.controller
                        .snapshot()
                        .unwrap()
                        .pending_effects
                        .iter()
                        .all(|effect| effect.state != tl3_store::EffectState::Intended),
                    "real worktree delivery did not complete"
                );
                let Input::Observation { key, observation } =
                    self.receiver.recv().expect("worker source ended")
                else {
                    panic!("worker observation required")
                };
                observe(
                    &mut self.controller,
                    &key,
                    observation,
                    &self.worktree,
                    &self.base,
                );
            }
        }
    }

    pub(super) fn write_trial(&self, root: &Path) {
        fs::write(
            root.join("maker-trial.json"),
            serde_json::to_vec_pretty(&self.record()).unwrap(),
        )
        .unwrap();
    }

    fn record(&self) -> Value {
        let reply = Store::open(&self.config.store)
            .unwrap()
            .worker_document("0", "maker_replies")
            .unwrap()
            .unwrap();
        let head = reply["head"].as_str().unwrap();
        assert_eq!(git(&self.worktree, &["rev-parse", "HEAD"]), head);
        assert_eq!(
            git(&self.repository, &["rev-parse", "tl3/issue-42-node-0"]),
            head
        );
        let paths = ["greeting.py", "test_greeting.py", "pyproject.toml"];
        let blobs: serde_json::Map<String, Value> = paths
            .into_iter()
            .map(|path| {
                let base = git(
                    &self.repository,
                    &["show", &format!("{}:{path}", self.base)],
                );
                let changed = git(&self.repository, &["show", &format!("{head}:{path}")]);
                (path.into(), json!({"base": base, "head": changed}))
            })
            .collect();
        json!({
            "repository": self.repository, "remote": self.remote, "base": self.base, "head": head,
            "changed_paths": git(&self.worktree, &["diff", "--name-only", &self.base, head]).lines().collect::<Vec<_>>(),
            "blobs": blobs, "maker_reply": reply
        })
    }
}

fn recorded_trial(prefix: &str) -> Value {
    use sha2::{Digest, Sha256};
    let records = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/maker/records");
    let receipt: Value =
        serde_json::from_slice(&fs::read(records.join(format!("{prefix}receipt.json"))).unwrap())
            .unwrap();
    let terminal: Value =
        serde_json::from_slice(&fs::read(records.join(format!("{prefix}terminal.json"))).unwrap())
            .unwrap();
    let payload = fs::read(records.join(format!("{prefix}result.log"))).unwrap();
    assert_eq!(receipt["action_key"], terminal["action_key"]);
    assert_eq!(terminal["exit_code"], 0);
    assert_eq!(
        terminal["result"]["receipt_sha256"],
        receipt["receipt_sha256"]
    );
    assert_eq!(terminal["diagnostics"]["cleanup"]["complete"], true);
    assert_eq!(
        receipt["result"]["sha256"],
        format!("{:x}", Sha256::digest(&payload))
    );
    String::from_utf8(payload)
        .unwrap()
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .find(|value| value["verification"] == "passed" && value.get("maker_trial").is_some())
        .expect("recorded maker-to-harness result")
}

pub(super) fn acceptance(settings: &tl3_config::Store) {
    let record = recorded_trial("");
    let report = &record["report"][0];
    assert_eq!(report["verdict"], "pass");
    assert_eq!(report["kind"], "demonstrating");
    assert_eq!(report["tests"][0]["test"], "test_greeting.py::test_hello");
    assert_eq!(report["tests"][0]["base"], "assertion");
    assert_eq!(report["tests"][0]["head"], "passed");
    assert_eq!(report["custody"], "in-process");
    let root = tempfile::tempdir().unwrap();
    let mut flow = Flow::start(settings, root.path());
    let current = flow.record();
    assert_eq!(
        current["blobs"], record["maker_trial"]["blobs"],
        "the replay uses exactly the code, tests and harness config checked in the trial"
    );
    assert_eq!(
        current["changed_paths"],
        record["maker_trial"]["changed_paths"]
    );
    assert_eq!(current["changed_paths"], json!(["greeting.py"]));
    let head = current["head"].as_str().unwrap();
    flow.until(|phase| phase.is_closed());
    while flow.controller.drive_once().unwrap() {}
    assert_eq!(
        flow.controller.world().unwrap().nodes[0].phase,
        Phase::Closed(tl3_kernel::Outcome::Merged)
    );
    assert!(!flow.worktree.exists());
    assert!(!flow.config.workers.runs.join("sessions/0").exists());
    assert!(git(&flow.remote, &["for-each-ref", "refs/heads/tl3"]).is_empty());
    let journal = flow.repository.join("tl3-controller");
    let pushed = fs::read_dir(journal)
        .unwrap()
        .filter_map(|entry| fs::read(entry.ok()?.path()).ok())
        .filter_map(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .any(|record| record["intent"]["Push"]["head"] == head && !record["completion"].is_null());
    assert!(
        pushed,
        "the real adapter verified publication before teardown"
    );
}

pub(super) fn rebase_revisit(settings: &tl3_config::Store, new_edits: bool) {
    let root = tempfile::tempdir().unwrap();
    let mut flow = Flow::start_mode(
        settings,
        root.path(),
        if new_edits { "rebase-edits" } else { "normal" },
        true,
    );
    flow.until(|phase| phase == Phase::Queued);
    let first = flow.controller.world().unwrap().nodes[0];
    let head = git(&flow.worktree, &["rev-parse", "HEAD"]);
    let upstream = root.path().join("repo");
    fs::write(
        upstream.join("greeting.py"),
        "def greet(): return 'upstream'\n",
    )
    .unwrap();
    git(&upstream, &["add", "."]);
    git(
        &upstream,
        &["commit", "--quiet", "-m", "Conflicting upstream greeting"],
    );
    git(
        &upstream,
        &["push", flow.remote.to_str().unwrap(), "HEAD:main"],
    );
    flow.controller
        .observe(
            "queue-conflict",
            Observation::Kernel(Event::Fact {
                node: NodeId(0),
                fact: tl3_kernel::Fact::Conflict,
            }),
        )
        .unwrap();
    flow.until(|phase| phase == Phase::Making);
    let revisit = flow.controller.world().unwrap().nodes[0];
    assert_eq!(
        (revisit.attempt, revisit.round),
        (first.attempt, first.round)
    );
    assert_ne!(revisit.generation, first.generation);
    while flow.controller.drive_once().unwrap() {}
    let (key, observation) = loop {
        let Input::Observation { key, observation } = flow.receiver.recv().unwrap() else {
            panic!("worker observation")
        };
        if matches!(
            &observation,
            Observation::WorkerReply { generation, .. }
                | Observation::Kernel(Event::Reply { generation, .. })
                if *generation == revisit.generation
        ) {
            break (key, observation);
        }
        observe(
            &mut flow.controller,
            &key,
            observation,
            &flow.worktree,
            &flow.base,
        );
        while flow.controller.drive_once().unwrap() {}
    };
    assert!(
        matches!(&observation, Observation::WorkerReply { reply, .. } if reply["kind"] == "done"),
        "{observation:?}"
    );
    let resolved = git(&flow.worktree, &["rev-parse", "HEAD"]);
    assert_ne!(resolved, head);
    let main = git(&flow.remote, &["rev-parse", "main"]);
    assert_eq!(git(&flow.worktree, &["merge-base", &resolved, &main]), main);
    assert!(git(&flow.worktree, &["status", "--porcelain"]).is_empty());
    if !new_edits {
        // The recorded maker resolves the markers by keeping its leaf's existing content.
        assert_eq!(
            git(&flow.worktree, &["rev-parse", "HEAD^{tree}"]),
            git(&flow.worktree, &["rev-parse", &format!("{head}^{{tree}}")])
        );
        let runs = requests(&flow.config);
        assert_eq!(runs[1]["operation"]["generation"], revisit.generation);
    }
    observe(
        &mut flow.controller,
        &key,
        observation,
        &flow.worktree,
        &flow.base,
    );
    flow.until(|phase| phase == Phase::Checking);
    flow.controller
        .observe(
            "stop-rebase-fixture",
            Observation::Kernel(Event::Command {
                node: NodeId(0),
                id: 1,
                command: tl3_kernel::Command::Stop,
            }),
        )
        .unwrap();
    flow.until(|phase| phase.is_closed());
    while flow.controller.drive_once().unwrap() {}
}

pub(super) fn reproducing_fix(settings: &tl3_config::Store, rebase: bool) {
    let root = tempfile::tempdir().unwrap();
    let mut flow = Flow::start_mode(settings, root.path(), "review-fix", rebase);
    let mut store = Store::open(settings).unwrap();
    let mut commit = tl3_store::Commit::new("review-goals", json!({}), &store.load().unwrap());
    commit.rows.push(tl3_store::RowChange::Put {
        table: "goals".into(),
        id: "0".into(),
        value: json!([{"id":"greeting", "description":"Return hello.", "verification":"test"}]),
    });
    store.commit(&commit).unwrap();
    let reviewed = flow.record()["head"].as_str().unwrap().to_owned();
    if rebase {
        // Force reconciliation to reach Checking before the source seeds this head.
        let mut missing =
            tl3_store::Commit::new("late-first-maker-plan", json!({}), &store.load().unwrap());
        missing.rows.push(tl3_store::RowChange::Delete {
            table: "check_plans".into(),
            id: format!("0:{reviewed}"),
        });
        store.commit(&missing).unwrap();
        let request = requests(&flow.config).pop().unwrap();
        let reply = store
            .worker_document("0", "maker_replies")
            .unwrap()
            .unwrap();
        observe(
            &mut flow.controller,
            "late-first-maker-source-reply",
            Observation::WorkerReply {
                node: NodeId(0),
                generation: request["operation"]["generation"].as_u64().unwrap(),
                role: "maker".into(),
                reply,
            },
            &flow.worktree,
            &flow.base,
        );
        while flow
            .controller
            .snapshot()
            .unwrap()
            .rows
            .iter()
            .any(|row| row.table == "inbox")
        {
            assert!(flow.controller.drive_once().unwrap());
        }
        assert!(
            flow.controller
                .snapshot()
                .unwrap()
                .rows
                .iter()
                .any(|row| { row.table == "check_plans" && row.id == format!("0:{reviewed}") }),
            "a late first maker source reply must seed its missing fixture plan"
        );
    }
    flow.until(|phase| phase == Phase::Making);
    let feedback = store
        .worker_document("0", "maker_findings")
        .unwrap()
        .unwrap();
    assert_eq!(
        feedback.as_array().unwrap().len(),
        2,
        "each blocking finding needs its own mapping"
    );
    for finding in feedback.as_array().unwrap() {
        assert!(finding["id"].is_string(), "review findings need stable ids");
        assert_eq!(finding["reviewed_sha"], reviewed);
    }
    flow.until(|phase| phase == Phase::Checking);
    let fixed = flow.record()["head"].as_str().unwrap().to_owned();
    let snapshot = flow.controller.snapshot().unwrap();
    let run = snapshot
        .rows
        .iter()
        .find(|row| row.table == "check_plans" && row.id == format!("0:{fixed}"))
        .unwrap();
    assert_eq!(run.value["reviewed_sha"], reviewed);
    let checks = run.value["plan"].as_array().unwrap();
    assert_eq!(checks.len(), 2, "the original plan and reproducer both run");
    assert_eq!(checks[1]["kind"], "reproducing");
    assert_eq!(checks[1]["tests"], json!(["test_greeting.py::test_empty"]));
    if rebase {
        flow.until(|phase| phase == Phase::Queued);
        // Reconciliation can consume a reply before its source message is received.
        let request = requests(&flow.config).pop().unwrap();
        let reply = store
            .worker_document("0", "maker_replies")
            .unwrap()
            .unwrap();
        observe(
            &mut flow.controller,
            "delayed-maker-source-reply",
            Observation::WorkerReply {
                node: NodeId(0),
                generation: request["operation"]["generation"].as_u64().unwrap(),
                role: "maker".into(),
                reply,
            },
            &flow.worktree,
            &flow.base,
        );
        while flow.controller.drive_once().unwrap() {}
        let replayed = flow.controller.snapshot().unwrap();
        let preserved = replayed
            .rows
            .iter()
            .find(|row| row.table == "check_plans" && row.id == format!("0:{fixed}"))
            .unwrap();
        assert_eq!(
            preserved.value, run.value,
            "a delayed maker source reply must preserve the enriched check plan"
        );
        let upstream = root.path().join("repo");
        fs::OpenOptions::new()
            .append(true)
            .open(upstream.join("pyproject.toml"))
            .unwrap()
            .write_all(b"\n# upstream configuration\n")
            .unwrap();
        fs::write(upstream.join("conftest.py"), "# upstream fixture\n").unwrap();
        git(&upstream, &["add", "."]);
        git(&upstream, &["commit", "-m", "Upstream configuration"]);
        git(
            &upstream,
            &["push", flow.remote.to_str().unwrap(), "HEAD:main"],
        );
        flow.controller
            .observe(
                "clean-review-fix-rebase",
                Observation::Kernel(Event::Fact {
                    node: NodeId(0),
                    fact: tl3_kernel::Fact::Conflict,
                }),
            )
            .unwrap();
        flow.until(|phase| phase == Phase::Checking);
        flow.until(|phase| phase == Phase::Queued);
        let head = git(&flow.worktree, &["rev-parse", "HEAD"]);
        let snapshot = flow.controller.snapshot().unwrap();
        let plan = snapshot
            .rows
            .iter()
            .find(|row| row.table == "check_plans" && row.id == format!("0:{head}"))
            .unwrap();
        let rebased_reviewed = plan.value["reviewed_sha"].as_str().unwrap_or_else(|| {
            panic!(
                "rebased check plan must retain its reviewed tree: {}",
                plan.value
            )
        });
        assert_ne!(rebased_reviewed, reviewed);
        let evidence = store.worker_document("0", "review_fixes").unwrap().unwrap();
        assert_eq!(evidence["head_sha"], head);
        assert_eq!(evidence["reviewed_sha"], rebased_reviewed);
        let reviews = fs::read_dir(flow.config.workers.runs.join("reviewer-requests"))
            .unwrap()
            .map(|e| {
                serde_json::from_slice::<Value>(&fs::read(e.unwrap().path()).unwrap()).unwrap()
            })
            .collect::<Vec<_>>();
        let next = reviews
            .iter()
            .find(|r| r["inputs"]["head_sha"] == head)
            .unwrap();
        assert_eq!(next["inputs"]["base_sha"], rebased_reviewed);
        assert_eq!(next["inputs"]["check_base_sha"], plan.value["base_sha"]);
        assert_eq!(
            next["inputs"]["review_fixes"]["reviewed_sha"],
            rebased_reviewed
        );
        assert_eq!(
            next["inputs"]["review_fixes"]["findings"][1]["reason"],
            "This is a documentation correction."
        );
        flow.controller
            .observe(
                "rebased-fix-merged",
                Observation::Merged {
                    node: NodeId(0),
                    sha: head,
                    checks_passed: true,
                },
            )
            .unwrap();
    }
    flow.until(|phase| phase.is_closed());
    let reviews = fs::read_dir(flow.config.workers.runs.join("reviewer-requests"))
        .unwrap()
        .map(|entry| {
            serde_json::from_slice::<Value>(&fs::read(entry.unwrap().path()).unwrap()).unwrap()
        })
        .collect::<Vec<_>>();
    let next = reviews
        .iter()
        .find(|request| request["inputs"]["head_sha"] == fixed)
        .unwrap();
    assert_eq!(next["inputs"]["base_sha"], reviewed);
    assert_eq!(next["inputs"]["check_base_sha"], flow.base);
    assert_eq!(next["inputs"]["review_fixes"]["reviewed_sha"], reviewed);
    let fixes = next["inputs"]["review_fixes"]["findings"]
        .as_array()
        .unwrap();
    assert_eq!(fixes[0]["tests"], json!(["test_greeting.py::test_empty"]));
    assert_eq!(fixes[1]["reason"], "This is a documentation correction.");
    assert!(
        fixes[1]["report"]
            .as_str()
            .unwrap()
            .contains("Explain the greeting")
    );
    assert_eq!(
        flow.controller.world().unwrap().nodes[0].phase,
        Phase::Closed(tl3_kernel::Outcome::Merged)
    );
}

struct HeldQueue;
impl Executor for HeldQueue {
    fn execute(&mut self, _: &Delivery) -> tl3_controller::Result<Completion> {
        Ok(Completion::confirmed(vec![]))
    }
    fn reconcile(&mut self, _: &Delivery) -> tl3_controller::Result<Reconciliation> {
        Ok(Reconciliation::Found(Completion::confirmed(vec![])))
    }
}

pub(super) fn outside_paths_bounded(settings: &tl3_config::Store) {
    let root = tempfile::tempdir().unwrap();
    let mut flow = Flow::start(settings, root.path());
    for index in 0..2000 {
        fs::write(
            flow.worktree.join(format!("outside-{index:04}.py")),
            "outside\n",
        )
        .unwrap();
    }
    git(&flow.worktree, &["add", "."]);
    git(
        &flow.worktree,
        &["commit", "--quiet", "-m", "Thousands of outside files"],
    );
    let node = flow.controller.world().unwrap().nodes[0];
    let sha = Store::open(settings)
        .unwrap()
        .worker_document("0", "maker_replies")
        .unwrap()
        .unwrap()["head"]
        .as_str()
        .unwrap()
        .to_owned();
    // External reports can also contain paths longer than the host filesystem allows.
    let mut paths = vec!["é".repeat(4097)];
    paths.extend(
        git(&flow.worktree, &["diff", "--name-only", &flow.base, "HEAD"])
            .lines()
            .map(str::to_owned),
    );
    assert_eq!(paths.len(), 2002);
    flow.controller
        .observe(
            "many-outside-paths",
            Observation::Checks {
                results: vec![],
                node: NodeId(0),
                generation: node.generation,
                action_key: "a".repeat(64),
                base_sha: flow.base.clone(),
                sha,
                changed_paths: paths,
                failed_tests: vec![],
                report: true,
                pass: true,
            },
        )
        .unwrap();
    // Consume this observation before dispatching the fix round.
    assert!(flow.controller.drive_once().unwrap());
    let feedback = Store::open(settings)
        .unwrap()
        .worker_document("0", "maker_findings")
        .unwrap()
        .unwrap();
    let finding = &feedback[0];
    let paths = finding["outside_paths"].as_array().unwrap();
    assert_eq!(paths.len(), 100, "outside path feedback must be bounded");
    assert_eq!(paths[0].as_str().unwrap().chars().count(), 4096);
    assert_eq!(finding["outside_paths_total"], 2001);
    flow.until(|phase| phase == Phase::Checking);
    let runs = requests(&flow.config);
    assert_eq!(runs.last().unwrap()["inputs"]["feedback"][0], *finding);
}

pub(super) fn recorded_failure_feedback(settings: &tl3_config::Store) {
    let record = recorded_trial("failure-");
    let results = &record["failure_report"];
    let message = results[0]["tests"][0]["message"].as_str().unwrap();
    assert!(message.contains("goodbye") && message.contains("hello"));
    assert_eq!(results[0]["verdict"], "fail");
    let root = tempfile::tempdir().unwrap();
    let mut flow = Flow::start(settings, root.path());
    let current = flow.record();
    assert_eq!(current["blobs"], record["maker_trial"]["blobs"]);
    let queue = root.path().join("fakes/checks/queue");
    fs::write(queue.join("mode"), "check_fail").unwrap();
    fs::write(
        queue.join("recorded_results.json"),
        serde_json::to_vec(results).unwrap(),
    )
    .unwrap();
    flow.until(|phase| phase == Phase::Making);
    let findings = Store::open(settings)
        .unwrap()
        .worker_document("0", "maker_findings")
        .unwrap()
        .unwrap();
    assert_eq!(findings[0]["failed_tests"][0]["message"], message);
    flow.until(|phase| phase == Phase::Checking);
    let runs = requests(&flow.config);
    assert_eq!(
        runs.last().unwrap()["inputs"]["feedback"][0]["failed_tests"][0]["message"],
        message
    );
}

pub(super) fn review_feedback(settings: &tl3_config::Store) {
    let root = tempfile::tempdir().unwrap();
    let mut flow = Flow::start_mode(settings, root.path(), "review-feedback", false);
    flow.until(|phase| phase == Phase::Making);
    flow.until(|phase| phase == Phase::Checking);
    let snapshot = flow.controller.snapshot().unwrap();
    let head = git(&flow.worktree, &["rev-parse", "HEAD"]);
    let plan = snapshot
        .rows
        .iter()
        .find(|r| r.table == "check_plans" && r.id == format!("0:{head}"))
        .unwrap();
    assert!(
        plan.value.get("reviewed_sha").is_none(),
        "reasons alone need no reviewed tree"
    );
    let runs = requests(&flow.config);
    assert_eq!(runs.len(), 2);
    assert_eq!(runs[1]["inputs"]["round"], 1);
    let feedback = &runs[1]["inputs"]["feedback"];
    let feedback = feedback.as_array().unwrap();
    assert!(
        feedback.len() >= 3,
        "blocking findings must have individual feedback entries"
    );
    for (index, problem, evidence, goal_ids) in [
        (
            0,
            "The greeting lacks context.",
            "greeting.py returns only hello.",
            json!(["greeting"]),
        ),
        (
            1,
            "The report stream is overwritten.",
            "The reporter replaces stdout.",
            json!([]),
        ),
        (2, "Problem 0", "Evidence 0", json!(["greeting"])),
    ] {
        let report = feedback[index]["report"].as_str().unwrap();
        assert!(
            report.contains(problem) && report.contains(evidence),
            "maker feedback missing {problem} or {evidence}: {report}"
        );
        assert_eq!(feedback[index]["kind"], "review_finding");
        assert_eq!(feedback[index]["goal_ids"], goal_ids);
    }
    assert!(feedback.len() <= 20, "review feedback must be bounded");
    let report: String = feedback
        .iter()
        .map(|finding| finding["report"].as_str().unwrap())
        .collect();
    assert!(!report.contains("Optional style note"));
    assert!(!report.contains(&"é".repeat(1001)));
    assert!(!report.contains(&"界".repeat(1001)));
    assert!(report.chars().count() <= 16000);
    assert!(
        Store::open(settings)
            .unwrap()
            .worker_document("0", "findings")
            .unwrap()
            .is_none()
    );

    flow.until(|phase| matches!(phase, Phase::Planning { .. }));
    assert!(flow.controller.world().unwrap().nodes[0].replanned);
    let findings = Store::open(settings)
        .unwrap()
        .worker_document("0", "findings")
        .unwrap()
        .unwrap();
    assert!(findings.as_array().unwrap().iter().any(|finding| {
        finding.as_str().is_some_and(|text| {
            text.contains("The greeting lacks context.")
                && text.contains("greeting.py returns only hello.")
        })
    }));
}
