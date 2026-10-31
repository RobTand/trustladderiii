use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};
use tl3_controller::executor::Output as Out;
use tl3_controller::{Repository, adapters::worktrees::Real, executor::Delivery};
use tl3_kernel::{EffectKind, NodeId};

pub fn git(path: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .current_dir(path)
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap())
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .args([
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@localhost",
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().into()
}

pub fn identity() -> Repository {
    Repository {
        owner: "RobTand".into(),
        name: "tl3-sandbox".into(),
        installation: 1,
    }
}

pub struct Fixture {
    pub root: PathBuf,
    pub repo: PathBuf,
    pub remote: PathBuf,
    pub source: PathBuf,
}
impl Fixture {
    pub fn create(root: &Path) -> Self {
        let source = root.join("source");
        fs::create_dir_all(&source).unwrap();
        git(&source, &["init", "-b", "main"]);
        commit(&source, "shared.txt", "base\n");
        let remote = root.join("remote.git");
        git(
            root,
            &[
                "clone",
                "--bare",
                source.to_str().unwrap(),
                remote.to_str().unwrap(),
            ],
        );
        let repo = root.join("served.git");
        git(
            root,
            &[
                "clone",
                "--bare",
                remote.to_str().unwrap(),
                repo.to_str().unwrap(),
            ],
        );
        Self::open(root)
    }
    pub fn open(root: &Path) -> Self {
        Self {
            root: root.into(),
            repo: root.join("served.git"),
            remote: root.join("remote.git"),
            source: root.join("source"),
        }
    }
    pub fn adapter(&self) -> Real {
        self.adapter_at(0)
    }
    pub fn adapter_at(&self, epoch: i64) -> Real {
        let mut adapter = Real::new(self.root.join("worktrees"), epoch).unwrap();
        adapter
            .serve_local(identity(), &self.repo, &self.remote)
            .unwrap();
        adapter
    }
    pub fn delivery(&self, kind: EffectKind, seq: u64) -> Delivery {
        Delivery {
            key: format!("effect:0:{seq}"),
            epoch: 1,
            output: Out::Effect {
                node: NodeId(0),
                seq,
                kind,
                head: None,
            },
            generation: seq,
            head_sha: None,
            repository: identity(),
            issue: 42,
            pull_request: None,
            paths: vec![],
            attempt: 1,
            round: 1,
            replanned: false,
            checks: None,
            job_generation: None,
            question: None,
            marker_key: None,
            note: None,
            question_context: None,
            steering: None,
            request_text: None,
        }
    }
    pub fn advance_main(&self, conflict: bool) -> String {
        commit(
            &self.source,
            if conflict { "shared.txt" } else { "main.txt" },
            &format!(
                "main advance {}\n",
                git(&self.source, &["rev-parse", "HEAD"])
            ),
        );
        git(
            &self.source,
            &["push", self.remote.to_str().unwrap(), "main"],
        );
        git(&self.source, &["rev-parse", "HEAD"])
    }
    pub fn head(&self, delivery: &Delivery) -> String {
        git(
            &self.repo,
            &["rev-parse", Real::branch(delivery).unwrap().as_str()],
        )
    }
}
pub fn commit(path: &Path, file: &str, text: &str) -> String {
    fs::write(path.join(file), text).unwrap();
    git(path, &["add", "--all"]);
    git(path, &["commit", "-m", "fixture change"]);
    git(path, &["rev-parse", "HEAD"])
}
