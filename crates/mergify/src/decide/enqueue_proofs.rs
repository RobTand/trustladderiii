//! Scalar inputs range over their full domains. Each transition emits at most
//! three effects. Unwind 4 covers those three slots and the loop exit; the
//! grace proof additionally applies two successive transitions, with no loop.
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
    let (_, effects) = decide(state, QueueEvent::Reconcile(read));
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
                let pending = read.queue == QueueState::Waiting
                    && issue.queue_label
                    && (issue.recovery || issue.repaired)
                    || match state.intent {
                        None => issue.dequeue_pending,
                        Some(Intent {
                            requeue: Some(command),
                        }) => {
                            issue.recovery || command == Requeue::Sending || issue.dequeue_pending
                        }
                        Some(Intent { requeue: None }) => issue.queue_label || issue.applied,
                    };
                Effect::Complete(if pending {
                    Complete::Pending
                } else {
                    Complete::Absent
                })
            }
        }
    };
    assert_eq!(first(&effects), expected, "enqueue outcome misjudged");
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
