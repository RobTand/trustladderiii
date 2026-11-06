use postgresql_embedded::{PostgreSQL, Settings, VersionReq};
use std::{
    collections::HashMap,
    fs,
    path::PathBuf,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};
use tempfile::TempDir;

mod cache;
mod scratch;

pub struct Database {
    postgres: PostgreSQL,
    runtime: tokio::runtime::Runtime,
    scratch: TempDir,
    password_file: PathBuf,
}

impl Database {
    pub fn start() -> Self {
        Self::start_with_fsync(true)
    }

    pub fn start_with_fsync(fsync: bool) -> Self {
        Self::start_with(fsync, 20, false)
    }

    /// One server for a whole test process (#731). Rust never drops a static, so
    /// a watchdog stops Postgres and removes its scratch when this process exits
    /// and the kernel closes the watchdog's stdin. Keep the returned child; never
    /// write to its stdin or wait for it.
    #[allow(
        dead_code,
        reason = "Used by the controller library's shared fixture; this fixture is shared with store tests"
    )]
    pub fn start_shared() -> (Self, Child) {
        Self::start_shared_with(false)
    }

    /// Adapter crash children reserve stdout for their handshake (#785).
    #[allow(dead_code, reason = "Used by standalone controller adapter fixtures")]
    pub fn start_shared_on_stderr() -> (Self, Child) {
        Self::start_shared_with(true)
    }

    fn start_shared_with(stderr: bool) -> (Self, Child) {
        // Parallel `cargo test` threads share this server, and each test opens several sessions.
        let database = Self::start_with(true, 200, stderr);
        let settings = database.postgres.settings();
        let watchdog = Command::new("sh")
            .args([
                "-c",
                r#"cat >/dev/null; "$0" stop -D "$1" -m immediate -s; rm -rf "$2""#,
            ])
            .arg(settings.binary_dir().join("pg_ctl"))
            .arg(&settings.data_dir)
            .arg(database.scratch.path())
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("start the shared Postgres watchdog");
        (database, watchdog)
    }

    fn start_with(fsync: bool, max_connections: u32, stderr: bool) -> Self {
        scratch::reap(&std::env::temp_dir());
        let scratch = tempfile::Builder::new()
            .prefix("tl3-store-pg-")
            .tempdir()
            .unwrap();
        let root = scratch.path();
        fs::write(
            root.join("fixture.pid"),
            format!("{}\n", std::process::id()),
        )
        .unwrap();
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(root.join("fixture.pid"), fs::Permissions::from_mode(0o600))
                .unwrap();
        }
        let started = Instant::now();
        let settings = Settings {
            releases_url: "https://github.com/zonkyio/embedded-postgres-binaries".into(),
            version: VersionReq::parse("=17.4.0").unwrap(),
            installation_dir: cache::directory().join("17.4.0"),
            password_file: root.join("pgpass"),
            data_dir: root.join("data"),
            host: "127.0.0.1".into(),
            port: 0,
            username: "postgres".into(),
            password: "tl3-disposable-test-password".into(),
            temporary: true,
            trust_installation_dir: false,
            socket_dir: None,
            timeout: Some(Duration::from_secs(90)),
            configuration: HashMap::from([
                ("max_connections".into(), max_connections.to_string()),
                ("shared_buffers".into(), "16MB".into()),
                ("max_parallel_workers".into(), "0".into()),
                ("max_worker_processes".into(), "0".into()),
                // postgresql_embedded supplies -F; override it for every recovery test.
                ("fsync".into(), if fsync { "on" } else { "off" }.into()),
                ("synchronous_commit".into(), "on".into()),
            ]),
        };
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .max_blocking_threads(2)
            .build()
            .unwrap();
        cache::install(&settings, &runtime);
        initialize(&settings);
        let mut postgres = PostgreSQL::new(settings);
        runtime
            .block_on(postgres.setup())
            .expect("download and initialize embedded Postgres");
        let diagnostic = format!(
            "embedded Postgres cached setup (fsync={fsync}): {:.2}s",
            started.elapsed().as_secs_f64()
        );
        if stderr {
            eprintln!("{diagnostic}");
        } else {
            println!("{diagnostic}");
        }
        runtime
            .block_on(postgres.start())
            .expect("start embedded Postgres");
        let password_file = root.join("client-password");
        fs::write(&password_file, "tl3-disposable-test-password\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&password_file, fs::Permissions::from_mode(0o600)).unwrap();
        }
        Self {
            postgres,
            runtime,
            scratch,
            password_file,
        }
    }

    pub fn create(&mut self, name: &str) -> tl3_config::Store {
        let settings = self.create_uninitialized(name);
        let mut ownership =
            tl3_store::Ownership::claim(tl3_store::Store::connect(&settings).unwrap()).unwrap();
        ownership.migrate().unwrap();
        ownership.release().unwrap();
        settings
    }

    #[allow(
        dead_code,
        reason = "Schema and migration regressions need an empty database"
    )]
    pub fn create_uninitialized(&mut self, name: &str) -> tl3_config::Store {
        self.runtime
            .block_on(self.postgres.create_database(name))
            .unwrap();
        tl3_config::Store {
            host: "127.0.0.1".into(),
            port: self.postgres.settings().port,
            database: name.into(),
            user: "postgres".into(),
            password_file: Some(self.password_file.clone()),
            connect_timeout_secs: 10,
        }
    }

    #[allow(
        dead_code,
        reason = "Used by status tests; this fixture is shared with worker tests"
    )]
    pub fn restart(&mut self) {
        self.stop();
        self.resume();
    }

    #[allow(
        dead_code,
        reason = "Used by status tests; this fixture is shared with worker tests"
    )]
    pub fn resume(&mut self) {
        self.runtime
            .block_on(self.postgres.start())
            .expect("start embedded Postgres again");
    }

    pub fn stop(&self) {
        self.runtime.block_on(self.postgres.stop()).unwrap();
        assert!(self.scratch.path().exists());
    }
}

/// Run initdb as postgresql_embedded's setup would, but with `--no-sync`. The
/// cluster is disposable, and initdb's flush fsyncs every file it created: on a
/// contended disk that alone took tens of seconds (#362). The server still runs
/// with fsync on, as the store requires.
fn initialize(settings: &Settings) {
    if settings.data_dir.join("postgresql.conf").exists() {
        // cache::install's first download initialized this cluster already.
        return;
    }
    fs::write(&settings.password_file, &settings.password).unwrap();
    let output = Command::new(settings.binary_dir().join("initdb"))
        .arg("--pgdata")
        .arg(&settings.data_dir)
        .args(["--username", "postgres", "--auth", "password"])
        .arg("--pwfile")
        .arg(&settings.password_file)
        .args(["--encoding", "UTF8", "--no-sync"])
        .output()
        .expect("run initdb");
    assert!(
        output.status.success(),
        "initdb: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}
