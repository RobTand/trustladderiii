//! The controller library's tests share one embedded Postgres per test process
//! (#731), as the scenario harnesses do. Each test still gets its own databases,
//! and its handle ends their sessions and drops them, as stopping a private
//! server used to.

#[allow(dead_code)]
#[path = "../../store/tests/support/mod.rs"]
mod fixture;

use std::{
    process::Child,
    sync::{
        Mutex, MutexGuard, OnceLock, PoisonError,
        atomic::{AtomicUsize, Ordering},
    },
};

struct Server {
    fixture: fixture::Database,
    _watchdog: Child,
}

fn server() -> MutexGuard<'static, Server> {
    static SERVER: OnceLock<Mutex<Server>> = OnceLock::new();
    SERVER
        .get_or_init(|| {
            let (fixture, watchdog) = fixture::Database::start_shared();
            Mutex::new(Server {
                fixture,
                _watchdog: watchdog,
            })
        })
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
}

pub struct Database {
    settings: Option<tl3_config::Store>,
    names: Vec<String>,
}

impl Database {
    pub fn start() -> Self {
        drop(server());
        Self {
            settings: None,
            names: Vec::new(),
        }
    }

    /// A new, empty database on the shared server. Names are unique per process,
    /// so tests that reuse a name stay isolated.
    pub fn create(&mut self, name: &str) -> tl3_config::Store {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let unique = format!("{name}_{}", NEXT.fetch_add(1, Ordering::Relaxed));
        let settings = server().fixture.create(&unique);
        self.names.push(unique);
        self.settings = Some(settings.clone());
        settings
    }

    /// End this test's sessions and drop its databases.
    pub fn stop(&mut self) {
        self.drop_databases()
            .expect("drop this test's databases on the shared server");
    }

    fn drop_databases(&mut self) -> Result<(), postgres::Error> {
        let Some(settings) = &self.settings else {
            return Ok(());
        };
        if self.names.is_empty() {
            return Ok(());
        }
        let password = std::fs::read_to_string(settings.password_file.as_ref().unwrap()).unwrap();
        let mut admin = postgres::Config::new()
            .host(&settings.host)
            .port(settings.port)
            .user(&settings.user)
            .password(password.trim_end())
            .dbname("postgres")
            .connect(postgres::NoTls)?;
        while let Some(name) = self.names.pop() {
            admin.batch_execute(&format!("DROP DATABASE IF EXISTS \"{name}\" WITH (FORCE)"))?;
        }
        Ok(())
    }
}

impl Drop for Database {
    fn drop(&mut self) {
        let dropped = self.drop_databases();
        if !std::thread::panicking() {
            dropped.expect("drop this test's databases on the shared server");
        }
    }
}

/// Standalone adapter fixtures use isolated files and share one claimed session.
pub fn ownership() -> MutexGuard<'static, crate::Ownership> {
    static OWNER: OnceLock<Mutex<crate::Ownership>> = OnceLock::new();
    OWNER
        .get_or_init(|| {
            let settings = server().fixture.create("adapter_fixture_owner");
            Mutex::new(crate::Ownership::claim(tl3_store::Store::open(&settings).unwrap()).unwrap())
        })
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
}
