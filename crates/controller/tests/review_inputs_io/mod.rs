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
const REPORT_RULE: &str = "test-report-integrity";
mod criteria;
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
    pub fn clipped(text: &str, cap: usize) -> String {
        text.chars().take(cap).collect()
    }
}
mod protocol {
    use super::*;
    // Unchanged validator is a recorded leaf. Retained worker targets run its real schema.
    pub fn validate_inputs(role: &str, inputs: &Value) -> Result<()> {
        io::read("validate_inputs", json!([role, inputs])).map(|_| ())
    }
}
