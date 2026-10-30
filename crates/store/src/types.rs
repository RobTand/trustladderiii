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
        pub struct $name(pub(crate) i32);

        impl $name {
            pub fn get(self) -> i32 {
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
    pub id: i32,
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
            result: Value::Null,
            next_epoch: None,
        }
    }
}
