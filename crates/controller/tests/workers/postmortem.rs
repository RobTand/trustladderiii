use serde_json::{Value, json};
use std::io::Write;
use std::process::{Command, Output, Stdio};

fn command(mut command: Command, input: &[u8]) -> Output {
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(input).unwrap();
    child.wait_with_output().unwrap()
}

fn cli(input: &Value) -> Value {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_tl3"));
    cmd.args(["check-reply", "postmortem"]);
    let output = command(cmd, input.to_string().as_bytes());
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    serde_json::from_slice(&output.stdout).unwrap()
}

pub fn run() {
    let cases: Vec<Value> = serde_json::from_str(include_str!("postmortem/golden.json")).unwrap();
    for case in &cases {
        assert_eq!(
            cli(case),
            json!({"violations":case["violations"]}),
            "golden: {}",
            case["name"]
        );
    }
    println!(
        "test postmortem_golden_cli_contract ... ok ({} vectors)",
        cases.len()
    );
    schema_contract(&cases);
    let mut malformed = cases[0].clone();
    malformed["reply"]["postmortem"]["went_wrong"] = json!(["The maker skipped a test."]);
    assert!(!cli(&malformed)["violations"].as_array().unwrap().is_empty());
    malformed = cases[1].clone();
    malformed["reply"]["postmortem"]["recommendations"][0]["operation"] = json!("execute");
    assert!(!cli(&malformed)["violations"].as_array().unwrap().is_empty());
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_tl3"));
    cmd.args(["check-reply", "postmortem"]);
    assert!(!command(cmd, b"{broken JSON").status.success());
    println!("test postmortem_cli_schema_and_json_boundary ... ok");
    parsed_text_and_counts(&cases);
    println!("test postmortem_parsed_text_and_counts ... ok");
    real_worker(&cases);
    println!("test postmortem_real_worker_repair_and_cache ... ok");
}

/// Q32 / #403: the wire schema enforces the count cap and add text independently.
fn schema_contract(cases: &[Value]) {
    let schema: Value = serde_json::from_str(
        &std::fs::read_to_string(
            super::worker_package().join("src/tl3_workers/schemas/postmortem-reply.json"),
        )
        .unwrap(),
    )
    .unwrap();
    let validator = jsonschema::validator_for(&schema).unwrap();
    let mut reply = cases[1]["reply"].clone();
    let mut recommendation = reply["postmortem"]["recommendations"][0].clone();
    recommendation["operation"] = json!("add");
    recommendation["before"] = json!("");
    recommendation["after"] = json!("Run the tests first.");
    reply["postmortem"]["recommendations"] = json!(vec![recommendation.clone(); 3]);
    assert!(
        validator.is_valid(&reply),
        "schema must accept three valid recommendations"
    );
    reply["postmortem"]["recommendations"] = json!(vec![recommendation.clone(); 4]);
    assert!(
        !validator.is_valid(&reply),
        "schema alone must reject four recommendations"
    );
    for (before, after) in [("existing text", "new text"), ("", "")] {
        recommendation["before"] = json!(before);
        recommendation["after"] = json!(after);
        reply["postmortem"]["recommendations"] = json!([recommendation.clone()]);
        assert!(
            !validator.is_valid(&reply),
            "schema alone must reject invalid add text: {before:?}, {after:?}"
        );
    }
}

fn parsed_text_and_counts(cases: &[Value]) {
    use proptest::{prelude::*, test_runner::TestRunner};
    TestRunner::new(ProptestConfig::with_cases(128))
        .run(
            &prop::collection::vec((0u8..3, "\\PC{0,24}", "\\PC{0,24}"), 0..7),
            |items| {
                let mut reply = cases[1]["reply"].clone();
                let template = reply["postmortem"]["recommendations"][0].clone();
                let mut valid = items.len() <= 3;
                let recommendations: Vec<_> = items
                    .iter()
                    .map(|(operation, before, after)| {
                        let mut item = template.clone();
                        let operation = ["add", "remove", "replace"][usize::from(*operation)];
                        valid &= match operation {
                            "add" => before.is_empty() && !after.is_empty(),
                            "remove" => !before.is_empty() && after.is_empty(),
                            _ => !before.is_empty() && !after.is_empty(),
                        };
                        item["operation"] = json!(operation);
                        item["before"] = json!(before);
                        item["after"] = json!(after);
                        item
                    })
                    .collect();
                reply["postmortem"]["recommendations"] = json!(recommendations);
                let encoded = serde_json::to_vec(&reply).unwrap();
                let decoded: Value = serde_json::from_slice(&encoded).unwrap();
                prop_assert_eq!(&reply, &decoded);
                prop_assert_eq!(
                    tl3_controller::postmortem_reply::validate(&decoded).is_ok(),
                    valid
                );
                Ok(())
            },
        )
        .unwrap();
}

fn real_worker(cases: &[Value]) {
    let root = tempfile::tempdir().unwrap();
    let mut worker = Command::new(super::worker_package().join(".venv/bin/python"));
    worker
        .args(["-c", include_str!("postmortem/worker.py")])
        .env("TL3_BINARY", env!("CARGO_BIN_EXE_tl3"))
        .env("LANGSMITH_TRACING", "false")
        .env("LANGCHAIN_TRACING_V2", "false");
    let input = json!({"worktree":root.path(), "reply":cases[0]["reply"]});
    let output = command(worker, input.to_string().as_bytes());
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&output.stdout).unwrap(),
        cases[0]["reply"]
    );
}
