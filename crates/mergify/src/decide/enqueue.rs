//! Per-delivery enqueue decisions. The adapter owns file formats and I/O.
use super::{Head, QueueState, Status, Time};

pub const GRACE_MS: u64 = 300_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(kani, derive(kani::Arbitrary))]
#[derive(serde::Serialize, serde::Deserialize)]
pub enum Requeue {
    Ready,
    Sending,
    Refused,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(kani, derive(kani::Arbitrary))]
#[derive(serde::Serialize, serde::Deserialize)]
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
    pub repaired: Option<bool>,
    pub recovery: Option<bool>,
    pub dequeue_pending: Option<bool>,
    pub applied: Option<bool>,
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
    CommandReply,
    HeadChecks,
    RerunPull,
    RecoveryFile,
    RepairedHead,
    Dequeue,
    Applied,
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
    ExecuteGate {
        dequeue_pending: bool,
        checks_ready: Option<bool>,
    },
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
    CommandStatus,
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
        writable: Option<bool>,
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

fn command_completion(command: Command) -> Complete {
    match command {
        Command::Refused => Complete::Permanent(Failure::Refused),
        Command::PermissionDenied => Complete::Permanent(Failure::Permission),
        Command::Rejected => Complete::Permanent(Failure::Rejected),
        Command::Ready | Command::Sending | Command::Sent => Complete::Pending,
    }
}

fn reconcile_issue(intent: Option<Intent>, queue: QueueState, issue: IssueRead) -> Effect {
    if queue == QueueState::Waiting && issue.queue_label {
        let Some(recovery) = issue.recovery else {
            return Effect::Read(Need::RecoveryFile);
        };
        if recovery {
            return Effect::Complete(Complete::Pending);
        }
        match issue.repaired {
            None => return Effect::Read(Need::RepairedHead),
            Some(true) => return Effect::Complete(Complete::Pending),
            Some(false) => {}
        }
    }
    let pending = match intent {
        None => issue.dequeue_pending,
        Some(Intent {
            requeue: Some(requeue),
        }) => {
            let Some(recovery) = issue.recovery else {
                return Effect::Read(Need::RecoveryFile);
            };
            if recovery || requeue == Requeue::Sending {
                Some(true)
            } else {
                issue.dequeue_pending
            }
        }
        Some(Intent { requeue: None }) => {
            if issue.queue_label {
                Some(true)
            } else {
                let Some(applied) = issue.applied else {
                    return Effect::Read(Need::Applied);
                };
                Some(applied)
            }
        }
    };
    match pending {
        None => Effect::Read(Need::Dequeue),
        Some(true) => Effect::Complete(Complete::Pending),
        Some(false) => Effect::Complete(Complete::Absent),
    }
}

