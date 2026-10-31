mod support;

use postgres::NoTls;
use std::{
    fs::{self, File, FileTimes},
    os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt, symlink},
    path::{Path, PathBuf},
    process::Command,
    sync::{Arc, Barrier},
    thread,
    time::{Duration, Instant, SystemTime},
};
use support::Database;

fn main() {
    let args: Vec<_> = std::env::args().collect();
    if let Some(case) = args.get(1) {
        match case.as_str() {
            "lifecycle" => lifecycle(),
            "concurrent" => concurrent(),
            "safety" => safety(),
            _ => panic!("unknown fixture case {case}"),
        }
        return;
    }
    let cache_home = std::env::home_dir().unwrap().join(".cache");
    fs::create_dir_all(&cache_home).unwrap();
    let mut failed = Vec::new();
    for case in ["lifecycle", "concurrent", "safety"] {
        let scratch = tempfile::tempdir().unwrap();
        let cache = tempfile::Builder::new()
            .prefix("tl3-pg-cache-regression-")
            .tempdir_in(&cache_home)
            .unwrap();
        let status = Command::new(std::env::current_exe().unwrap())
            .arg(case)
            .env("TMPDIR", scratch.path())
            .env("XDG_CACHE_HOME", cache.path())
            .status()
            .unwrap();
        println!("fixture {case}: {status}");
        if !status.success() {
            failed.push(case);
        }
    }
    assert!(failed.is_empty(), "fixture regressions failed: {failed:?}");
    println!("3 fixture regression scenarios passed; no skips");
}

fn fixture() -> (Database, PathBuf) {
    let started = Instant::now();
    let mut database = Database::start();
    let config = database.create("fixture_probe");
    let mut client = postgres::Config::new()
        .host(&config.host)
        .port(config.port)
        .user(&config.user)
        .password("tl3-disposable-test-password")
        .dbname(&config.database)
        .connect(NoTls)
        .unwrap();
    let data: String = client.query_one("SHOW data_directory", &[]).unwrap().get(0);
    assert_eq!(
        client.query_one("SELECT 42", &[]).unwrap().get::<_, i32>(0),
        42
    );
    let root = PathBuf::from(data).parent().unwrap().to_owned();
    let (logical, allocated) = bytes(&root);
    println!(
        "fixture measurement: logical_bytes={logical} allocated_bytes={allocated} start_seconds={:.3}",
        started.elapsed().as_secs_f64()
    );
    (database, root)
}

fn bytes(path: &Path) -> (u64, u64) {
    let meta = fs::symlink_metadata(path).unwrap();
    let mut total = (meta.len(), meta.blocks() * 512);
    if meta.is_dir() {
        for entry in fs::read_dir(path).unwrap() {
            let size = bytes(&entry.unwrap().path());
            total.0 += size.0;
            total.1 += size.1;
        }
    }
    total
}

fn old(path: &Path) {
    File::open(path)
        .unwrap()
        .set_times(FileTimes::new().set_modified(SystemTime::now() - Duration::from_secs(7200)))
        .unwrap();
}

fn dead_pid() -> u32 {
    let mut child = Command::new("sh").args(["-c", "exit 0"]).spawn().unwrap();
    let pid = child.id();
    assert!(child.wait().unwrap().success());
    pid
}

fn leftover(name: &str, pid: &str) -> PathBuf {
    let root = std::env::temp_dir().join(name);
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(root.join("data"))
        .unwrap();
    fs::write(root.join("data/postmaster.pid"), pid).unwrap();
    fs::set_permissions(
        root.join("data/postmaster.pid"),
        fs::Permissions::from_mode(0o600),
    )
    .unwrap();
    fs::write(root.join("data/PG_VERSION"), "17\n").unwrap();
    old(&root);
    root
}

