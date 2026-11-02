//! #427: a writer run carries the operation and trace tags of the request it
//! writes for, like every other worker run.
use serde_json::{Value, json};
use std::fs;
use tl3_controller::adapters::workers::writer::{
    MessageKind, ProcessWriter, WriteFor, WriteRequest, Writer,
};

/// A stand-in worker that records the request it was given.
fn writer(root: &std::path::Path) -> (ProcessWriter, std::path::PathBuf) {
    let script = root.join("writer.py");
    fs::write(
        &script,
        r#"import json, pathlib, sys
request = json.load(sys.stdin)
pathlib.Path(sys.argv[1]).write_text(json.dumps(request))
print(json.dumps({"event": "reply", "role": "writer", "reply": {"text": "The change is delivered."}}), flush=True)
"#,
    )
    .unwrap();
    let seen = root.join("seen.json");
    let mut config = tl3_config::Config::parse(include_str!("../../config/example.toml")).unwrap();
    // The writer copies the model key into its run; a fixture key stands in.
    let key = root.join("model-key");
    fs::write(&key, "fixture-key").unwrap();
    config.models.key_file = key;
    config.workers.python = "/usr/bin/env".into();
    config.workers.command = vec![
        "python3".into(),
        script.to_str().unwrap().into(),
        seen.to_str().unwrap().into(),
    ];
    (
        ProcessWriter::new(&config, &root.join("writer")).unwrap(),
        seen,
    )
}

fn input(request: Option<WriteFor>) -> WriteRequest {
    WriteRequest {
        kind: MessageKind::Summary,
        facts: json!({"issue_title": "A change", "request": "Make a change", "outcome": "Delivered"}),
        violations: vec![],
        request,
    }
}

#[test]
fn a_draft_for_a_request_names_its_node_and_request() {
    let root = tempfile::tempdir().unwrap();
    let (mut writer, seen) = writer(root.path());
    let request = WriteFor {
        node: 7,
        generation: 3,
        repo: "owner/sandbox".into(),
        issue: 12,
        attempt: 2,
        round: 1,
    };
    writer
        .draft("notice:7:writer:0", &input(Some(request)))
        .unwrap();
    let sent: Value = serde_json::from_slice(&fs::read(&seen).unwrap()).unwrap();
    assert_eq!(
        sent["operation"],
        json!({"node": 7, "generation": 3, "kind": "writer"})
    );
    assert_eq!(
        sent["trace"],
        json!({"repo": "owner/sandbox", "issue": 12, "request": 7, "attempt": 2, "round": 1})
    );
    // The worker's inputs stay protocol v1 writer-input.
    assert!(
        sent["inputs"].get("request").is_none(),
        "{}",
        sent["inputs"]
    );
    // A replayed draft reads the stored reply; the worker, and its trace, run once.
    fs::remove_file(&seen).unwrap();
    writer.draft("notice:7:writer:0", &input(None)).unwrap();
    assert!(
        !seen.exists(),
        "a replay must not start a second writer run"
    );
}

#[test]
fn a_draft_for_no_request_says_none() {
    let root = tempfile::tempdir().unwrap();
    let (mut writer, seen) = writer(root.path());
    writer.draft("incident:1:writer:0", &input(None)).unwrap();
    let sent: Value = serde_json::from_slice(&fs::read(&seen).unwrap()).unwrap();
    assert_eq!(
        sent["operation"],
        json!({"node": 0, "generation": 0, "kind": "writer"})
    );
    for key in ["repo", "issue", "request", "attempt", "round"] {
        assert_eq!(sent["trace"][key], "none", "{key}: {}", sent["trace"]);
    }
}
