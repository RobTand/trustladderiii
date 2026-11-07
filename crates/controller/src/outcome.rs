//! PR outcomes shared by delivery adapters and simulation.
use crate::findings::{FailedTest, CiEvidence};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Outcome {
    Merged {
        head: String,
        checks_passed: bool,
    },
    CiFailed {
        head: String,
        infra: bool,
        failed_tests: Vec<FailedTest>,
        evidence: CiEvidence,
    },
    Conflict {
        head: String,
    },
    PushByOther {
        head: String,
    },
    ClosedByPerson,
}

/// Preserve receipt identities when delivery changes adapters across a restart.
pub(crate) fn identity(repository: &crate::Repository, number: u64, suffix: &str) -> String {
    format!(
        "{}/{}/{}/{}/{suffix}",
        repository.owner.to_ascii_lowercase(),
        repository.name.to_ascii_lowercase(),
        repository.installation,
        number
    )
}

pub(crate) fn recorded(snapshot: &tl3_store::Snapshot, identity: &str) -> bool {
    snapshot.rows.iter().any(|row| {
        row.table == "delivery_outcomes" && row.id == identity
    })
}
