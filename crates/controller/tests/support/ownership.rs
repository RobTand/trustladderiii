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

/// Standalone adapter fixtures use isolated files and share one claimed session.
pub fn ownership() -> MutexGuard<'static, tl3_controller::Ownership> {
    static DATABASE: OnceLock<Mutex<(Database, Child)>> = OnceLock::new();
    static OWNER: OnceLock<Mutex<tl3_controller::Ownership>> = OnceLock::new();
    OWNER
        .get_or_init(|| {
            let mut database = DATABASE
                .get_or_init(|| Mutex::new(Database::start_shared_on_stderr()))
                .lock()
                .unwrap();
            let settings = database.0.create("adapter_fixture_owner");
            Mutex::new(
                tl3_controller::Ownership::claim(tl3_store::Store::open(&settings).unwrap())
                    .unwrap(),
            )
        })
        .lock()
        .unwrap()
}
