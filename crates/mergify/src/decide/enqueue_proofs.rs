//! Scalar inputs range over their full domains. Each transition emits at most
//! three effects. Unwind 4 covers those three slots and the loop exit; the
//! grace proof additionally applies two successive transitions, with no loop.
//!
//! Property traceability:
//! - `enqueue_ack_sound`: state-table Publishing, step 4 confirms before Queued.
//! - `enqueue_stale_head`: Reviewing's Approved(head) row and #608's dispatch fence.
//! - `enqueue_refused_sticky`: #608, preserve terminal command refusal handling.
//! - `enqueue_write_ahead`: #608, persist intent before upstream dispatch.
//! - `enqueue_recovery_grace`: #608, five-minute grace and one command per head.
//! - `enqueue_dequeue_fresh`: #608, retain prior dequeue without reusing stale labels.
//! - `enqueue_required_check`: Q47, distinguish code failures from runner failures.
use super::enqueue::*;
use super::{Head, QueueState};

macro_rules! proof {
    ($name:ident, $body:block) => {
        #[kani::proof]
        #[kani::unwind(4)]
        fn $name() $body
    };
}

fn first(effects: &Effects) -> Effect {
    effects.get(0).unwrap()
}
fn refused(state: EnqueueState) -> bool {
    matches!(
        state.intent,
        Some(Intent {
            requeue: Some(Requeue::Refused)
        })
    )
}

proof!(enqueue_ack_sound, {
    let state: EnqueueState = kani::any();
    let read: QueueRead = kani::any();
    let (next, effects) = decide(state, QueueEvent::Reconcile(read));
    // Independent outcome oracle. It does not call a production predicate.
    let expected = if refused(state) {
        Effect::SaveRecovery
    } else if read.queue != QueueState::Merged && read.held {
        Effect::Complete(Complete::Pending)
    } else if !state.confirmed
        && read.queue != QueueState::Merged
        && matches!(
            state.intent,
            Some(Intent {
                requeue: Some(Requeue::Sending)
            })
        )
        && read.refusal.is_none()
    {
        Effect::Read(Need::Refusal)
    } else if !state.confirmed
        && read.queue != QueueState::Merged
        && matches!(
            state.intent,
            Some(Intent {
                requeue: Some(Requeue::Sending)
            })
        )
        && read.refusal == Some(true)
    {
        Effect::SaveRecovery
    } else if !state.confirmed && read.reruns_pending.is_none() {
        Effect::Read(Need::Reruns)
    } else if !state.confirmed && read.reruns_pending == Some(true) {
        Effect::Complete(Complete::Pending)
    } else if read.head == Head::Other && read.queue != QueueState::Merged && !state.confirmed {
        Effect::Complete(Complete::Retryable)
    } else if read.head == Head::Current && !read.merged && read.conflicts {
        Effect::Complete(Complete::Confirmed {
            progress: false,
            conflict: true,
        })
    } else if read.person_removed.is_none() {
        Effect::Read(Need::Person)
    } else if state.confirmed
        || read.person_removed == Some(true)
        || read.queue == QueueState::Merged
        || read.head == Head::Current
            && (matches!(read.queue, QueueState::Queued | QueueState::ChecksRunning)
                || matches!(
                    read.queue,
                    QueueState::Dequeued
                        | QueueState::FailedCi
                        | QueueState::Conflict
                        | QueueState::DequeuedByPerson
                ) && matches!(state.intent, Some(Intent { requeue: None }))
                    && read.check_changed)
    {
        Effect::Complete(Complete::Confirmed {
            progress: true,
            conflict: false,
        })
    } else {
        match read.issue {
            None => Effect::Read(Need::Issue),
            Some(issue) => {
                let recovery_wait = read.queue == QueueState::Waiting && issue.queue_label;
                if recovery_wait && issue.recovery.is_none() {
                    Effect::Read(Need::RecoveryFile)
                } else if recovery_wait && issue.recovery == Some(true) {
                    Effect::Complete(Complete::Pending)
                } else if recovery_wait && issue.repaired.is_none() {
                    Effect::Read(Need::RepairedHead)
                } else if recovery_wait && issue.repaired == Some(true) {
                    Effect::Complete(Complete::Pending)
                } else {
                    match state.intent {
                        None => match issue.dequeue_pending {
                            None => Effect::Read(Need::Dequeue),
                            Some(true) => Effect::Complete(Complete::Pending),
                            Some(false) => Effect::Complete(Complete::Absent),
                        },
                        Some(Intent {
                            requeue: Some(command),
                        }) => match issue.recovery {
                            None => Effect::Read(Need::RecoveryFile),
                            Some(true) => Effect::Complete(Complete::Pending),
                            Some(false) if command == Requeue::Sending => {
                                Effect::Complete(Complete::Pending)
                            }
                            Some(false) => match issue.dequeue_pending {
                                None => Effect::Read(Need::Dequeue),
                                Some(true) => Effect::Complete(Complete::Pending),
                                Some(false) => Effect::Complete(Complete::Absent),
                            },
                        },
                        Some(Intent { requeue: None }) => {
                            if issue.queue_label {
                                Effect::Complete(Complete::Pending)
                            } else {
                                match issue.applied {
                                    None => Effect::Read(Need::Applied),
                                    Some(true) => Effect::Complete(Complete::Pending),
                                    Some(false) => Effect::Complete(Complete::Absent),
                                }
                            }
                        }
                    }
                }
            }
        }
    };
    assert_eq!(first(&effects), expected, "enqueue outcome misjudged");
    assert_eq!(
        next.confirmed,
        state.confirmed || matches!(expected, Effect::Complete(Complete::Confirmed { .. })),
        "acknowledgement missing from next state"
    );
    if refused(state) {
        assert_eq!(effects.len(), 2);
        assert_eq!(
            effects.get(1),
            Some(Effect::Complete(Complete::Permanent(Failure::Refused)))
        );
    } else if expected == Effect::SaveRecovery {
        assert_eq!(effects.len(), 3);
        assert_eq!(effects.get(1), Some(Effect::SaveIntent));
        assert_eq!(
            effects.get(2),
            Some(Effect::Complete(Complete::Permanent(Failure::Refused)))
        );
        assert_eq!(
            next.intent,
            Some(Intent {
                requeue: Some(Requeue::Refused)
            })
        );
    } else {
        assert_eq!(effects.len(), 1);
    }
    kani::cover!(
        expected == Effect::Complete(Complete::Pending),
        "await acknowledgement"
    );
    kani::cover!(
        expected == Effect::Complete(Complete::Absent),
        "no write was applied"
    );
    kani::cover!(
        expected
            == Effect::Complete(Complete::Confirmed {
                progress: true,
                conflict: false
            })
            && read.queue == QueueState::FailedCi
            && !state.confirmed,
        "changed terminal acknowledges"
    );
    kani::cover!(
        expected
            == Effect::Complete(Complete::Confirmed {
                progress: false,
                conflict: true
            }),
        "pre-enqueue conflict"
    );
});

