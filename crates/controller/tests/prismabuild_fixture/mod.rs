use std::{fs, path::Path, time::Duration};
use tl3_controller::adapters::prismabuild::Settings;
use tl3_prismabuild::{Demand, Options, PrismaBuild, Tool};

pub fn settings(root: &Path, mode: &str, bell_url: &str) -> (PrismaBuild, Settings) {
    let queue = root.join("queue");
    fs::create_dir_all(&queue).unwrap();
    for state in ["ready", "claimed", "done", "failed", "withdrawn"] {
        fs::create_dir_all(queue.join(state)).unwrap();
    }
    fs::write(queue.join("mode"), mode).unwrap();
    let tool = Tool {
        program: "python3".into(),
        args: vec![
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/pb.py")
                .into(),
            queue.clone().into(),
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../prismabuild/tests/fixtures/queue")
                .into(),
        ],
    };
    let client = PrismaBuild::new(Options {
        queue_root: queue,
        tool: tool.clone(),
        bell_url: bell_url.into(),
    })
    .unwrap();
    (
        client,
        Settings {
            records: root.join("records"),
            harness: Path::new(env!("CARGO_MANIFEST_DIR")).join("../../harness/src/tl3_harness"),
            python: vec!["python3.14".into()],
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
}