pub fn decide(mut state: EnqueueState, event: QueueEvent) -> (EnqueueState, Effects) {
    let mut out = Effects::new();
    let refused_intent = matches!(
        state.intent,
        Some(Intent {
            requeue: Some(Requeue::Refused)
        })
    );
    if refused_intent {
        state.recovery = Some(Recovery {
            eligible_since_ms: None,
            command: Command::Refused,
        });
        out.push(Effect::SaveRecovery);
        out.push(Effect::Complete(Complete::Permanent(Failure::Refused)));
        return (state, out);
    }
    match event {
        QueueEvent::ExecuteGate {
            dequeue_pending,
            checks_ready,
        } => out.push(if dequeue_pending {
            Effect::Error(Failure::DequeuePending)
        } else {
            match checks_ready {
                None => Effect::Read(Need::HeadChecks),
                Some(false) => Effect::Error(Failure::ChecksPending),
                Some(true) => Effect::Continue,
            }
        }),
        QueueEvent::Begin => out.push(Effect::Read(Need::Queue)),
        QueueEvent::Reconcile(read) => {
            let completion = if read.queue != QueueState::Merged && read.held {
                Complete::Pending
            } else {
                if !state.confirmed
                    && read.queue != QueueState::Merged
                    && matches!(
                        state.intent,
                        Some(Intent {
                            requeue: Some(Requeue::Sending)
                        })
                    )
                {
                    match read.refusal {
                        None => {
                            out.push(Effect::Read(Need::Refusal));
                            return (state, out);
                        }
                        Some(true) => {
                            state.intent = Some(Intent {
                                requeue: Some(Requeue::Refused),
                            });
                            state.recovery = Some(Recovery {
                                eligible_since_ms: None,
                                command: Command::Refused,
                            });
                            out.push(Effect::SaveRecovery);
                            out.push(Effect::SaveIntent);
                            out.push(Effect::Complete(Complete::Permanent(Failure::Refused)));
                            return (state, out);
                        }
                        Some(false) => {}
                    }
                }
                if !state.confirmed {
                    match read.reruns_pending {
                        None => {
                            out.push(Effect::Read(Need::Reruns));
                            return (state, out);
                        }
                        Some(true) => {
                            out.push(Effect::Complete(Complete::Pending));
                            return (state, out);
                        }
                        Some(false) => {}
                    }
                }
                if read.head != Head::Current
                    && read.queue != QueueState::Merged
                    && !state.confirmed
                {
                    Complete::Retryable
                } else if read.head == Head::Current && !read.merged && read.conflicts {
                    Complete::Confirmed {
                        progress: false,
                        conflict: true,
                    }
                } else {
                    let Some(person_removed) = read.person_removed else {
                        out.push(Effect::Read(Need::Person));
                        return (state, out);
                    };
                    let active =
                        matches!(read.queue, QueueState::Queued | QueueState::ChecksRunning);
                    let terminal = matches!(
                        read.queue,
                        QueueState::Dequeued
                            | QueueState::FailedCi
                            | QueueState::Conflict
                            | QueueState::DequeuedByPerson
                    );
                    if state.confirmed
                        || person_removed
                        || read.queue == QueueState::Merged
                        || read.head == Head::Current
                            && (active
                                || terminal
                                    && state.intent.is_some_and(|before| {
                                        before.requeue.is_none() && read.check_changed
                                    }))
                    {
                        Complete::Confirmed {
                            progress: true,
                            conflict: false,
                        }
                    } else {
                        let Some(issue) = read.issue else {
                            out.push(Effect::Read(Need::Issue));
                            return (state, out);
                        };
                        out.push(reconcile_issue(state.intent, read.queue, issue));
                        return (state, out);
                    }
                }
            };
            if matches!(completion, Complete::Confirmed { .. }) {
                state.confirmed = true;
            }
            out.push(Effect::Complete(completion));
        }
        QueueEvent::Prepare {
            head,
            dequeue_pending,
            checks_ready,
            queue_label,
        } => {
            if head != Head::Current {
                out.push(Effect::Complete(Complete::Retryable));
            } else if dequeue_pending {
                out.push(Effect::Error(Failure::DequeuePending));
            } else if !checks_ready {
                out.push(Effect::Error(Failure::ChecksPending));
            } else {
                let intent = *state.intent.get_or_insert(Intent {
                    requeue: queue_label.then_some(Requeue::Ready),
                });
                out.push(Effect::SaveIntent);
                out.push(if intent.requeue.is_some() {
                    Effect::Read(Need::Command)
                } else {
                    Effect::Write(Write::Label)
                });
            }
        }
        QueueEvent::RecoveryTick { eligible, now_ms } => {
            let recovery = state.recovery.get_or_insert(Recovery {
                eligible_since_ms: None,
                command: Command::Ready,
            });
            if recovery.command != Command::Ready {
                out.push(Effect::Complete(command_completion(recovery.command)));
            } else {
                if !eligible {
                    recovery.eligible_since_ms = None;
                }
                let matured = if eligible {
                    let since = *recovery.eligible_since_ms.get_or_insert(now_ms);
                    now_ms.saturating_sub(since) >= GRACE_MS
                } else {
                    false
                };
                out.push(Effect::SaveRecovery);
                out.push(if matured {
                    Effect::Read(Need::Command)
                } else {
                    Effect::Complete(Complete::Pending)
                });
            }
        }
        QueueEvent::CommandStatus => {
            let command = state.recovery.map_or(Command::Ready, |r| r.command);
            out.push(match command {
                Command::Ready => Effect::Continue,
                Command::Sending | Command::Sent => Effect::Read(Need::CommandReply),
                _ => Effect::Complete(command_completion(command)),
            });
        }
        QueueEvent::CommandReceipt { refused, found } => {
            let recovery = state.recovery.get_or_insert(Recovery {
                eligible_since_ms: None,
                command: Command::Sending,
            });
            let in_flight = matches!(recovery.command, Command::Sending | Command::Sent);
            if in_flight && refused {
                recovery.command = Command::Refused;
            } else if in_flight && found {
                recovery.command = Command::Sent;
            }
            if in_flight && (refused || found) {
                out.push(Effect::SaveRecovery);
            }
            out.push(Effect::Complete(command_completion(recovery.command)));
        }
        QueueEvent::DispatchCommand(read) => {
            let recovery = state.recovery.get_or_insert(Recovery {
                eligible_since_ms: None,
                command: Command::Ready,
            });
            if recovery.command != Command::Ready {
                out.push(Effect::Complete(command_completion(recovery.command)));
            } else if read.waiting && !read.eligible {
                recovery.eligible_since_ms = None;
                out.push(Effect::SaveRecovery);
                out.push(Effect::Complete(Complete::Pending));
            } else if read.head != Head::Current
                || !read.open
                || read.merged
                || read.draft
                || read.held
                || read.paused
                || read.waiting
                    && !recovery
                        .eligible_since_ms
                        .is_some_and(|since| read.now_ms.saturating_sub(since) >= GRACE_MS)
            {
                out.push(Effect::Complete(Complete::Pending));
            } else {
                recovery.command = Command::Sending;
                out.push(Effect::SaveRecovery);
                out.push(Effect::Write(Write::Command));
            }
        }
        QueueEvent::CommandDone(result) => {
            if let Some(recovery) = &mut state.recovery {
                if recovery.command != Command::Sending {
                    out.push(Effect::Complete(command_completion(recovery.command)));
                    return (state, out);
                }
                recovery.command = match result {
                    CommandResult::Accepted => Command::Sent,
                    CommandResult::PermissionDenied => Command::PermissionDenied,
                    CommandResult::Rejected => Command::Rejected,
                    CommandResult::Unknown => recovery.command,
                };
                out.push(Effect::SaveRecovery);
                out.push(Effect::Complete(command_completion(recovery.command)));
            }
        }
        QueueEvent::RerunRead {
            head,
            attempt,
            status,
            writable,
        } => {
            if head != Head::Current {
                out.push(Effect::Error(Failure::PullChanged));
            } else if state
                .rerun
                .is_some_and(|r| r.accepted || attempt > r.attempt)
            {
                out.push(Effect::Continue);
            } else if state.rerun.is_some_and(|r| !r.waiting) {
                out.push(Effect::Error(Failure::RerunUnknown));
            } else if status != Status::Completed {
                state.rerun = Some(Rerun {
                    attempt,
                    accepted: false,
                    waiting: true,
                });
                out.push(Effect::SaveRerun);
                out.push(Effect::Error(Failure::RerunFinishing));
            } else if writable.is_none() {
                out.push(Effect::Read(Need::RerunPull));
            } else if writable == Some(false) {
                out.push(Effect::Error(Failure::PullChanged));
            } else {
                state.rerun = Some(Rerun {
                    attempt,
                    accepted: false,
                    waiting: false,
                });
                out.push(Effect::SaveRerun);
                out.push(Effect::Write(Write::Rerun));
            }
        }
        QueueEvent::RerunDone(result) => {
            match result {
                RerunResult::Accepted => {
                    if let Some(rerun) = &mut state.rerun {
                        rerun.accepted = true;
                    }
                }
                RerunResult::InProgress => {
                    if let Some(rerun) = &mut state.rerun {
                        rerun.waiting = true;
                    }
                }
                RerunResult::Refused => state.rerun = None,
                RerunResult::Unknown => {}
            }
            out.push(Effect::SaveRerun);
        }
        QueueEvent::RerunPending {
            head,
            attempt,
            status,
        } => {
            let pending = state.rerun.is_some_and(|saved| {
                !saved.accepted
                    && (head != Head::Current
                        || attempt <= saved.attempt
                            && (!saved.waiting || status != Status::Completed))
            });
            out.push(Effect::RerunPending(pending));
        }
    }
    (state, out)
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RequiredState {
    Pending,
    Passed,
    Infrastructure,
    Tests,
    Unknown,
}

pub fn required_check(check: super::CheckView, runner_failure: bool) -> RequiredState {
    use super::{App, Conclusion};
    if check.status != Status::Completed
        || matches!(
            check.conclusion,
            Some(Conclusion::ActionRequired | Conclusion::Stale)
        )
    {
        RequiredState::Pending
    } else if matches!(
        check.conclusion,
        Some(Conclusion::Success | Conclusion::Neutral | Conclusion::Skipped)
    ) {
        RequiredState::Passed
    } else if check.app == App::Actions
        && (matches!(
            check.conclusion,
            Some(Conclusion::Cancelled | Conclusion::TimedOut | Conclusion::StartupFailure)
        ) || check.conclusion == Some(Conclusion::Failure) && runner_failure)
    {
        RequiredState::Infrastructure
    } else if check.conclusion.is_some() {
        RequiredState::Tests
    } else {
        RequiredState::Unknown
    }
}
