use serde_json::{Value, json};
use std::io::Write;
use std::process::{Command, Stdio};

fn check(command: &str, role: &str, input: Value) -> Value {
    let mut child = Command::new(env!("CARGO_BIN_EXE_tl3"))
        .args([command, role])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(input.to_string().as_bytes()).unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn maker_inputs_report_all_independent_rules() {
    let result = check("check-inputs", "maker", json!({
        "goals": [{"id":"a"}, {"id":"a"}, {"id":"b"}],
        "plan": {"files":["../bad", "/bad"], "checks":[{"goal_ids":["unknown"]}]}
    }));
    let rows = result["violations"].as_array().unwrap();
    assert_eq!(rows.first().unwrap(), "goal ids must be unique");
    assert!(rows.iter().any(|v| v == "unknown goal id"), "{result}");
    assert!(rows.iter().any(|v| v == "checks must cover every maker and checking goal"), "{result}");
    assert!(rows.len() >= 4, "{result}");
}

#[test]
fn classifier_reports_all_independent_rules_in_order() {
    let result = check("check-reply", "classifier", json!({
        "inputs": {"comment":{"author":"requester","body":"comment"},
                   "state":{"closed":true,"phase":"Making","open_question":null}},
        "reply": {"kind":"answer", "question_id":"q1", "answer":"add_credit", "amount":1}
    }));
    assert_eq!(result["violations"], json!([
        "closed requests allow only complaint, remark, unsure or restart",
        "reply answers a question that is not open",
        "reply selects an answer the question does not offer"
    ]));
}

#[test]
fn reviewer_reports_findings_from_different_rules_in_order() {
    let result = check("check-reply", "reviewer", json!({
        "inputs": {"goals":[{"id":"g1","description":"Return hello.","verification":"test"}],
            "base_sha":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "head_sha":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            "diff":"+def unrelated(): return 42\n", "checks":[],
            "standing_rules":[{"id":"request-scope","description":"Request goals only."}]},
        "reply": {"verdict":"changes", "head_sha":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            "blocking_findings":[
                {"problem":"Unknown goal", "evidence":"goal", "citation":{"kind":"goal","id":"missing"}},
                {"problem":"Outside scope", "evidence":"no quote", "citation":{"kind":"standing_rule","id":"request-scope"}}
            ], "findings":[]}
    }));
    assert_eq!(result["violations"], json!([
        "blocking finding cites an unknown goal or standing rule",
        "scope blocker must quote a changed hunk"
    ]));
}