proof!(enqueue_stale_head, {
    let state: EnqueueState = kani::any();
    let event: QueueEvent = kani::any();
    let stale = match event {
        QueueEvent::Reconcile(read) => {
            read.head == Head::Other && read.queue != QueueState::Merged && !state.confirmed
        }
        QueueEvent::Prepare { head, .. } | QueueEvent::RerunRead { head, .. } => {
            head == Head::Other
        }
        QueueEvent::DispatchCommand(read) => read.head == Head::Other,
        _ => false,
    };
    let (_, effects) = decide(state, event);
    for i in 0..effects.len() {
        if stale {
            assert!(
                !matches!(effects.get(i), Some(Effect::Write(_))),
                "stale head wrote upstream"
            );
            assert!(
                !matches!(
                    effects.get(i),
                    Some(Effect::Complete(Complete::Confirmed { .. }))
                ),
                "stale head acknowledged"
            );
        }
    }
    if let QueueEvent::Reconcile(read) = event {
        if stale
            && !refused(state)
            && !read.held
            && read.reruns_pending == Some(false)
            && (!matches!(
                state.intent,
                Some(Intent {
                    requeue: Some(Requeue::Sending)
                })
            ) || read.refusal == Some(false))
        {
            assert_eq!(
                first(&effects),
                Effect::Complete(Complete::Retryable),
                "stale head must retry"
            );
        }
    }
    kani::cover!(
        stale && first(&effects) == Effect::Complete(Complete::Retryable),
        "stale reconcile retries"
    );
    kani::cover!(
        stale && first(&effects) == Effect::Complete(Complete::Pending),
        "held stale head waits"
    );
    kani::cover!(
        matches!(
            event,
            QueueEvent::DispatchCommand(CommandRead {
                head: Head::Other,
                ..
            })
        ) && !refused(state),
        "stale command blocked"
    );
});

