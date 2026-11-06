#[cfg(loom)]
#[allow(dead_code)]
#[path = "../../../store/tests/support/mod.rs"]
mod database;
#[cfg(not(loom))]
use crate::support::Database;
#[cfg(loom)]
use database::Database;
use std::{
    process::Child,
    sync::{Mutex, MutexGuard, OnceLock},
};

static DATABASE: OnceLock<Mutex<(Database, Child)>> = OnceLock::new();

fn database() -> MutexGuard<'static, (Database, Child)> {
    DATABASE
        .get_or_init(|| Mutex::new(Database::start_shared_on_stderr()))
        .lock()
        .unwrap()
}

/// Adapter and controller fixtures share the server, with isolated databases.
#[allow(dead_code, reason = "Used by the PrismaBuild scenario harness")]
pub fn with_database<T>(scenario: impl FnOnce(&mut Database) -> T) -> T {
    // Initialize the owner before holding the database lock: adapters opened by
    // the scenario use that owner without recursively initializing the server.
    drop(ownership());
    scenario(&mut database().0)
}

/// Standalone adapter fixtures use isolated files and share one claimed session.
pub fn ownership() -> MutexGuard<'static, tl3_controller::Ownership> {
    static OWNER: OnceLock<Mutex<tl3_controller::Ownership>> = OnceLock::new();
    OWNER
        .get_or_init(|| {
            let mut database = database();
            let settings = database.0.create("adapter_fixture_owner");
            Mutex::new(
                tl3_controller::Ownership::claim(tl3_store::Store::open(&settings).unwrap())
                    .unwrap(),
            )
        })
        .lock()
        .unwrap()
}
