use std::{fs, path::Path, time::Duration};
use tl3_controller::adapters::prismabuild::{Adapter, Settings};
use tl3_prismabuild::{Demand, Options, PrismaBuild, Tool};

pub fn key() -> String {
    "a".repeat(64)
}

pub fn adapter(root: &Path) -> Adapter {
    adapter_with_bell(root, "http://127.0.0.1:9/")
}

pub fn adapter_with_bell(root: &Path, bell_url: &str) -> Adapter {
    let queue = root.join("queue");
    for state in ["ready", "claimed", "done", "failed", "withdrawn"] {
        fs::create_dir_all(queue.join(state)).unwrap();
    }
    let tool = Tool {
        program: "/never-submit-from-wait-fixtures".into(),
        args: vec![],
    };
    let client = PrismaBuild::new(Options {
        queue_root: queue,
        tool: tool.clone(),
        bell_url: bell_url.into(),
    })
    .unwrap();
    Adapter::new(
        client,
        Settings {
            records: root.join("records"),
            harness: root.join("harness"),
            python: vec!["python3".into()],
            image: format!("sha256:{}", "a".repeat(64)),
            demand: Demand {
                cpus: 1.try_into().unwrap(),
                mem_gb: 2,
                gpu: false,
            },
            quiet: Duration::from_secs(120),
            priority: -10,
            result_tool: tool,
        },
    )
    .unwrap()
}

pub fn bind(root: &Path, checkout: &Path) -> String {
    use std::process::Command;
    fs::create_dir_all(checkout).unwrap();
    let git = |args: &[&str]| {
        let out = Command::new("git")
            .current_dir(checkout)
            .args([
                "-c",
                "user.name=Fixture",
                "-c",
                "user.email=fixture@example.invalid",
                "-c",
                "core.hooksPath=/dev/null",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap().trim().to_owned()
    };
    if !checkout.join(".git").exists() {
        git(&["init", "--quiet", "--template=", "-b", "main"]);
        git(&[
            "commit",
            "--quiet",
            "--allow-empty",
            "-m",
            &checkout.display().to_string(),
        ]);
    }
    let parent = git(&["rev-parse", "HEAD"]);
    fs::write(root.join("wait-parent"), &parent).unwrap();
    parent
}

pub fn record(root: &Path, state: &str) {
    let parent = fs::read_to_string(root.join("wait-parent"))
        .unwrap_or_else(|_| bind(root, &root.join("worktree")));
    for slot in ["ready", "claimed", "done", "failed", "withdrawn"] {
        let _ = fs::remove_file(
            root.join("queue")
                .join(slot)
                .join(format!("{}.json", key())),
        );
    }
    if state == "missing" {
        return;
    }
    let terminal = matches!(state, "done" | "failed" | "withdrawn");
    let record = serde_json::json!({
        "schema": if terminal { "prismaquant.prismabuild.pool_outcome.v1" } else { "prismaquant.prismabuild.pool_item.v1" },
        "action_key": key(), "published_unix": 100, "checkout_root": root.join("worktree"), "checkout_snapshot": {"parent":parent},
        "status": if state == "done" { "executed" } else { "lease_lost" },
        "detail": {"returncode":0},
    });
    fs::write(
        root.join("queue")
            .join(state)
            .join(format!("{}.json", key())),
        serde_json::to_vec(&record).unwrap(),
    )
    .unwrap();
}
