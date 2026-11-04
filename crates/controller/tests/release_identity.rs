//! #742: a process decides its release identity once. A Bazel-built binary has
//! no Git tree, so the identity is the digest of the running binary, and a file
//! replaced on disk later must not change the identity of the code that runs.

use std::{env, fs, process::Command};

const CHILD: &str = "TL3_RELEASE_IDENTITY_CHILD";
const NAME: &str = "release_identity_survives_replacing_the_binary_on_disk";

#[test]
fn release_identity_survives_replacing_the_binary_on_disk() {
    if env::var_os(CHILD).is_some() {
        return replace_own_binary();
    }
    // The child replaces its own executable, so it runs from a private copy.
    let scratch = tempfile::tempdir().unwrap();
    let copy = scratch.path().join("release_identity");
    fs::copy(env::current_exe().unwrap(), &copy).unwrap();
    let output = Command::new(&copy)
        .args(["--exact", NAME, "--nocapture", "--test-threads=1"])
        .env(CHILD, "1")
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
