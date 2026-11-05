//! Scalar inputs range over their full domains. Each transition emits at most
//! three effects. Unwind 4 covers those three slots and the loop exit; the
//! remaining proofs each exercise one transition, with no temporal loop.
//!
//! Property traceability:
//! - `enqueue_ack_sound`: state-table Publishing, step 4 confirms before Queued.
//! - `enqueue_stale_head`: Reviewing's Approved(head) row and #608's dispatch fence.
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



proof!(enqueue_stale_head, {
    let state: EnqueueState = kani::any();
    let event: QueueEvent = kani::any();
    let stale = match event {
        QueueEvent::Reconcile(read) => {
            read.head == Head::Other && read.queue != QueueState::Merged && !state.confirmed
        }
        QueueEvent::LabelRead { head, .. } | QueueEvent::RerunRead { head, .. } => {
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