fn lifecycle() {
    let (database, root) = fixture();
    drop(database);
    assert!(
        !root.exists(),
        "normal fixture drop left its scratch folder"
    );
    let stale = leftover("tl3-store-pg-killed", &format!("{}\n", dead_pid()));
    let interrupted = leftover("tl3-store-pg-interrupted-setup", "");
    fs::remove_file(interrupted.join("data/postmaster.pid")).unwrap();
    fs::write(interrupted.join("fixture.pid"), format!("{}\n", dead_pid())).unwrap();
    fs::set_permissions(
        interrupted.join("fixture.pid"),
        fs::Permissions::from_mode(0o600),
    )
    .unwrap();
    old(&interrupted);
    let (database, root) = fixture();
    drop(database);
    assert!(!root.exists(), "next fixture left its own scratch folder");
    assert!(!stale.exists(), "next fixture did not reap the killed run");
    assert!(
        !interrupted.exists(),
        "next fixture did not reap interrupted initialization"
    );
    assert_eq!(
        fs::read_dir(std::env::temp_dir()).unwrap().count(),
        0,
        "fixture leaked other temporary folders"
    );
}

fn concurrent() {
    let cache =
        PathBuf::from(std::env::var_os("XDG_CACHE_HOME").unwrap()).join("tl3-test-postgres");
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(cache.join("installing/binaries/17.4.0/bin"))
        .unwrap();
    fs::write(
        cache.join("installing/binaries/17.4.0/bin/postgres"),
        "incomplete install",
    )
    .unwrap();
    fs::write(cache.join("install.lock"), format!("{}\n", dead_pid())).unwrap();
    fs::set_permissions(
        cache.join("install.lock"),
        fs::Permissions::from_mode(0o600),
    )
    .unwrap();
    let barrier = Arc::new(Barrier::new(2));
    let threads: Vec<_> = (0..2)
        .map(|_| {
            let barrier = barrier.clone();
            thread::spawn(move || {
                barrier.wait();
                let (database, root) = fixture();
                let duplicated_binaries = root.join("binaries").exists();
                drop(database);
                assert!(!root.exists());
                assert!(
                    !duplicated_binaries,
                    "fixture still carries its own binary install"
                );
            })
        })
        .collect();
    let outcomes: Vec<_> = threads.into_iter().map(|t| t.join()).collect();
    assert!(
        outcomes.into_iter().all(|r| r.is_ok()),
        "concurrent fixture failed"
    );
    let installs: Vec<_> = fs::read_dir(&cache)
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(
        installs,
        ["17.4.0"],
        "expected exactly one complete install"
    );
    assert!(cache.join("17.4.0/bin/postgres").is_file());
    let (database, root) = fixture();
    drop(database);
    assert!(!root.exists());
}

fn safety() {
    let (live_database, live) = fixture();
    live_database.stop();
    old(&live);
    let legacy_live = leftover(
        "tl3-store-pg-live-postmaster",
        &format!("{}\n", std::process::id()),
    );
    let malformed = leftover("tl3-store-pg-malformed", "not a pid\n");
    let zero = leftover("tl3-store-pg-zero", "0\n");
    let negative = leftover("tl3-store-pg-negative", "-1\n");
    let missing = leftover("tl3-store-pg-unknown", "");
    fs::remove_file(missing.join("data/postmaster.pid")).unwrap();
    let recent = leftover("tl3-store-pg-recent", &format!("{}\n", dead_pid()));
    File::open(&recent)
        .unwrap()
        .set_times(FileTimes::new().set_modified(SystemTime::now()))
        .unwrap();
    let unrelated = leftover("another-project-pg", &format!("{}\n", dead_pid()));
    let link = std::env::temp_dir().join("tl3-store-pg-symlink");
    symlink(&unrelated, &link).unwrap();
    let linked_data = std::env::temp_dir().join("tl3-store-pg-linked-data");
    fs::create_dir(&linked_data).unwrap();
    symlink(unrelated.join("data"), linked_data.join("data")).unwrap();
    old(&linked_data);
    let (database, root) = fixture();
    drop(database);
    assert!(!root.exists());
    for path in [
        &live,
        &legacy_live,
        &malformed,
        &zero,
        &negative,
        &missing,
        &recent,
        &unrelated,
        &link,
        &linked_data,
    ] {
        assert!(path.exists(), "unsafe reaping of {}", path.display());
    }
    drop(live_database);
    assert!(!live.exists());
}
