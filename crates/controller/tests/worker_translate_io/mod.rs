use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use tl3_controller::{Error, Result, Repository};
use tl3_kernel::{Event, Reply, HeadId, NodeId};
use tl3_store::Snapshot;
#[derive(Serialize)]
enum RowChange { Put {table:String,id:String,value:Value}, Delete {table:String,id:String} }
use io::{Controller, World, Commit, put, check_run};
mod io;
mod reference;
mod current;
mod properties;
mod goals;
mod findings;
mod helpers;
mod quota;
mod evidence;
mod memory;
mod reviewer {
    use super::*;
    #[path = "../decision.rs"]
    mod decision;
    include!("reviewer.rs");
}
pub mod contracts { pub use tl3_controller::contracts::*; }
pub mod budget { pub use tl3_controller::budget::*; }
mod adapters {
    pub mod prismabuild {
        pub use tl3_controller::adapters::prismabuild::*;
        use super::super::*;
        const MAX_REPORT_LINE_BYTES:usize = 4 * 1024 * 1024;
        
        include!("plan-size.rs");
    }
    pub mod results { pub const MAX_RESULT_BYTES:usize = 8 * 1024 * 1024; }
    pub mod workers {
        pub mod protocol { include!("protocol.rs"); }
        pub mod review { include!("review.rs"); }
        pub mod reproducing { include!("reproducing.rs"); }
    }
}
#[derive(Clone, Debug)]
enum Observation {
    WorkerFailed {node:NodeId, generation:u64, failure_class:String, failure_message:String},
    WorkerReply {node:NodeId, generation:u64, role:String, reply:Value, model:Option<String>},
}
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Request {
    repo: Repository,
    issue:u64,
    #[serde(default)] title:String,
    issue_id:i64,
    #[serde(default)] source:Option<String>,
    #[serde(default)] proposed:Option<tl3_controller::adoption::ProposedPullRequest>,
    #[serde(default, skip_serializing_if="Option::is_none")] priority:Option<u8>,
}
use helpers::{request, decode_stored, commit_sha};
