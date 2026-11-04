use std::path::Path;
use std::process::Command;

pub fn python_command() -> Command {
    let mut command =
        Command::new(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../workers/.venv/bin/python"));
    command.env_clear();
    for key in [
        "PATH",
        "HOME",
        "TMPDIR",
        "TEST_TMPDIR",
        "TEST_SRCDIR",
        "TEST_WORKSPACE",
        "RUNFILES_DIR",
        "RUNFILES_MANIFEST_FILE",
        "JAVA_RUNFILES",
        "PYTHON_RUNFILES",
    ] {
        if let Some(value) = std::env::var_os(key) {
            command.env(key, value);
        }
    }
    command
}

pub fn shared_inputs() -> Vec<String> {
    let mut matrix: Vec<String> =
        serde_json::from_str(include_str!("../../src/redaction/matrix.json")).unwrap();
    assert_eq!(matrix.len(), 196);
    matrix.extend(
        serde_json::from_str::<Vec<String>>(include_str!("../../src/redaction/corpus.json"))
            .unwrap(),
    );
    matrix
}

pub fn python_redact(matrix: &[String]) -> Vec<String> {
    use std::io::Write;
    let mut child = python_command()
            .args(["-I", "-c", "import json,sys; from tl3_workers.tracing import Redactor; r=Redactor(None); print(json.dumps([r.text(s,limit=len(s)) for s in json.load(sys.stdin)]))"])
            .stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped())
            .spawn().unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(&serde_json::to_vec(matrix).unwrap())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    let expected: Vec<String> = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(expected.len(), matrix.len());
    expected
}
