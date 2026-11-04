//! Per-delivery enqueue decisions. The adapter owns file formats and I/O.
use super::{Head, QueueState, Status, Time};

pub const GRACE_MS: u64 = 300_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(kani, derive(kani::Arbitrary))]
pub enum Requeue {
    Ready,
    Sending,
    Refused,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(kani, derive(kani::Arbitrary))]
pub enum Command {
    Ready,
    Sending,
    Sent,
    Refused,
    PermissionDenied,
    Rejected,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(kani, derive(kani::Arbitrary))]
pub struct Recovery {
    pub eligible_since_ms: Option<u64>,
    pub command: Command,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(kani, derive(kani::Arbitrary))]
pub struct Intent {
    pub requeue: Option<Requeue>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(kani, derive(kani::Arbitrary))]
pub struct Rerun {
    pub attempt: u64,
    pub accepted: bool,
    pub waiting: bool,
}

/// A projection of the existing side files, scoped to one delivery and head.
/// `rerun` is the receipt for the workflow run currently being reconciled.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[cfg_attr(kani, derive(kani::Arbitrary))]
pub struct EnqueueState {
    pub intent: Option<Intent>,
    pub confirmed: bool,
    pub recovery: Option<Recovery>,
    pub rerun: Option<Rerun>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(kani, derive(kani::Arbitrary))]
pub struct IssueRead {
    pub queue_label: bool,
    pub repaired: bool,
    pub recovery: bool,
    pub dequeue_pending: bool,
    pub applied: bool,
}

/// Optional fields are reads not yet requested. This preserves the adapter's
/// short circuits, including refusal before reruns and stale-head handling.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(kani, derive(kani::Arbitrary))]
pub struct QueueRead {
    pub head: Head,
    pub queue: QueueState,
    pub merged: bool,
    pub conflicts: bool,
    pub held: bool,
    pub check_changed: bool,
    pub refusal: Option<bool>,
    pub reruns_pending: Option<bool>,
    pub person_removed: Option<bool>,
    pub issue: Option<IssueRead>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(kani, derive(kani::Arbitrary))]
pub enum Need {
    Queue,
    Refusal,
    Reruns,
    Person,
    Issue,
    Command,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(kani, derive(kani::Arbitrary))]
pub enum Failure {
    Refused,
    Permission,
    Rejected,
    DequeuePending,
    ChecksPending,
    RerunUnknown,
    RerunFinishing,
    PullChanged,
}
impl Failure {
    pub fn class(self) -> &'static str {
        match self {
            Self::Refused => "queue_command_refused",
            Self::Permission => "queue_command_permission",
            Self::Rejected => "queue_command_rejected",
            Self::DequeuePending => "Mergify dequeue is still finishing",
            Self::ChecksPending => "required checks pending",
            Self::RerunUnknown => "workflow rerun acceptance is still unknown",
            Self::RerunFinishing => "workflow run is still finishing before rerun",
            Self::PullChanged => "PR changed before workflow rerun",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(kani, derive(kani::Arbitrary))]
pub enum Complete {
    Confirmed { progress: bool, conflict: bool },
    Retryable,
    Permanent(Failure),
    Pending,
    Absent,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(kani, derive(kani::Arbitrary))]
pub enum Write {
    Label,
    Command,
    Rerun,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(kani, derive(kani::Arbitrary))]
pub enum CommandResult {
    Accepted,
    PermissionDenied,
    Rejected,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(kani, derive(kani::Arbitrary))]
pub enum RerunResult {
    Accepted,
    InProgress,
    Refused,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(kani, derive(kani::Arbitrary))]
pub struct CommandRead {
    pub head: Head,
    pub open: bool,
    pub merged: bool,
    pub draft: bool,
    pub held: bool,
    pub paused: bool,
    pub waiting: bool,
    pub eligible: bool,
    pub now_ms: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(kani, derive(kani::Arbitrary))]
pub enum QueueEvent {
    Begin,
    Reconcile(QueueRead),
    Prepare {
        head: Head,
        dequeue_pending: bool,
        checks_ready: bool,
        queue_label: bool,
    },
    RecoveryTick {
        eligible: bool,
        now_ms: u64,
    },
    CommandReceipt {
        refused: bool,
        found: bool,
    },
    DispatchCommand(CommandRead),
    CommandDone(CommandResult),
    RerunRead {
        head: Head,
        attempt: u64,
        status: Status,
        writable: bool,
    },
    RerunDone(RerunResult),
    RerunPending {
        head: Head,
        attempt: u64,
        status: Status,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Effect {
    Read(Need),
    SaveIntent,
    SaveRecovery,
    SaveRerun,
    Write(Write),
    Complete(Complete),
    Error(Failure),
    Continue,
    RerunPending(bool),
}

#[cfg(not(kani))]
pub type Effects = Vec<Effect>;

/// At most two saves and one completion, or one save and one upstream write.
#[cfg(kani)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Effects {
    items: [Option<Effect>; 3],
    len: usize,
}
#[cfg(kani)]
impl Effects {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn push(&mut self, effect: Effect) {
        assert!(self.len < 3);
        self.items[self.len] = Some(effect);
        self.len += 1;
    }
    pub fn len(&self) -> usize {
        self.len
    }
    pub fn get(&self, i: usize) -> Option<Effect> {
        self.items[i]
    }
}

pub fn decide(_state: EnqueueState, _event: QueueEvent) -> (EnqueueState, Effects) {
    panic!("enqueue decision core not yet implemented")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(kani, derive(kani::Arbitrary))]
pub struct LabelEvent {
    pub id: u64,
    pub labeled: bool,
    pub at: Time,
}

pub fn dequeue_pending(
    retry: bool,
    rule_concluded: bool,
    dequeued_label: bool,
    dequeued: Option<LabelEvent>,
    queued: Option<u64>,
    finished: Option<Time>,
) -> bool {
    if !retry || rule_concluded {
        return false;
    }
    if !dequeued_label {
        return true;
    }
    !dequeued.is_some_and(|event| {
        event.labeled
            && queued.map_or_else(
                || finished.is_some_and(|finished| event.at >= finished),
                |queued| event.id > queued,
            )
    })
}
