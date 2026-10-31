use postgresql_embedded::{PostgreSQL, Settings, VersionReq};
use std::{
    collections::HashMap,
    fs,
    path::PathBuf,
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
                ("max_connections".into(), "20".into()),
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
        let mut postgres = PostgreSQL::new(settings);
        runtime
            .block_on(postgres.setup())
            .expect("download and initialize embedded Postgres");
        println!(
            "embedded Postgres cached setup (fsync={fsync}): {:.2}s",
            started.elapsed().as_secs_f64()
        );
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
