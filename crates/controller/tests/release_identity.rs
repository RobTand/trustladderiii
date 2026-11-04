//! #742: a process decides its release identity once. A Bazel-built binary has
//! no Git tree, so the identity is the digest of the running binary, and a file
//! replaced on disk later must not change the identity of the code that runs.

use std::{env, fs, path::Path, process::Command};

const CHILD: &str = "TL3_RELEASE_IDENTITY_CHILD";
const REPLACE: &str = "release_identity_survives_replacing_the_binary_on_disk";
const RECOVER: &str = "release_identity_recovers_after_a_failed_read";

/// Run one test of this binary in a child process and require that it passed.
fn child(executable: &Path, name: &str, descriptor_limit: Option<u32>) {
    let mut command = match descriptor_limit {
        Some(limit) => {
            let mut shell = Command::new("sh");
            shell
                .arg("-c")
                .arg(format!("ulimit -Sn {limit} && exec \"$0\" \"$@\""))
                .arg(executable);
            shell
        }
        None => Command::new(executable),
    };
    let output = command
        .args(["--exact", name, "--nocapture", "--test-threads=1"])
        .env(CHILD, name)
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "child failed:\n{stdout}\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        stdout.contains("1 passed"),
        "the child ran the test: {stdout}"
    );
}

fn in_child(name: &str) -> bool {
    env::var_os(CHILD).is_some_and(|value| value == name)
}

#[test]
fn release_identity_survives_replacing_the_binary_on_disk() {
    if in_child(REPLACE) {
        return replace_own_binary();
    }
    // The child replaces its own executable, so it runs from a private copy.
    let scratch = tempfile::tempdir().unwrap();
    let copy = scratch.path().join("release_identity");
    fs::copy(env::current_exe().unwrap(), &copy).unwrap();
    child(&copy, REPLACE, None);
}

fn replace_own_binary() {
    let before = tl3_controller::release::id();
    assert!(
        !before.is_empty(),
        "a release identity before the replacement"
    );
    let exe = env::current_exe().unwrap();
    fs::remove_file(&exe).unwrap();
    fs::write(&exe, b"a different release").unwrap();
    assert_eq!(
        tl3_controller::release::id(),
        before,
        "the running binary did not change"
    );
}

#[test]
fn release_identity_recovers_after_a_failed_read() {
    if in_child(RECOVER) {
        return fail_then_recover();
    }
    // A low descriptor limit lets the child use up every descriptor (EMFILE).
    child(&env::current_exe().unwrap(), RECOVER, Some(64));
}

fn fail_then_recover() {
    let mut hoard = Vec::new();
    while let Ok(file) = fs::File::open("/dev/null") {
        hoard.push(file);
    }
    let failed = tl3_controller::release::id();
    drop(hoard);
    let recovered = tl3_controller::release::id();
    if !failed.is_empty() && !failed.starts_with("sha256:") {
        // Built from a Git tree: the identity never reads the binary.
        assert_eq!(recovered, failed);
        return;
    }
    assert_eq!(failed, "", "reading the binary failed with EMFILE");
    assert!(
        recovered.starts_with("sha256:"),
        "a later read recovers the identity: {recovered:?}"
    );
    assert_eq!(tl3_controller::release::id(), recovered);
}
