use super::*;
use std::sync::mpsc::Receiver;
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
        let (config, source, worktree, base) = repository_fixture(settings, root, "normal");
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
        let mut real = Real::new(&config.worktrees_root, 0).unwrap();
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
        let (mut registry, source, checks_source) =
            registry(&config, &root.join("fakes"), &worktree, &base, false);
        registry.register(Upstream::Worktrees, real);
        let controller = Controller::open(
            Store::open(settings).unwrap(),
            registry,
            Bootstrap::from_config(&config).unwrap(),
        )
        .unwrap();
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
            if let Some(node) = world.nodes.first()
                && done(node.phase)
            {
                return;
            }
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

pub(super) fn acceptance(settings: &tl3_config::Store) {
    use sha2::{Digest, Sha256};
    let records = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/maker/records");
    let receipt: Value =
        serde_json::from_slice(&fs::read(records.join("receipt.json")).unwrap()).unwrap();
    let terminal: Value =
        serde_json::from_slice(&fs::read(records.join("terminal.json")).unwrap()).unwrap();
    let payload = fs::read(records.join("result.log")).unwrap();
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
    let record = String::from_utf8(payload)
        .unwrap()
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .find(|value| value["verification"] == "passed" && value.get("maker_trial").is_some())
        .expect("recorded maker-to-harness result");
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
