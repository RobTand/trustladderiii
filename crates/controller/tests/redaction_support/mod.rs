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
