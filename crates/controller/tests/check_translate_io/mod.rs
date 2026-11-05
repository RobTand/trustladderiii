use io::{Commit, Controller, Feedback, World, put};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use tl3_controller::{Error, Repository, Result, adoption, executor::Observation};
use tl3_kernel::{Event, HeadId, NodeId, Reply};
use tl3_store::{RowChange, Snapshot};
mod checks_current;
mod checks_reference;
mod io;
mod plan_current;
mod plan_reference;
mod prepared_current;
mod prepared_reference;
mod properties;
mod production {
    include!("production_helpers.rs");
}
mod goals {
    include!("goals_helpers.rs");
}
mod memory {
    include!("memory_helpers.rs");
}
mod evidence {
    include!("evidence_helpers.rs");
}
mod findings {
    pub use tl3_controller::findings::*;
    pub const MAX_OUTSIDE_PATHS: usize = 100;
    pub const MAX_PATH_CHARS: usize = 4096;
    include!("clipped_helpers.rs");
}
mod adapters {
    pub mod prismabuild {
        pub use tl3_controller::adapters::prismabuild::*;
        include!("selected_helpers.rs");
    }
}
include!("request_type.rs");
