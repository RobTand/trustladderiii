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
        let (mut registry, source) =
            registry(&config, &root.join("fakes"), &worktree, &base, false);
        registry.register(Upstream::Worktrees, real);
        let controller = Controller::open(
            Store::open(settings).unwrap(),
            registry,
            Bootstrap::from_config(&config).unwrap(),
        )
        .unwrap();
        let (sender, receiver) = mpsc::channel();
        thread::spawn(move || source.run(sender).unwrap());
        let mut flow = Self {
            controller,
            receiver,
            config,
            repository,
            remote,
            worktree,
            base,
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
                self.controller.observe(&key, observation).unwrap();
            }
        }
    }

    pub(super) fn write_trial(&self, root: &Path) {
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
        let blobs: serde_json::Map<String, Value> = paths.into_iter().map(|path| (path.into(), json!({"base": git(&self.repository, &["show", &format!("{}:{path}", self.base)]), "head": git(&self.repository, &["show", &format!("{head}:{path}")])}))).collect();
        let record = json!({"repository": self.repository, "remote": self.remote, "base": self.base, "head": head, "changed_paths": git(&self.worktree, &["diff", "--name-only", &self.base, head]).lines().collect::<Vec<_>>(), "blobs": blobs, "maker_reply": reply});
        fs::write(
            root.join("maker-trial.json"),
            serde_json::to_vec_pretty(&record).unwrap(),
        )
        .unwrap();
    }
}
