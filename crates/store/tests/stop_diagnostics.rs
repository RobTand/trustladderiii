mod support;

use postgres::NoTls;
use std::{fs, path::PathBuf, process::Command};

const MARKER: &str = "tl3-stop-diagnostics-server-marker";
const OLD: &str = "tl3-stop-diagnostics-old-prefix";

// Restore the owned server's pidfile before Database's destructor stops it,
// including when the forced failure unwinds.
struct RestorePidfile(PathBuf);

impl Drop for RestorePidfile {
    fn drop(&mut self) {
        fs::rename(
            self.0.join("postmaster.pid.hidden"),
            self.0.join("postmaster.pid"),
        )
        .expect("restore disposable server pidfile for cleanup");
    }
}

#[test]
fn failed_stop_retains_server_diagnostics() {
    if std::env::var_os("TL3_STOP_DIAGNOSTICS_CHILD").is_some() {
        let mut database = support::Database::start();
        let settings = database.create("stop_diagnostics");
        let mut client = postgres::Config::new()
            .host(&settings.host)
            .port(settings.port)
            .user(&settings.user)
            .password("tl3-disposable-test-password")
            .dbname(&settings.database)
            .connect(NoTls)
            .unwrap();
        let data: String = client.query_one("SHOW data_directory", &[]).unwrap().get(0);
        let data = PathBuf::from(data);
        let root = data.parent().unwrap().to_owned();
        fs::write(
            std::env::var_os("TL3_STOP_DIAGNOSTICS_ROOT").unwrap(),
            root.to_str().unwrap(),
        )
        .unwrap();
        // A prefix larger than the printed tail must remain only in the artifact.
        fs::write(
            data.join("start.log"),
            format!("{OLD}\n{}\n", "x".repeat(65536)),
        )
        .unwrap();
        client
            .batch_execute(&format!("DO $$ BEGIN RAISE LOG '{MARKER}'; END $$"))
            .unwrap();
        drop(client);
        fs::rename(
            data.join("postmaster.pid"),
            data.join("postmaster.pid.hidden"),
        )
        .unwrap();
        let _restore = RestorePidfile(data);
        database.stop();
        panic!("pg_ctl unexpectedly accepted a missing pidfile");
    }

    let outputs = std::env::var_os("TEST_UNDECLARED_OUTPUTS_DIR")
        .or_else(|| std::env::var_os("BUILDBUDDY_ARTIFACTS_DIRECTORY"));
    let artifacts = match outputs {
        Some(root) => tempfile::Builder::new()
            .prefix("stop-regression-")
            .tempdir_in(root),
        None => tempfile::Builder::new()
            .prefix("stop-regression-")
            .tempdir(),
    }
    .unwrap();
    let root_record = artifacts.path().join("fixture-root.txt");
    let result = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "failed_stop_retains_server_diagnostics",
            "--nocapture",
        ])
        .env("TL3_STOP_DIAGNOSTICS_CHILD", "1")
        .env("TL3_STOP_DIAGNOSTICS_ROOT", &root_record)
        .env("TEST_UNDECLARED_OUTPUTS_DIR", artifacts.path())
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&result.stderr);
    println!(
        "forced-stop child stdout:\n{}",
        String::from_utf8_lossy(&result.stdout)
    );
    println!("forced-stop child stderr:\n{stderr}");
    assert!(!result.status.success());
    assert!(
        stderr.contains("DatabaseStopError"),
        "original error missing"
    );
    assert!(stderr.contains("stdout=") && stderr.contains("stderr="));
    assert!(stderr.contains("postmaster.pid") && stderr.contains("does not exist"));
    let root = fs::read_to_string(&root_record).unwrap();
    assert!(!PathBuf::from(root).exists(), "fixture cleanup did not run");
    assert!(
        stderr.contains(MARKER),
        "server log tail missing from stop failure"
    );
    assert!(!stderr.contains(OLD), "printed tail includes old prefix");
    assert!(stderr.len() < 20000, "stop failure output exceeded bound");
    let logs: Vec<_> = fs::read_dir(artifacts.path())
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.is_dir())
        .collect();
    assert_eq!(logs.len(), 1, "expected one retained server log directory");
    let files: Vec<_> = fs::read_dir(&logs[0])
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(files, ["server.log"]);
    let retained = fs::read_to_string(logs[0].join("server.log")).unwrap();
    assert!(retained.contains(OLD) && retained.contains(MARKER));
    println!("retained server log verified after fixture cleanup; no skips");
    let _ = artifacts.keep();
}
