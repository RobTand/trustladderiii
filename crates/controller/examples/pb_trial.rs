//! Outer-session trial. Its only check execution is the job submitted by the real adapter.
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::mpsc,
    time::Duration,
};
use tl3_controller::{
    Repository,
    adapters::prismabuild::{Adapter, Check, CheckRun, Kind, Settings},
    cli::Input,
    executor::{Delivery, Executor, Observation},
};
use tl3_kernel::{EffectKind, NodeId, Out};
use tl3_prismabuild::{Demand, Options, PrismaBuild, Tool};

fn git(repo: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .current_dir(repo)
        .args([
            "-c",
            "user.name=TL3",
            "-c",
            "user.email=tl3@example.invalid",
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .output()
        .expect("git");
    assert!(out.status.success(), "fixture git command failed");
    String::from_utf8(out.stdout).unwrap().trim().to_owned()
}
fn fixture(root: &Path) -> Delivery {
    let repo = root.join("served");
    fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "--quiet", "-b", "main", "--template="]);
    fs::write(
        repo.join("pyproject.toml"),
        "[tool.tl3.harness]\ncommand = ['python', '-m', 'pytest']\ntest-files = ['test_*.py']\n",
    )
    .unwrap();
    fs::write(repo.join("value.py"), "def value(): return 1\n").unwrap();
    fs::write(
        repo.join("test_value.py"),
        "from value import value\ndef test_value(): assert value() == 2\n",
    )
    .unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "--quiet", "-m", "base"]);
    let base = git(&repo, &["rev-parse", "HEAD"]);
    fs::write(repo.join("value.py"), "def value(): return 2\n").unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "--quiet", "-m", "head"]);
    let head = git(&repo, &["rev-parse", "HEAD"]);
    Delivery {
        key: "effect:0:62".into(),
        epoch: 1,
        output: Out::Effect {
            node: NodeId(0),
            seq: 62,
            kind: EffectKind::SubmitChecks,
            head: None,
        },
        generation: 1,
        head_sha: Some(head),
        repository: Repository {
            owner: "fixture".into(),
            name: "checks".into(),
            installation: 1,
        },
        issue: 62,
        pull_request: None,
        paths: vec!["value.py".into()],
        attempt: 1,
        round: 0,
        job_generation: Some(1),
        checks: Some(CheckRun {
            repository: repo,
            base_sha: base,
            plan: vec![Check {
                name: "value changes".into(),
                kind: Kind::Demonstrating,
                tests: vec!["test_value.py::test_value".into()],
            }],
        }),
    }
}
fn main() {
    assert!(
        std::env::var_os("PRISMABUILD_ACTION_PROGRESS_TOKEN").is_none(),
        "run from the submitting session, outside PrismaBuild"
    );
    let args: Vec<_> = std::env::args().collect();
    assert_eq!(
        args.len(),
        4,
        "usage: pb_trial submit|observe ABSOLUTE_DIRECTORY IMAGE_DIGEST"
    );
    let root = PathBuf::from(&args[2]);
    assert!(root.is_absolute());
    fs::create_dir_all(&root).unwrap();
    let path = root.join("delivery.json");
    let delivery = if path.exists() {
        serde_json::from_slice(&fs::read(&path).unwrap()).unwrap()
    } else {
        let delivery = fixture(&root);
        fs::write(path, serde_json::to_vec(&delivery).unwrap()).unwrap();
        delivery
    };
    let client = PrismaBuild::new(Options {
        queue_root: "/mnt/shared/prismabuild-fleet/pb-queue".into(),
        tool: Tool::published(),
        bell_url: "http://127.0.0.1:9470/".into(),
    })
    .unwrap();
    let mut adapter = Adapter::new(
        client,
        Settings {
            records: root.join("records"),
            harness: std::env::current_dir()
                .unwrap()
                .join("harness/src/tl3_harness"),
            python: vec![
                "/home/rob/.local/bin/uv".into(),
                "run".into(),
                "--python".into(),
                "3.14".into(),
                "--no-project".into(),
                "python".into(),
            ],
            image: args[3].clone(),
            demand: Demand {
                cpus: 1.try_into().unwrap(),
                mem_gb: 2,
                gpu: false,
            },
            quiet: Duration::from_secs(120),
            priority: -10,
            result_tool: Tool {
                program: "python3".into(),
                args: vec!["/mnt/shared/prismabuild-fleet/repo/tools/pbwait.py".into()],
            },
        },
    )
    .unwrap();
    match args[1].as_str() {
        "submit" => {
            adapter.execute(&delivery).unwrap();
            println!(
                "{}",
                serde_json::json!({"action_key": adapter.action_key(&delivery.key).unwrap()})
            );
        }
        "observe" | "observe-failure" => {
            let (sender, receiver) = mpsc::channel();
            let mut source = adapter.source();
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            assert_eq!(
                runtime
                    .block_on(source.poll(&sender, Duration::from_secs(1)))
                    .unwrap(),
                1,
                "wait for PB's cleaned ending before observing"
            );
            match receiver.recv().unwrap() {
                Input::Observation { key, observation } => {
                    assert!(
                        matches!(&observation, Observation::Checks { report, pass, changed_paths, .. } if *report == (args[1] == "observe") && *pass == (args[1] == "observe") && changed_paths == &["value.py"])
                    );
                    println!(
                        "{}",
                        serde_json::json!({"key": key, "observation": observation})
                    );
                }
                _ => panic!("expected check observation"),
            }
        }
        _ => panic!("use submit or observe"),
    }
}
