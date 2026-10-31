//! Subprocess fixtures keep hostile Git variables out of parallel test threads.
use crate::executor::Delivery;
use serde_json::json;
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

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

#[path = "../../store/tests/support/mod.rs"]
#[allow(dead_code)]
pub mod support;

pub fn hostile(name: &str, check: impl FnOnce()) {
    if std::env::var_os("TL3_HOSTILE_ADAPTER_CHILD").is_some() {
        check();
        return;
    }
    let foreign = Repo::new();
    let result = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", name, "--nocapture"])
        .env("TL3_HOSTILE_ADAPTER_CHILD", "1")
        .env("GIT_DIR", foreign.path.join(".git"))
        .env("GIT_WORK_TREE", &foreign.path)
        .env("GIT_INDEX_FILE", foreign.path.join("foreign-index"))
        .env("GIT_CONFIG_COUNT", "2")
        .env("GIT_CONFIG_KEY_0", "protocol.allow")
        .env("GIT_CONFIG_VALUE_0", "always")
        .env("GIT_CONFIG_KEY_1", "credential.helper")
        .env("GIT_CONFIG_VALUE_1", "!exit 99")
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{name}:\n{}\n{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
}

pub struct Repo {
    pub root: tempfile::TempDir,
    pub path: PathBuf,
    pub base: String,
    pub head: String,
}
impl Repo {
    pub fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("repo");
        fs::create_dir(&path).unwrap();
        git(&path, &["init", "--quiet", "--template=", "-b", "main"]);
        fs::write(path.join("README.md"), "base\n").unwrap();
        git(&path, &["add", "."]);
        git(&path, &["commit", "-qm", "base"]);
        let base = git(&path, &["rev-parse", "HEAD"]);
        fs::write(
            path.join("README.md"),
            format!("intended {}\n", path.display()),
        )
        .unwrap();
        git(&path, &["add", "."]);
        git(&path, &["commit", "-qm", "head"]);
        let head = git(&path, &["rev-parse", "HEAD"]);
        Self {
            root,
            path,
            base,
            head,
        }
    }
    pub fn delivery(&self) -> Delivery {
        serde_json::from_value(json!({
            "key":"hostile", "epoch":1,
            "output":{"Effect":{"node":0,"kind":"SubmitChecks","seq":1,"head":null}},
            "generation":1,"head_sha":self.head,
            "repository":{"owner":"RobTand","name":"tl3-sandbox","installation":1},
            "issue":42,"pull_request":null,"paths":["README.md"],"attempt":1,"round":0,
            "checks":{"repository":self.path,"base_sha":self.base,"plan":[
                {"name":"value","kind":"regression","tests":["test_value.py::test_value"]}]}
        }))
        .unwrap()
    }
}

pub fn documents(
    store: &mut tl3_store::Store,
    values: &[(&str, serde_json::Value)],
) -> tl3_store::Snapshot {
    let before = store.load().unwrap();
    let mut commit = tl3_store::Commit::new("fixture-documents", json!(null), &before);
    commit.rows = values
        .iter()
        .map(|(table, value)| tl3_store::RowChange::Put {
            table: (*table).into(),
            id: "0".into(),
            value: value.clone(),
        })
        .collect();
    store.commit(&commit).unwrap();
    store.load().unwrap()
}

pub fn wait_client(root: &Path, parent: &str) -> tl3_prismabuild::PrismaBuild {
    let queue = root.join("queue");
    fs::create_dir_all(queue.join("ready")).unwrap();
    for state in ["claimed", "done", "failed", "withdrawn"] {
        fs::create_dir(queue.join(state)).unwrap();
    }
    let key = "a".repeat(64);
    fs::write(
        queue.join("ready").join(format!("{key}.json")),
        serde_json::to_vec(&json!({
            "schema":"prismaquant.prismabuild.pool_item.v1", "action_key":key,
            "published_unix":100,"attempt_history":[],"checkout_snapshot":{"parent":parent}
        }))
        .unwrap(),
    )
    .unwrap();
    tl3_prismabuild::PrismaBuild::new(tl3_prismabuild::Options {
        queue_root: queue,
        bell_url: "http://127.0.0.1:1/".into(),
        tool: tl3_prismabuild::Tool {
            program: "false".into(),
            args: vec![],
        },
    })
    .unwrap()
}
