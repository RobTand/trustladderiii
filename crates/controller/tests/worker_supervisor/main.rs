mod model;

use serde_json::Value;
use std::{
    fs,
    io::{BufRead, BufReader, Read},
    path::Path,
    process::{Child, Command, Stdio},
};

fn command(root: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_tl3-worker-supervisor"));
    command
        .arg(root)
        .stdin(Stdio::null())
        .stdout(Stdio::piped());
    command
}

fn fixture(root: &Path, body: &str) {
    fs::write(root.join("request.json"), b"legacy request\n").unwrap();
    fs::write(
        root.join("command.json"),
        serde_json::to_vec(&["-c", body]).unwrap(),
    )
    .unwrap();
    // The old launch record pins the runtime across a release symlink switch.
    fs::write(
        root.join("launch.json"),
        br#"{"python":"/usr/bin/python3"}"#,
    )
    .unwrap();
}

fn claimed(child: &mut Child) {
    let mut line = String::new();
    BufReader::new(child.stdout.take().unwrap())
        .read_line(&mut line)
        .unwrap();
    assert_eq!(
        line, "claimed\n",
        "the legacy handshake must be byte-identical"
    );
}

fn check_identity(root: &Path, file: &str) -> Value {
    let bytes = fs::read_to_string(root.join(file)).unwrap();
    let value: Value = serde_json::from_str(&bytes).unwrap();
    assert_eq!(
        bytes,
        format!(
            "{{\"pid\": {}, \"machine\": {}, \"start\": {}, \"boot\": {}}}",
            value["pid"], value["machine"], value["start"], value["boot"]
        )
    );
    assert!(value["pid"].as_u64().unwrap() > 0);
    assert!(value["start"].as_u64().is_some());
    assert_eq!(
        value["machine"],
        fs::read_to_string("/etc/machine-id").unwrap().trim()
    );
    assert_eq!(
        value["boot"],
        fs::read_to_string("/proc/sys/kernel/random/boot_id")
            .unwrap()
            .trim()
    );
    value
}

fn exit_code(root: &Path) -> i64 {
    let bytes = fs::read_to_string(root.join("exit.json")).unwrap();
    let value: Value = serde_json::from_str(&bytes).unwrap();
    assert_eq!(
        bytes,
        format!(
            "{{\"code\": {}, \"finished_ms\": {}}}",
            value["code"], value["finished_ms"]
        )
    );
    assert!(value["finished_ms"].as_u64().unwrap() > 0);
    value["code"].as_i64().unwrap()
}

#[test]
fn legacy_protocol_fixture_is_byte_compatible() {
    for legacy in [true, false] {
        for (body, expected) in [
            (
                "import sys; print(sys.stdin.read(), end=''); print('error', file=sys.stderr); sys.exit(7)",
                7,
            ),
            (
                "import os, signal; os.kill(os.getpid(), signal.SIGTERM)",
                -15,
            ),
        ] {
            let root = tempfile::tempdir().unwrap();
            fixture(root.path(), body);
            let mut launch = if legacy {
                let mut command = Command::new("/usr/bin/python3");
                // Freeze the pre-migration executable as a protocol fixture, never production code.
                fs::write(
                    root.path().join("supervisor.py"),
                    include_bytes!("legacy.py"),
                )
                .unwrap();
                command
                    .arg(root.path().join("supervisor.py"))
                    .arg(root.path())
                    .stdout(Stdio::piped());
                command
            } else {
                command(root.path())
            };
            let mut child = launch.spawn().unwrap();
            claimed(&mut child);
            assert!(child.wait().unwrap().success());
            check_identity(root.path(), "claim.json");
            let worker = check_identity(root.path(), "process.json");
            let bytes = fs::read_to_string(root.path().join("worker-started.json")).unwrap();
            let started: Value = serde_json::from_str(&bytes).unwrap();
            assert_eq!(
                bytes,
                format!("{{\"started_ms\": {}}}", started["started_ms"])
            );
            assert_eq!(exit_code(root.path()), expected);
            assert!(
                !Path::new(&format!("/proc/{}", worker["pid"])).exists(),
                "waitpid must reap the worker"
            );
            if expected == 7 {
                assert_eq!(
                    fs::read(root.path().join("stdout.jsonl")).unwrap(),
                    b"legacy request\n"
                );
                assert_eq!(
                    fs::read(root.path().join("stderr.log")).unwrap(),
                    b"error\n"
                );
            }
        }
    }
}

