use super::scratch::owned_metadata;
use postgresql_embedded::{PostgreSQL, Settings};
use std::{
    fs::{self, OpenOptions},
    io::{self, Write},
    os::{
        linux::net::SocketAddrExt,
        unix::{
            fs::{DirBuilderExt, MetadataExt},
            net::{SocketAddr, UnixListener},
        },
    },
    path::PathBuf,
    thread,
    time::{Duration, Instant},
};

struct InstallLock {
    file: PathBuf,
    _socket: UnixListener,
}

impl InstallLock {
    fn acquire(cache: &std::path::Path) -> Self {
        let metadata = owned_metadata(cache).unwrap();
        // dl380g10 forbids flock/fcntl. The abstract socket releases on SIGKILL
        // and serializes stale lock-file recovery without an unlink race.
        let name = format!("tl3-test-postgres-{}-{}", metadata.dev(), metadata.ino());
        let address = SocketAddr::from_abstract_name(name.as_bytes()).unwrap();
        let started = Instant::now();
        let socket = loop {
            match UnixListener::bind_addr(&address) {
                Ok(socket) => break socket,
                Err(error) if error.kind() == io::ErrorKind::AddrInUse => {
                    assert!(
                        started.elapsed() < Duration::from_secs(90),
                        "test Postgres cache installer did not release its lock"
                    );
                    thread::sleep(Duration::from_millis(25));
                }
                Err(error) => panic!("lock test Postgres cache: {error}"),
            }
        };
        let file = cache.join("install.lock");
        match owned_metadata(&file) {
            Ok(metadata) => {
                assert!(metadata.is_file(), "invalid test Postgres install lock");
                fs::remove_file(&file).unwrap();
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => panic!("uncertain test Postgres cache lock: {error}"),
        }
        let mut handle = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&file)
            .unwrap();
        writeln!(handle, "{}", std::process::id()).unwrap();
        Self {
            file,
            _socket: socket,
        }
    }
}

impl Drop for InstallLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.file);
    }
}

pub fn directory() -> PathBuf {
    let home = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .unwrap_or_else(|| {
            std::env::home_dir()
                .expect("test Postgres cache needs a home")
                .join(".cache")
        });
    let cache = home.join("tl3-test-postgres");
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&cache)
        .unwrap();
    assert!(owned_metadata(&cache).unwrap().is_dir());
    cache
}

pub fn install(settings: &Settings, runtime: &tokio::runtime::Runtime) {
    let cache = settings.installation_dir.parent().unwrap();
    let _lock = InstallLock::acquire(cache);
    let staging = cache.join("installing");
    match owned_metadata(&staging) {
        Ok(metadata) => {
            assert!(metadata.is_dir());
            fs::remove_dir_all(&staging).unwrap();
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => panic!("uncertain test Postgres staging ownership: {error}"),
    }
    if settings.installation_dir.exists() {
        assert!(owned_metadata(&settings.installation_dir).unwrap().is_dir());
        assert!(settings.binary_dir().join("postgres").is_file());
        return;
    }
    fs::create_dir(&staging).unwrap();
    let mut installer = PostgreSQL::new(Settings {
        installation_dir: staging.join("binaries"),
        // setup also initializes this fixture's cluster; retain it after publication.
        temporary: false,
        ..settings.clone()
    });
    runtime
        .block_on(installer.setup())
        .expect("install embedded Postgres into staging");
    fs::rename(
        &installer.settings().installation_dir,
        &settings.installation_dir,
    )
    .unwrap();
    fs::remove_dir_all(&staging).unwrap();
}
