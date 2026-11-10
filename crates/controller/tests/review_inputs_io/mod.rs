use criteria::criteria;
use io::{Store, Worktrees};
use serde_json::{Value, json};
use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};
use tl3_controller::{Error, Result, executor::Delivery};
use tl3_controller_core::review_inputs::{bounds, history, selection};
use tl3_kernel::{EffectKind, NodeId};
use tl3_store::Snapshot;
use tl3_worktrees::CommitId;
const SCOPE_RULE: &str = "request-scope";
const ISSUE_RULE: &str = "request-coverage";
const REPORT_RULE: &str = "test-report-integrity";
mod criteria;
#[path = "criteria-current.rs"]
mod criteria_current;
mod run_backend { pub mod durable { pub const MAX_BYTES: usize = 8 * 1024 * 1024; } }
mod current;
mod goals;
mod io;
mod properties;
mod reference;
mod tl3_store {
    use super::*;
    #[derive(Clone, Debug)]
    pub struct Row {
        pub table: String,
        pub id: String,
        pub value: Value,
    }
    #[derive(Clone, Debug)]
    pub struct Snapshot {
        pub rows: Vec<Row>,
    }
}
mod findings {
    include!("clipped.rs");
}

#[path = "failure.rs"]
mod worker_inputs;

mod protocol {
    use super::*;
    include!("validation.rs");
    pub fn validate_inputs(role: &str, inputs: &Value) -> Result<()> {
        io::read("validate_inputs", json!([role, inputs]))?;
        validate_real(role, inputs)
    }
}
