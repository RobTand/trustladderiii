use serde_json::Value;
use tl3_controller::maker_reply;

fn vectors() -> Vec<Value> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let output = std::process::Command::new(root.join("workers/.venv/bin/python"))
        .arg("-c")
        .arg("import json,runpy,sys; sys.path.insert(0, str(__import__('pathlib').Path(sys.argv[1]).parent)); json.dump(runpy.run_path(sys.argv[1])['vectors'](),sys.stdout,ensure_ascii=False)")
        .arg(root.join("workers/tests/maker_reply_cases.py"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn repair_diagnostics_match_the_verbatim_python_reference() {
    let cases = vectors();
    assert!(cases.len() > 50);
    for case in cases.iter().filter(|case| case["operation"] == "reply") {
        let result = maker_reply::validate(&case["inputs"], &case["reply"]);
        let actual = result.err().map_or(Value::Null, Value::String);
        assert_eq!(actual, case["error"], "{}", case["name"]);
        let output = std::process::Command::new(std::env::var("TL3_BINARY").unwrap())
            .args(["check-reply", "maker"])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .and_then(|mut child| {
                use std::io::Write;
                child.stdin.take().unwrap().write_all(
                    serde_json::to_string(
                        &serde_json::json!({"inputs":case["inputs"], "reply":case["reply"]}),
                    )
                    .unwrap()
                    .as_bytes(),
                )?;
                child.wait_with_output()
            })
            .unwrap();
        assert!(output.status.success(), "{}", case["name"]);
        let document: Value = serde_json::from_slice(&output.stdout).unwrap();
        let violations: Vec<_> = case["error"].as_str().into_iter().collect();
        assert_eq!(
            document,
            serde_json::json!({"violations":violations}),
            "{}",
            case["name"]
        );
    }
}

#[test]
fn pending_handoff_matches_the_verbatim_python_reference() {
    for case in vectors()
        .iter()
        .filter(|case| case["operation"] == "handoff")
    {
        assert_eq!(
            maker_reply::checking_handoff(&case["inputs"], "head", "tree"),
            case["handoff"],
            "{}",
            case["name"]
        );
    }
}

#[test]
fn extracted_loop_preserves_generated_io_schedules() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let output = std::process::Command::new(root.join("workers/.venv/bin/python"))
        .arg("-c")
        .arg("import pathlib,sys,pytest; sys.path.insert(0,str(pathlib.Path(sys.argv[1]).parent)); raise SystemExit(pytest.main([sys.argv[1],'-v']))")
        .arg(root.join("workers/tests/test_maker_reply_schedule.py"))
        .env("TEST_REAL_CHECKER", "1")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("12 passed"));
}
