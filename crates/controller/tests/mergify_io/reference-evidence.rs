use super::*;
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Evidence {
    ChecksUnavailable {
        permissions: Vec<String>,
    },
    /// Every check suite on the head completed without this required context (Q52).
    CheckMissing {
        head: String,
        required: RequiredCheck,
    },
    /// #447: Mergify waits on queue conditions that nothing still running can meet.
    QueueBlocked {
        head: String,
        conditions: Vec<String>,
        failed: Vec<String>,
    },
    /// The context of an open missing-check incident has reported on that head (Q52).
    CheckReported {
        head: String,
        required: RequiredCheck,
    },
    Merged {
        head: String,
        checks_passed: bool,
    },
    CiFailed {
        check: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        completed_at: Option<String>,
        head: String,
        infra: bool,
        #[serde(default)]
        head_check: bool,
        #[serde(default)]
        details: crate::findings::QueueCiEvidence,
    },
    Conflict {
        check: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        completed_at: Option<String>,
        head: String,
    },
    DequeuedByCommand {
        check: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        completed_at: Option<String>,
        head: String,
    },
    DequeuedByPerson {
        event: u64,
    },
    ClosedByPerson {
        event: u64,
    },
    PushByOther {
        head: String,
    },
}
