use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{Error, Result};

macro_rules! counter {
    ($name:ident) => {
        #[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Ord, PartialOrd, Serialize)]
        pub struct $name(i64);

        impl $name {
            pub const ZERO: Self = Self(0);

            pub fn get(self) -> i64 {
                self.0
            }

            pub(crate) fn next(self) -> Result<Self> {
                self.0
                    .checked_add(1)
                    .map(Self)
                    .ok_or(Error::CounterOverflow)
            }
        }

        impl TryFrom<i64> for $name {
            type Error = Error;

            fn try_from(value: i64) -> Result<Self> {
                if value < 0 {
                    return Err(Error::Invalid("counter must be nonnegative"));
                }
                Ok(Self(value))
            }
        }
    };
}

counter!(Revision);
counter!(Epoch);

macro_rules! intern_id {
    ($name:ident) => {
        #[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
        pub struct $name(pub(crate) i64);

        impl $name {
            pub fn get(self) -> i64 {
                self.0
            }
        }
    };
}

intern_id!(RepoId);
intern_id!(CommitId);
intern_id!(PathId);
intern_id!(IssueId);
intern_id!(PullRequestId);

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum EffectState {
    Intended,
    Sent,
    Uncertain,
    Confirmed,
    Failed,
    Cancelling,
    Cancelled,
}

impl EffectState {
    pub fn allows(self, next: Self) -> bool {
        use EffectState::*;
        matches!(
            (self, next),
            (Intended, Sent | Cancelled)
                | (Sent, Confirmed | Intended | Failed | Uncertain | Cancelling)
                | (Uncertain, Confirmed | Intended | Cancelling)
                | (Cancelling, Confirmed | Cancelled)
        )
    }

    pub fn is_pending(self) -> bool {
        matches!(
            self,
            Self::Intended | Self::Sent | Self::Uncertain | Self::Cancelling
        )
    }

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Intended => "Intended",
            Self::Sent => "Sent",
            Self::Uncertain => "Uncertain",
            Self::Confirmed => "Confirmed",
            Self::Failed => "Failed",
            Self::Cancelling => "Cancelling",
            Self::Cancelled => "Cancelled",
        }
    }
}

impl TryFrom<&str> for EffectState {
    type Error = Error;

