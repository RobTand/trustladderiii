use serde_json::Value;
use tl3_controller::maker_reply;

fn vectors() -> Vec<Value> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let output = std::process::Command::new(root.join("workers/.venv/bin/python"))
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
