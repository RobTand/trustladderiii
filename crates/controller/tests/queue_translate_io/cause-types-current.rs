pub use tl3_controller::queue_cause::{CauseEvidence, QueueContext, GateEvidence, CheckIdentity};
use super::*;
use chrono::{DateTime, Utc};
/// Captured in the shared command receipt before POST and published on Sent.
/// Failure identities already observed at that fence cannot be retest results.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct RetryTransition {
    pub command_key: String,
    pub before: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Hold {
    pub repository: Repository,
    pub number: u64,
    pub node: NodeId,
    pub head: String,
    pub identity: String,
    pub context: QueueContext,
    pub failed_at: Option<DateTime<Utc>>,
    pub since_ms: u64,
    pub witness: Option<GateEvidence>,
    pub witnessed_ms: Option<u64>,
    #[serde(default)]
    pub retry: Option<RetryTransition>,
    #[serde(default)]
    pub reporters: Vec<CheckIdentity>,
}

pub(crate) fn key(node: NodeId, head: &str) -> String {
    format!("{}:{head}", node.0)
}

pub(crate) fn held(snapshot: &Snapshot, node: NodeId, head: &str) -> Result<Option<Hold>> {
    snapshot
        .rows
        .iter()
        .find(|row| row.table == "queue_holds" && row.id == key(node, head))
        .map(|row| decode_stored(row.value.clone()))
        .transpose()
}

