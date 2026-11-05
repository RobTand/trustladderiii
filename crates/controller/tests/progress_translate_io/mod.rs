use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tl3_controller::{
    Error,
    executor::{Delivery, Observation, Upstream, upstream},
};
use tl3_controller_core::looks_state as state_decisions;
use tl3_kernel::Event;
use tl3_store::{Commit, RowChange, Snapshot, WorldRow};
type Result<T> = std::result::Result<T, Error>;

fn decode_stored<T: serde::de::DeserializeOwned>(value: Value) -> Result<T> {
    serde_json::from_value(value).map_err(Error::StoredJson)
}
fn put(table: &str, id: String, value: Value) -> RowChange {
    RowChange::Put {
        table: table.into(),
        id,
        value,
    }
}
include!("types.rs");
macro_rules! controller {
    () => {
        struct Controller {
            allowances: tl3_config::Allowances,
        }
        impl Controller {
            fn new() -> Self {
                let mut allowances = tl3_config::Allowances::default();
                for a in [
                    &mut allowances.agent_worker,
                    &mut allowances.github_mergify,
                    &mut allowances.prismabuild,
                    &mut allowances.worktrees,
                ] {
                    a.silence_secs = 1;
                    a.no_progress_secs = 3;
                }
                Self { allowances }
            }
            fn now(&self) -> SystemTime {
                io::now()
            }
        }
    };
}
mod current;
mod io;
mod properties;
mod reference;
mod source;