#[test]
fn old_claims_fence_duplicate_and_cancelled_launches() {
    for name in ["claim.json", "process.json"] {
        let root = tempfile::tempdir().unwrap();
        fixture(
            root.path(),
            "raise AssertionError('a cancelled run executed')",
        );
        let record = b"{\"cancelled\": true}";
        fs::write(root.path().join(name), record).unwrap();
        let mut launch = command(root.path());
        if name == "process.json" {
            launch.arg("--worker");
        }
        let output = launch.output().unwrap();
        assert!(output.status.success());
        assert_eq!(fs::read(root.path().join(name)).unwrap(), record);
        assert!(!root.path().join("worker-started.json").exists());
        assert!(!root.path().join("exit.json").exists());
    }
}

#[test]
fn real_trace_conforms_across_restart_duplicate_and_new_generation() {
    for legacy in [false, true] {
        use model::{Action::*, Protocol};
        use stateright::Model;
        let model = Protocol { mutant: false };
        let mut state = model.init_states().pop().unwrap();
        let mut trace = |action| {
            state = model
                .next_state(&state, action)
                .expect("real event must be enabled in model");
        };
        let old = tempfile::tempdir().unwrap();
        // Pipe EOF, rather than a delay, releases the real worker after the controller disconnects.
        fixture(old.path(), "import sys; sys.stdin.read(); print('done')");
        // request.json is a FIFO so the worker stays live until the test closes its writer.
        fs::remove_file(old.path().join("request.json")).unwrap();
        rustix::fs::mknodat(
            rustix::fs::CWD,
            old.path().join("request.json"),
            rustix::fs::FileType::Fifo,
            rustix::fs::Mode::RUSR | rustix::fs::Mode::WUSR,
            0,
        )
        .unwrap();
        let mut owner = if legacy {
            fs::write(
                old.path().join("supervisor.py"),
                include_bytes!("legacy.py"),
            )
            .unwrap();
            Command::new("/usr/bin/python3")
                .arg(old.path().join("supervisor.py"))
                .arg(old.path())
                .stdout(Stdio::piped())
                .spawn()
                .unwrap()
        } else {
            let image = old.path().join("supervisor-image");
            fs::copy(env!("CARGO_BIN_EXE_tl3-worker-supervisor"), &image).unwrap();
            Command::new(image)
                .arg(old.path())
                .stdout(Stdio::piped())
                .spawn()
                .unwrap()
        };
        claimed(&mut owner);
        if !legacy {
            // The owner is blocked opening the FIFO before spawn. A deploy can
            // replace its installed image in this exact window.
            fs::remove_file(old.path().join("supervisor-image")).unwrap();
        }
        trace(Claim(0, 0));
        let input = fs::OpenOptions::new()
            .write(true)
            .open(old.path().join("request.json"))
            .unwrap();
        // process.json is observed via inotify, without a timed rescan.
        wait_for_file(old.path(), "worker-started.json");
        trace(Spawn(0, 0));
        trace(Start(0));
        trace(CrashController);
        let duplicate = command(old.path()).output().unwrap();
        assert!(duplicate.status.success());
        assert!(duplicate.stdout.is_empty());
        trace(Claim(0, 1));
        trace(RestartController);
        assert!(!old.path().join("exit.json").exists());
        // A pending inspection changes neither the durable files nor the model state.
        drop(input);
        assert!(owner.wait().unwrap().success());
        trace(Exit(0));
        trace(Reap(0, 0));
        trace(Publish(0, 0));
        assert_eq!(exit_code(old.path()), 0);
        let worker = check_identity(old.path(), "process.json");
        assert!(!Path::new(&format!("/proc/{}", worker["pid"])).exists());
        trace(Inspect(0));
        trace(Advance);
        let newer = tempfile::tempdir().unwrap();
        fixture(newer.path(), "print('new generation')");
        let mut next = command(newer.path()).spawn().unwrap();
        claimed(&mut next);
        trace(Claim(1, 0));
        trace(Spawn(1, 0));
        trace(Start(1));
        assert!(next.wait().unwrap().success());
        trace(Exit(1));
        trace(Reap(1, 0));
        trace(Publish(1, 0));
        let current = fs::read(newer.path().join("exit.json")).unwrap();
        assert!(command(old.path()).output().unwrap().status.success());
        trace(Replay(0));
        assert_eq!(fs::read(newer.path().join("exit.json")).unwrap(), current);
        trace(Inspect(1));
        assert_eq!(state.applied, [1, 1]);
    }
}

fn wait_for_file(root: &Path, name: &str) {
    use rustix::fs::inotify;
    let fd = inotify::init(inotify::CreateFlags::CLOEXEC).unwrap();
    inotify::add_watch(
        &fd,
        root,
        inotify::WatchFlags::MOVED_TO | inotify::WatchFlags::CREATE,
    )
    .unwrap();
    let mut input = fs::File::from(fd);
    while !root.join(name).exists() {
        input.read(&mut [0; 4096]).unwrap();
    }
}