proof!(enqueue_refused_sticky, {
    let mut state: EnqueueState = kani::any();
    state.intent = Some(Intent {
        requeue: Some(Requeue::Refused),
    });
    let event: QueueEvent = kani::any();
    let (next, effects) = decide(state, event);
    assert_eq!(
        next.intent, state.intent,
        "refused enqueue lost its receipt"
    );
    assert_eq!(effects.len(), 2);
    assert_eq!(first(&effects), Effect::SaveRecovery);
    assert_eq!(
        effects.get(1),
        Some(Effect::Complete(Complete::Permanent(Failure::Refused))),
        "refusal must be permanent"
    );
    assert_eq!(next.recovery.unwrap().command, Command::Refused);
    kani::cover!(
        matches!(event, QueueEvent::Prepare { .. }),
        "refused execute"
    );
    kani::cover!(
        matches!(event, QueueEvent::RecoveryTick { .. }),
        "refused recovery"
    );
    kani::cover!(
        matches!(event, QueueEvent::Reconcile(_)),
        "refused reconcile"
    );
    let command: Command = kani::any();
    kani::assume(matches!(
        command,
        Command::Refused | Command::PermissionDenied | Command::Rejected
    ));
    let recovery = Recovery {
        command,
        eligible_since_ms: kani::any(),
    };
    let event: QueueEvent = kani::any();
    kani::assume(matches!(
        event,
        QueueEvent::CommandStatus
            | QueueEvent::CommandReceipt { .. }
            | QueueEvent::DispatchCommand(_)
            | QueueEvent::CommandDone(_)
            | QueueEvent::RecoveryTick { .. }
    ));
    let (next, effects) = decide(
        EnqueueState {
            recovery: Some(recovery),
            ..EnqueueState::default()
        },
        event,
    );
    assert_eq!(next.recovery, Some(recovery), "a terminal command changed");
    let failure = match command {
        Command::Refused => Failure::Refused,
        Command::PermissionDenied => Failure::Permission,
        Command::Rejected => Failure::Rejected,
        _ => unreachable!(),
    };
    assert_eq!(
        effects.get(effects.len() - 1),
        Some(Effect::Complete(Complete::Permanent(failure))),
        "terminal command must stay permanent"
    );
    for i in 0..effects.len() {
        assert!(
            !matches!(effects.get(i), Some(Effect::Write(_))),
            "terminal command retried"
        );
    }
    kani::cover!(
        command == Command::Refused
            && matches!(event, QueueEvent::CommandDone(CommandResult::Accepted)),
        "late success cannot erase refusal"
    );
});

proof!(enqueue_write_ahead, {
    let state: EnqueueState = kani::any();
    let event: QueueEvent = kani::any();
    let (next, effects) = decide(state, event);
    let mut intent = false;
    let mut recovery = false;
    let mut rerun = false;
    let mut label_write = false;
    let mut command_write = false;
    let mut rerun_write = false;
    for i in 0..effects.len() {
        match effects.get(i).unwrap() {
            Effect::SaveIntent => intent = true,
            Effect::SaveRecovery => recovery = true,
            Effect::SaveRerun => rerun = true,
            Effect::Write(Write::Label) => {
                label_write = true;
                assert!(intent && next.intent.is_some(), "label without intent");
            }
            Effect::Write(Write::Command) => {
                command_write = true;
                assert!(
                    recovery && next.recovery.unwrap().command == Command::Sending,
                    "command without intent"
                );
            }
            Effect::Write(Write::Rerun) => {
                rerun_write = true;
                assert!(
                    rerun && next.rerun.is_some_and(|r| !r.accepted && !r.waiting),
                    "rerun without intent"
                );
            }
            _ => {}
        }
    }
    kani::cover!(label_write, "label write has intent");
    kani::cover!(command_write, "command write has intent");
    kani::cover!(rerun_write, "rerun write has intent");
});