    fn try_from(value: &str) -> Result<Self> {
        match value {
            "Intended" => Ok(Self::Intended),
            "Sent" => Ok(Self::Sent),
            "Uncertain" => Ok(Self::Uncertain),
            "Confirmed" => Ok(Self::Confirmed),
            "Failed" => Ok(Self::Failed),
            "Cancelling" => Ok(Self::Cancelling),
            "Cancelled" => Ok(Self::Cancelled),
            _ => Err(Error::Invalid("unknown effect state in database")),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct WorldRow {
    pub table: String,
    pub id: String,
    pub value: Value,
    /// The global commit revision that last changed this row.
    pub version: Revision,
    /// Monotonic insertion order, preserved by updates and across restarts.
    pub arrival: i64,
}

#[derive(Clone, Debug)]
pub enum RowChange {
    Put {
        table: String,
        id: String,
        value: Value,
    },
    Delete {
        table: String,
        id: String,
    },
}

#[derive(Clone, Debug)]
pub struct NewEffect {
    pub key: String,
    pub kind: String,
    pub payload: Value,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Effect {
    pub key: String,
    pub epoch: Epoch,
    pub kind: String,
    pub payload: Value,
    pub state: EffectState,
    pub created_revision: Revision,
    pub updated_revision: Revision,
}

#[derive(Clone, Debug)]
pub struct EffectTransition {
    pub key: String,
    pub from: EffectState,
    pub to: EffectState,
}

/// A postmortem made owed by a commit (#403), so a crash can't lose it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OwedPostmortem {
    pub node: String,
    /// What made it owed, unique per node: close, question:<id>, incident:<id>
    /// or rob:<console command id>.
    pub trigger: String,
    /// needed_fix, retry, escalation, closed_without_merge, sampled or rob_asked.
    pub reason: String,
    /// The sampled share in force, for reason sampled.
    pub sample_share: Option<f64>,
    pub repo: String,
    pub issue: i64,
    pub title: String,
}

/// One rubric dimension's grade from code. A later fact adds a grade.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CodeGrade {
    pub node: String,
    /// The fact that recorded it, such as close; unique per dimension and version.
    pub trigger: String,
    pub rubric_version: i32,
    pub dimension: String,
    /// 1 to 5, or none for a fact the rubric records without grading.
    pub grade: Option<i16>,
    pub fact: Value,
}

/// A postmortem's record, as the console reads it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PostmortemRecord {
    pub id: i64,
    pub node: String,
    pub trigger: String,
    pub reason: String,
    pub sample_share: Option<f64>,
    /// owed, running, done or failed.
    pub status: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CreditEntry {
    pub node: String,
    /// Include the operation/generation in a repeatable reason, such as attempt:2.
    pub reason: String,
    pub root: String,
    pub amount: i64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CreditRecord {
    pub entry: CreditEntry,
    pub revision: Revision,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InternKind {
    Repo,
    Commit,
    Path,
    Issue,
    PullRequest,
}

impl InternKind {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Repo => "Repo",
            Self::Commit => "Commit",
            Self::Path => "Path",
            Self::Issue => "Issue",
            Self::PullRequest => "PullRequest",
        }
    }
}

impl TryFrom<&str> for InternKind {
    type Error = Error;

    fn try_from(value: &str) -> Result<Self> {
        match value {
            "Repo" => Ok(Self::Repo),
            "Commit" => Ok(Self::Commit),
            "Path" => Ok(Self::Path),
            "Issue" => Ok(Self::Issue),
            "PullRequest" => Ok(Self::PullRequest),
            _ => Err(Error::Invalid("unknown intern kind in database")),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Interned {
    pub id: i64,
    pub kind: InternKind,
    /// Repo: owner; commit: node; path/issue/PR: decimal repo id.
    pub scope: String,
    pub value: String,
}

impl Interned {
    pub fn repo_id(&self) -> Option<RepoId> {
        (self.kind == InternKind::Repo).then_some(RepoId(self.id))
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Snapshot {
    pub revision: Revision,
    pub epoch: Epoch,
    pub rows: Vec<WorldRow>,
    pub pending_effects: Vec<Effect>,
    pub credit: Vec<CreditRecord>,
    pub interned: Vec<Interned>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CommitReceipt {
    pub revision: Revision,
    pub epoch: Epoch,
    /// Caller-supplied result, persisted for idempotent event redelivery.
    pub result: Value,
}

#[derive(Clone, Debug, PartialEq)]
pub struct EventRecord {
    pub key: String,
    pub payload: Value,
    pub receipt: CommitReceipt,
}

#[derive(Clone, Debug)]
pub struct Commit {
    pub event_key: String,
    pub event: Value,
    pub expected_revision: Revision,
    pub epoch: Epoch,
    pub rows: Vec<RowChange>,
    pub effects: Vec<NewEffect>,
    pub transitions: Vec<EffectTransition>,
    pub credit: Vec<CreditEntry>,
    /// Postmortems this commit makes owed (#403). A replay records each once.
    pub postmortems: Vec<OwedPostmortem>,
    /// Code grades this commit records. Grades are append-only.
    pub grades: Vec<CodeGrade>,
    pub result: Value,
    /// Upgrade only: advance by one, after all active deliveries have drained.
    pub next_epoch: Option<Epoch>,
}

impl Commit {
    pub fn new(event_key: impl Into<String>, event: Value, snapshot: &Snapshot) -> Self {
        Self {
            event_key: event_key.into(),
            event,
            expected_revision: snapshot.revision,
            epoch: snapshot.epoch,
            rows: Vec::new(),
            effects: Vec::new(),
            transitions: Vec::new(),
            credit: Vec::new(),
            postmortems: Vec::new(),
            grades: Vec::new(),
            result: Value::Null,
            next_epoch: None,
        }
    }
}

/// An outside job's owner and durable key, independent of the worker's role.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExternalWait {
    pub upstream: String,
    pub key: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct WorkerProgress {
    pub key: String,
    pub node: String,
    pub epoch: i64,
    pub attempt: i64,
    pub output_offset: i64,
    pub unit: String,
    pub external_wait: Option<ExternalWait>,
    /// Database observation time of this attempt claim, in Unix milliseconds.
    pub started_ms: i64,
    pub last_progress_ms: Option<i64>,
}

/// #422: one decision about a recalled memory, kept as an audit trail.
#[derive(Clone, Debug, PartialEq)]
pub struct MemoryCuration {
    pub memory_id: String,
    /// `hidden` from TL3's recall, `invalidated` in Hindsight, or `valid` again.
    pub state: &'static str,
    /// `accepted` decisions are sent to Hindsight; `rejected` ones are only recorded.
    pub verdict: &'static str,
    pub source: String,
    pub request: Option<String>,
    pub run: Option<String>,
    /// The memory's own words that the evidence contradicts.
    pub claim: Option<String>,
    pub reason: String,
    pub evidence: serde_json::Value,
}

/// One worker process attempt. Missing measurements stay unknown.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerRunFacts {
    pub started_ms: i64,
    pub finished_ms: Option<i64>,
    pub outcome: String,
    pub release: Option<String>,
    pub model: String,
    pub input_tokens: Option<i64>,
    pub cached_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub trace_id: Option<String>,
    pub failure: Option<WorkerBoundaryFailure>,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerBoundaryFailure {
    pub origin: String,
    pub class: String,
    pub status: Option<i32>,
    pub request_id: Option<String>,
}

impl WorkerRunFacts {
    pub fn validate(&self) -> crate::Result<()> {
        let bounded = |value: &str, max: usize| {
            !value.is_empty() && value.len() <= max && !value.chars().any(char::is_control)
        };
        let identifier = |value: &str, max: usize| {
            bounded(value, max)
                && value
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_.:-".contains(&b))
        };
        if self.started_ms < 0
            || self.finished_ms.is_some_and(|v| v < 0)
            || !matches!(
                self.outcome.as_str(),
                "succeeded" | "failed" | "quota" | "stopped" | "crashed"
            )
            || !bounded(&self.model, 256)
            || self.release.as_ref().is_some_and(|v| !identifier(v, 256))
            || self.trace_id.as_ref().is_some_and(|v| !identifier(v, 128))
            || [self.input_tokens, self.cached_tokens, self.output_tokens]
                .iter()
                .flatten()
                .any(|v| *v < 0)
            || self.failure.as_ref().is_some_and(|failure| {
                !matches!(
                    failure.origin.as_str(),
                    "provider" | "proxy" | "worker" | "unknown"
                ) || !identifier(&failure.class, 64)
                    || failure.status.is_some_and(|v| !(100..=599).contains(&v))
                    || failure
                        .request_id
                        .as_ref()
                        .is_some_and(|v| !identifier(v, 128))
            })
        {
            return Err(crate::Error::Invalid("invalid worker completion facts"));
        }
        Ok(())
    }
}
