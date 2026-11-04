use serde_json::{Value, json};
use std::process::{Command, Stdio};
use tl3_controller::reviewer::validate;

pub fn run() {
    golden_cases();
    println!("test reviewer_golden_cases ... ok");
    cli_contract();
    println!("test reviewer_cli_contract ... ok");
    parsed_citations_and_first_failure();
    println!("test reviewer_parsed_citations ... ok");
    real_worker_repair();
    println!("test reviewer_real_worker_repair ... ok");
}

fn cases() -> Vec<Value> {
    serde_json::from_str(include_str!("reviewer/golden.json")).unwrap()
}

fn golden_cases() {
    let cases = cases();
    assert!(cases.len() >= 50);
    let mut mismatches = Vec::new();
    for case in cases {
        let result = validate(&case["inputs"], &case["reply"]);
        if case["schema_invalid"] == true {
            assert!(result.is_err(), "{case}");
        } else {
            let actual: Vec<_> = result.err().map(|e| e.to_string()).into_iter().collect();
            if json!(actual) != case["violations"] {
                mismatches.push(
                    json!({"case":case["name"],"actual":actual,"expected":case["violations"]}),
                );
            }
        }
    }
    assert!(mismatches.is_empty(), "{mismatches:#?}");
}

fn command(mut command: Command, input: &Value) -> std::process::Output {
    use std::io::Write;
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.to_string().as_bytes())
        .unwrap();
    let result = child.wait_with_output().unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    result
}

fn cli_contract() {
    let case = &cases()[2];
    let mut cli = Command::new(env!("CARGO_BIN_EXE_tl3"));
    cli.args(["check-reply", "reviewer"]);
    let result = command(
        cli,
        &json!({"inputs":case["inputs"], "reply":case["reply"]}),
    );
    assert!(result.stderr.is_empty());
    assert_eq!(
        serde_json::from_slice::<Value>(&result.stdout).unwrap(),
        json!({"violations":case["violations"]})
    );
}

fn real_worker_repair() {
    let root = tempfile::tempdir().unwrap();
    let request = json!({
        "protocol":1,"operation":{"node":1,"generation":0,"kind":"review"},
        "role":"reviewer","model":{"base_url":"http://model.invalid/v1","name":"fixture-model","effort":"high","budget_tokens":32000},
        "key_file":root.path().join("never-read-this-key"),"worktree":root.path(),
        "session_file":root.path().join("session.json"),"inputs":cases()[0]["inputs"],
    });
    let mut python = Command::new(super::worker_package().join(".venv/bin/python"));
    python
        .args(["-c", include_str!("reviewer/worker.py")])
        .env("TL3_BINARY", env!("CARGO_BIN_EXE_tl3"))
        .env("LANGSMITH_TRACING", "false")
        .env("LANGCHAIN_TRACING_V2", "false");
    let result = command(python, &request);
    let report: Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(report["model_turns"], 3);
    assert_eq!(report["replayed"], true);
    assert_eq!(report["reply"]["head_sha"], request["inputs"]["head_sha"]);
    assert!(!root.path().join("forbidden.txt").exists());
}

/// State-table Reviewing: parsing preserves citation membership, kind, and first-failure order.
fn parsed_citations_and_first_failure() {
    use proptest::{prelude::*, test_runner::TestRunner};
    let membership = prop::collection::vec(0u8..10, 0..20);
    let findings = prop::collection::vec((0u8..12, 0u8..3, any::<bool>()), 1..21);
    TestRunner::new(ProptestConfig::with_cases(128))
        .run(&(membership, findings), |(ids, citations)| {
            let mut inputs = cases()[0]["inputs"].clone();
            let mut rules: Vec<_> = ids.iter().map(|id| json!({"id":format!("r{id}"),"description":"rule"})).collect();
            rules.push(json!({"id":"request-scope","description":"scope"}));
            inputs["standing_rules"] = json!(rules);
            inputs["diff"] = json!("+abcdefgh");
            let blockers: Vec<_> = citations.iter().map(|(id,kind,quoted)| {
                let (kind, id) = match kind {
                    0 => ("goal", "g1".to_string()),
                    1 => ("standing_rule", format!("r{id}")),
                    _ => ("standing_rule", "request-scope".to_string()),
                };
                json!({"citation":{"kind":kind,"id":id},"problem":"problem","evidence":if *quoted {"+abcdefgh"} else {"no quote"}})
            }).collect();
            let reply = json!({"verdict":"changes","blocking_findings":blockers,"findings":[]});
            let bytes = serde_json::to_vec(&json!({"inputs":inputs,"reply":reply})).unwrap();
            let roundtrip: Value = serde_json::from_slice(&bytes).unwrap();
            let expected = citations.iter().find_map(|(id,kind,quoted)| match kind {
                1 if !ids.contains(id) => Some("blocking finding cites an unknown goal or standing rule"),
                2 if !quoted => Some("scope blocker must quote a changed hunk"),
                _ => None,
            });
            let actual = validate(&roundtrip["inputs"], &roundtrip["reply"]).err().map(|e| e.to_string());
            prop_assert_eq!(actual.as_deref(), expected);
            Ok(())
        })
        .unwrap();
}
