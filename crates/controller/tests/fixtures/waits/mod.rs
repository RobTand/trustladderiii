use std::{fs, path::Path, time::Duration};
use tl3_controller::adapters::prismabuild::{Adapter, Settings};
use tl3_prismabuild::{Demand, Options, PrismaBuild, Tool};

pub fn key() -> String {
    "a".repeat(64)
}

pub fn adapter(root: &Path) -> Adapter {
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
        bell_url: "http://127.0.0.1:9/".into(),
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

pub fn record(root: &Path, state: &str) {
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
        "action_key": key(), "published_unix": 100,
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