proof!(enqueue_recovery_grace, {
    let now_ms: u64 = kani::any();
    let eligible: bool = kani::any();
    let recovery: Option<Recovery> = kani::any();
    let state = EnqueueState {
        recovery,
        ..EnqueueState::default()
    };
    let (next, effects) = decide(state, QueueEvent::RecoveryTick { eligible, now_ms });
    let expected_command = recovery.map_or(Command::Ready, |r| r.command);
    let ready = expected_command == Command::Ready;
    if ready {
        let since = recovery.and_then(|r| r.eligible_since_ms).unwrap_or(now_ms);
        assert_eq!(
            next.recovery.unwrap().eligible_since_ms,
            eligible.then_some(since),
            "grace timer misjudged"
        );
        assert_eq!(
            effects.get(1),
            Some(if eligible && now_ms.saturating_sub(since) >= 300_000 {
                Effect::Read(Need::Command)
            } else {
                Effect::Complete(Complete::Pending)
            }),
            "recovery grace misjudged"
        );
    }
    let read = CommandRead {
        head: Head::Current,
        open: true,
        merged: false,
        draft: false,
        held: false,
        paused: false,
        waiting: true,
        eligible,
        now_ms,
    };
    let (sent, dispatch) = decide(next, QueueEvent::DispatchCommand(read));
    let wrote = dispatch.get(1) == Some(Effect::Write(Write::Command));
    let matured = recovery
        .and_then(|r| r.eligible_since_ms)
        .is_some_and(|since| now_ms.saturating_sub(since) >= 300_000);
    assert_eq!(
        wrote,
        ready && eligible && matured,
        "recovery sent before grace or failed to send"
    );
    let (_, again) = decide(sent, QueueEvent::DispatchCommand(read));
    for i in 0..again.len() {
        assert!(
            !matches!(again.get(i), Some(Effect::Write(_))),
            "recovery sent twice"
        );
    }
    kani::cover!(ready && eligible && !matured, "grace still running");
    kani::cover!(wrote, "grace expired and command sent");
    kani::cover!(
        !ready && expected_command == Command::Sending,
        "lost response consumes budget"
    );
    kani::cover!(
        ready && !eligible && recovery.is_some(),
        "eligibility reset"
    );
});

proof!(enqueue_dequeue_fresh, {
    let retry: bool = kani::any();
    let concluded: bool = kani::any();
    let labeled: bool = kani::any();
    let dequeued: Option<LabelEvent> = kani::any();
    let queued: Option<u64> = kani::any();
    let finished: Option<super::Time> = kani::any();
    let fresh = match dequeued {
        None => false,
        Some(event) => {
            event.labeled
                && match queued {
                    Some(queued) => event.id > queued,
                    None => match finished {
                        Some(finished) => event.at >= finished,
                        None => false,
                    },
                }
        }
    };
    let pending = dequeue_pending(retry, concluded, labeled, dequeued, queued, finished);
    assert_eq!(
        pending,
        retry && !concluded && (!labeled || !fresh),
        "dequeue freshness misjudged"
    );
    kani::cover!(pending && !labeled, "wait for label");
    kani::cover!(pending && labeled && !fresh, "ignore old cycle");
    kani::cover!(
        !pending && retry && !concluded && fresh,
        "fresh event finishes dequeue"
    );
});

proof!(enqueue_required_check, {
    use super::{App, Conclusion, Status};
    let check: super::CheckView = kani::any();
    let runner: bool = kani::any();
    let expected = match (check.status, check.conclusion) {
        (
            Status::Completed,
            Some(Conclusion::Success | Conclusion::Neutral | Conclusion::Skipped),
        ) => RequiredState::Passed,
        (Status::Completed, Some(Conclusion::ActionRequired | Conclusion::Stale)) => {
            RequiredState::Pending
        }
        (Status::Completed, Some(conclusion)) => {
            if check.app == App::Actions
                && (matches!(
                    conclusion,
                    Conclusion::Cancelled | Conclusion::TimedOut | Conclusion::StartupFailure
                ) || conclusion == Conclusion::Failure && runner)
            {
                RequiredState::Infrastructure
            } else {
                RequiredState::Tests
            }
        }
        (Status::Completed, None) => RequiredState::Unknown,
        _ => RequiredState::Pending,
    };
    assert_eq!(
        required_check(check, runner),
        expected,
        "required check misclassified"
    );
    kani::cover!(expected == RequiredState::Passed, "head check passed");
    kani::cover!(expected == RequiredState::Pending, "head check pending");
    kani::cover!(expected == RequiredState::Unknown, "no conclusion");
    kani::cover!(expected == RequiredState::Tests, "test failure");
    kani::cover!(
        expected == RequiredState::Infrastructure && check.conclusion == Some(Conclusion::Failure),
        "runner annotation"
    );
    kani::cover!(
        expected == RequiredState::Infrastructure
            && check.conclusion == Some(Conclusion::Cancelled),
        "cancelled runner"
    );
});
