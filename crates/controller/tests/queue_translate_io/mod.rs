use adapters::mergify::{Evidence, QueueFact};
use io::{Commit, Controller, World, check_run, put};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use tl3_controller::Repository;
use tl3_controller::{Error, Result};
use tl3_kernel::{Event, Fact, HeadId, NodeId};
use tl3_store::{InternKind, Snapshot};
mod current;
mod io;
mod properties;
mod reference;
mod reference_core;

pub mod findings {
    pub use tl3_controller::findings::*;
    pub const MAX_REPORT_CHARS: usize = 16000;
    pub fn clipped(text: &str, cap: usize) -> String {
        text.chars().take(cap).collect()
    }
}
pub mod memory {
    // The extraction does not change this helper. Generated identities contain no secrets.
    pub fn redact_text(text: &str) -> String {
        text.to_owned()
    }
}
pub mod adapters {
    pub mod prismabuild {
        pub use tl3_controller::adapters::prismabuild::*;
    }
    pub mod mergify {
        use super::super::*;
        #[derive(Clone, Debug, Serialize, Deserialize)]
        pub struct QueueFact {
            pub repository: Repository,
            pub number: u64,
            pub evidence: Evidence,
        }
        include!("queue-fact.rs");
        pub mod ci_evidence {
            use super::*;
            include!("ci-evidence.rs");
        }
        pub use tl3_controller::adapters::mergify::RequiredCheck;
    }
}
#[derive(Clone, Debug)]
enum Observation {
    Queue(QueueFact),
}
use tl3_controller::adoption;
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Request {
    repo: Repository,
    issue: u64,
    #[serde(default)]
    title: String,
    issue_id: i64,
    #[serde(default)]
    source: Option<String>,
    #[serde(default)]
    proposed: Option<adoption::ProposedPullRequest>,
    /// P0–P3 from the issue, read once at intake (#416); `None` for unranked
    /// and older rows, and omitted then so the row keeps its older shape (#375).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    priority: Option<u8>,
}

fn request(snapshot: &Snapshot, node: NodeId) -> Result<Request> {
    decode_stored(
        snapshot
            .rows
            .iter()
            .find(|r| r.table == "requests" && r.id == node.0.to_string())
            .ok_or(Error::Invalid("missing request binding"))?
            .value
            .clone(),
    )
}
fn decode_stored<T: serde::de::DeserializeOwned>(value: Value) -> Result<T> {
    serde_json::from_value(value).map_err(Error::StoredJson)
}
fn commit_sha(snapshot: &Snapshot, head: Option<HeadId>) -> Option<&str> {
    let head = head?;
    snapshot
        .interned
        .iter()
        .find(|item| item.kind == InternKind::Commit && item.id == head.0 as i64)
        .map(|item| item.value.as_str())
}
pub mod looks {
    use super::*;
    pub use tl3_controller::looks::Cause;
    #[derive(Clone, Debug, Serialize, Deserialize)]
    pub(crate) struct Incident {
        pub id: String,
        pub node: Option<NodeId>,
        #[serde(default)]
        pub input_failure: Option<Value>,
        pub open: bool,
        pub needs_rob: bool,
        pub blocking: bool,
        pub summary: String,
        pub signature: Value,
        #[serde(default)]
        pub effect: Option<String>,
        #[serde(default)]
        pub post_key: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub cause: Option<Cause>,
    }
    include!("incident-helpers.rs");
}
