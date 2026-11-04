use serde_json::Value;
use tl3_controller::redaction;

#[test]
fn generated_policy_matches_committed_file() {
    let committed: Value =
        serde_json::from_str(include_str!("../src/redaction/policy.json")).unwrap();
    let generated: Value = serde_json::from_str(&redaction::generated_policy()).unwrap();
    assert!(generated == committed, "generated policy drift");
}

#[test]
fn generated_policy_retains_every_corpus_rule() {
    let committed: Value =
        serde_json::from_str(include_str!("../src/redaction/policy.json")).unwrap();
    let generated: Value = serde_json::from_str(&redaction::generated_policy()).unwrap();
    for key in ["patterns", "checked", "regex", "words", "characters"] {
        assert_eq!(
            generated[key], committed[key],
            "missing corpus rules: {key}"
        );
    }
}

#[test]
fn migration_preserves_all_original_matching_inputs() {
    use sha2::{Digest, Sha256};
    let policy: Value = serde_json::from_str(&redaction::generated_policy()).unwrap();
    let original: serde_json::Map<String, Value> =
        ["patterns", "checked", "regex", "words", "characters"]
            .into_iter()
            .map(|key| (key.into(), policy[key].clone()))
            .collect();
    // Fingerprint of the pre-migration policy, excluding its Python file hash.
    assert_eq!(
        format!(
            "{:x}",
            Sha256::digest(serde_json::to_vec(&original).unwrap())
        ),
        "0d935405219373054ecc950585c11363c1b3e40713f964973a3620a6474b95f4"
    );
}

#[test]
fn cli_check_rejects_a_hand_edited_policy() {
    use std::process::Command;
    let file = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(file.path(), redaction::generated_policy()).unwrap();
    let check = || {
        Command::new(env!("CARGO_BIN_EXE_tl3"))
            .env_clear()
            .arg("redaction-policy")
            .arg("--check")
            .arg(file.path())
            .output()
            .unwrap()
    };
    assert!(check().status.success());
    let mut policy: Value = serde_json::from_str(&redaction::generated_policy()).unwrap();
    policy["patterns"].as_array_mut().unwrap().remove(2);
    std::fs::write(file.path(), serde_json::to_vec_pretty(&policy).unwrap()).unwrap();
    let output = check();
    assert!(!output.status.success());
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("redaction policy drift")
    );
}

#[test]
fn cli_redacts_stdin_with_the_controller_engine() {
    use std::io::Write;
    use std::process::{Command, Stdio};
    let input = "password: zE9!pR4#cV7$
ordinary log line
";
    let mut child = Command::new(env!("CARGO_BIN_EXE_tl3"))
        .env_clear()
        .arg("redact")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        redaction::text(input).unwrap()
    );
}
