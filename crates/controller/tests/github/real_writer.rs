use super::*;
use tl3_controller::adapters::workers::writer::ProcessWriter;

pub(super) fn all() {
    for mode in [
        "multiline",
        "wrong-quote",
        "no-draft",
        "recommendation",
        "no-excerpt",
        "no-text",
        "always-multiline",
    ] {
        run(mode);
    }
}

pub(super) fn run(mode: &str) {
    let package = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../workers")
        .canonicalize()
        .unwrap();
    assert!(
        Command::new(env::var_os("TL3_TEST_UV").unwrap_or_else(|| "uv".into()))
            .args(["sync", "--locked", "--python", "3.14+gil", "--project"])
            .arg(&package)
            .status()
            .unwrap()
            .success()
    );
    let root = tempfile::tempdir().unwrap();
    let marker = root.path().join("model");
    let mut config =
        tl3_config::Config::parse(include_str!("../../../config/example.toml")).unwrap();
    config.workers.python = package.join(".venv/bin/python");
    config.workers.command = vec![
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/worker_model.py")
            .to_str()
            .unwrap()
            .into(),
        format!("writer-{mode}"),
        marker.to_str().unwrap().into(),
    ];
    config.models.key_file = root.path().join("fixture-key");
    fs::write(&config.models.key_file, "fixture-only").unwrap();
    let api = Server::new();
    let writer = ProcessWriter::new(&config, &root.path().join("writer")).unwrap();
    let mut adapter = adapters::github::Adapter::new(
        BTreeMap::from([(3, client(&api.uri()))]),
        writer,
        0,
        Policy::default(),
        &root.path().join("github"),
        Some(tl3_config::OperationsIssue {
            owner: "owner".into(),
            name: "tl3".into(),
            installation_id: 3,
            number: 17,
        }),
    )
    .unwrap();
    let question = matches!(mode, "wrong-quote" | "no-draft" | "recommendation");
    let mut effect = delivery(Notice::Ask { question: 9 });
    if question {
        effect.question = Some(tl3_kernel::Question {
            id: 9,
            kind: tl3_kernel::QKind::NeedsSplitting,
            cont: tl3_kernel::Cont::Unplanned,
            cost: 0,
            clarified: false,
        });
    } else {
        effect.output = Out::Comment {
            node: NodeId(0),
            kind: tl3_controller::executor::CommentKind::Clarify,
        };
        effect.steering = Some(tl3_controller::steering::Task::ClarifyComment {
            question: "Would you like me to stop or continue?".into(),
            excerpt: "Maybe do that".into(),
        });
    }
    assert_eq!(
        adapter
            .execute(&effect)
            .expect("structural violations must retry through the real writer process")
            .ack,
        tl3_kernel::Ack::Confirmed
    );
    let requests: Vec<Value> = fs::read_to_string(marker.with_extension("requests"))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap()["request"]["inputs"].clone())
        .collect();
    assert_eq!(
        requests.len(),
        if mode == "always-multiline" { 3 } else { 2 }
    );
    assert!(requests[0].get("violations").is_none());
    let feedback = requests[1]["violations"].to_string();
    let expected = match mode {
        "multiline" | "always-multiline" | "no-excerpt" | "no-text" => "one line",
        "recommendation" => "outside the options",
        "no-draft" => "wrong reply shape",
        _ => "original issue title exactly",
    };
    assert!(feedback.contains(expected), "missing feedback: {feedback}");
    adapter.execute(&effect).unwrap();
    let state = api.state.lock().unwrap();
    assert_eq!(state.posts, 1);
    let body = state.comments[0]["body"].as_str().unwrap();
    assert!(
        body.contains(if mode == "always-multiline" {
            "I couldn't act on your comment. Could you rephrase it?"
        } else if question {
            "You asked for \"Add a blue button\"."
        } else {
            "About \"Maybe do that\", would you like me to stop or continue?"
        }),
        "unexpected posted reply: {body}"
    );
    println!("test real_writer_{mode} ... ok");
}


pub(super) fn classifier_contract() {
    let root = tempfile::tempdir().unwrap();
    let key = root.path().join("fixture-key");
    fs::write(&key, "fixture-only").unwrap();
    let request = json!({
        "protocol": 1, "operation": {"node": 0, "generation": 0, "kind": "classifier"},
        "role": "classifier", "model": {"base_url": "http://model.invalid/v1", "name": "fixture", "effort": null, "budget_tokens": 1000},
        "key_file": key, "worktree": root.path(),
        "inputs": {"comment": {"author": "owner", "body": "Please pause"}, "state": {"phase": "Making", "closed": false}}
    });
    let package = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../workers");
    let mut child = Command::new(package.join(".venv/bin/python"))
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/worker_model.py"))
        .arg("normal").arg(root.path().join("entered"))
        .stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
    serde_json::to_writer(child.stdin.take().unwrap(), &request).unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    let events: Vec<Value> = String::from_utf8(output.stdout).unwrap().lines()
        .map(|line| serde_json::from_str(line).unwrap()).collect();
    assert!(events.iter().any(|event| event["event"] == "reply" && event["role"] == "classifier" && event["reply"]["kind"] == "pause"));
    println!("test real_worker_contract_classifier ... ok");
}
