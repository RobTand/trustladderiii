//! #608/#751/#760: Verus-driven syntax changes retain the pre-port decision.
use proptest::prelude::*;
use proptest::test_runner::{Config, FileFailurePersistence, RngSeed};
use tl3_mergify::decide::enqueue::*;
use tl3_mergify::decide::{Head, QueueState, Status};

mod reference {
    include!("enqueue_reference.rs.txt");
}

fn head() -> impl Strategy<Value = Head> {
    prop_oneof![Just(Head::Current), Just(Head::Other)]
}

fn timestamp() -> impl Strategy<Value = u64> {
    prop_oneof![
        any::<u64>(),
        Just(0),
        Just(1),
        Just(299_999),
        Just(300_000),
        Just(300_001),
        Just(u64::MAX)
    ]
}

fn command() -> impl Strategy<Value = Command> {
    prop_oneof![
        Just(Command::Ready),
        Just(Command::Sending),
        Just(Command::Sent),
        Just(Command::Refused),
        Just(Command::PermissionDenied),
        Just(Command::Rejected)
    ]
}

fn status() -> impl Strategy<Value = Status> {
    prop_oneof![
        Just(Status::Queued),
        Just(Status::InProgress),
        Just(Status::Completed),
        Just(Status::Waiting),
        Just(Status::Requested),
        Just(Status::Pending),
        Just(Status::Other)
    ]
}

fn state() -> impl Strategy<Value = EnqueueState> {
    let intent = prop::option::of(
        prop::option::of(prop_oneof![
            Just(Requeue::Ready),
            Just(Requeue::Sending),
            Just(Requeue::Refused)
        ])
        .prop_map(|requeue| Intent { requeue }),
    );
    let recovery = prop::option::of((prop::option::of(timestamp()), command()).prop_map(
        |(eligible_since_ms, command)| Recovery {
            eligible_since_ms,
            command,
        },
    ));
    let rerun = prop::option::of((timestamp(), any::<bool>(), any::<bool>()).prop_map(
        |(attempt, accepted, waiting)| Rerun {
            attempt,
            accepted,
            waiting,
        },
    ));
    (intent, any::<bool>(), recovery, rerun).prop_map(|(intent, confirmed, recovery, rerun)| {
        EnqueueState {
            intent,
            confirmed,
            recovery,
            rerun,
        }
    })
}

fn queue_read() -> impl Strategy<Value = QueueRead> {
    let queue = prop_oneof![
        Just(QueueState::Waiting),
        Just(QueueState::Queued),
        Just(QueueState::ChecksRunning),
        Just(QueueState::Merged),
        Just(QueueState::Dequeued),
        Just(QueueState::DequeuedByPerson),
        Just(QueueState::FailedCi),
        Just(QueueState::Conflict),
        Just(QueueState::Unknown)
    ];
    let issue = prop::option::of(
        (
            any::<bool>(),
            prop::option::of(any::<bool>()),
            prop::option::of(any::<bool>()),
            prop::option::of(any::<bool>()),
            prop::option::of(any::<bool>()),
            prop::option::of(any::<bool>()),
        )
            .prop_map(
                |(queue_label, repaired, recovery, requeue_recovery, dequeue_pending, applied)| {
                    IssueRead {
                        queue_label,
                        repaired,
                        recovery,
                        requeue_recovery,
                        dequeue_pending,
                        applied,
                    }
                },
            ),
    );
    (
        head(),
        queue,
        any::<bool>(),
        any::<bool>(),
        any::<bool>(),
        any::<bool>(),
        prop::option::of(any::<bool>()),
        prop::option::of(any::<bool>()),
        prop::option::of(any::<bool>()),
        issue,
    )
        .prop_map(
            |(
                head,
                queue,
                merged,
                conflicts,
                held,
                check_changed,
                refusal,
                reruns_pending,
                person_removed,
                issue,
            )| QueueRead {
                head,
                queue,
                merged,
                conflicts,
                held,
                check_changed,
                refusal,
                reruns_pending,
                person_removed,
                issue,
            },
        )
}

fn command_read() -> impl Strategy<Value = CommandRead> {
    (
        head(),
        any::<bool>(),
        any::<bool>(),
        any::<bool>(),
        any::<bool>(),
        any::<bool>(),
        any::<bool>(),
        any::<bool>(),
        timestamp(),
    )
        .prop_map(
            |(head, open, merged, draft, held, paused, waiting, eligible, now_ms)| CommandRead {
                head,
                open,
                merged,
                draft,
                held,
                paused,
                waiting,
                eligible,
                now_ms,
            },
        )
}

fn event() -> impl Strategy<Value = QueueEvent> {
    prop_oneof![
        Just(QueueEvent::Begin),
        (any::<bool>(), prop::option::of(any::<bool>())).prop_map(
            |(dequeue_pending, checks_ready)| QueueEvent::ExecuteGate {
                dequeue_pending,
                checks_ready
            }
        ),
        queue_read().prop_map(QueueEvent::Reconcile),
        (any::<bool>(), any::<bool>(), any::<bool>()).prop_map(
            |(dequeue_pending, checks_ready, queue_label)| QueueEvent::Prepare {
                dequeue_pending,
                checks_ready,
                queue_label
            }
        ),
        (head(), any::<bool>(), any::<bool>())
            .prop_map(|(head, open, merged)| QueueEvent::LabelRead { head, open, merged }),
        (any::<bool>(), timestamp())
            .prop_map(|(eligible, now_ms)| QueueEvent::RecoveryTick { eligible, now_ms }),
        Just(QueueEvent::CommandStatus),
        (any::<bool>(), any::<bool>())
            .prop_map(|(refused, found)| QueueEvent::CommandReceipt { refused, found }),
        command_read().prop_map(QueueEvent::DispatchCommand),
        prop_oneof![
            Just(CommandResult::Accepted),
            Just(CommandResult::PermissionDenied),
            Just(CommandResult::Rejected),
            Just(CommandResult::Unknown)
        ]
        .prop_map(QueueEvent::CommandDone),
        (
            head(),
            timestamp(),
            status(),
            prop::option::of(any::<bool>())
        )
            .prop_map(|(head, attempt, status, writable)| QueueEvent::RerunRead {
                head,
                attempt,
                status,
                writable
            }),
        prop_oneof![
            Just(RerunResult::Accepted),
            Just(RerunResult::InProgress),
            Just(RerunResult::Refused),
            Just(RerunResult::Unknown)
        ]
        .prop_map(QueueEvent::RerunDone),
        (head(), timestamp(), status()).prop_map(|(head, attempt, status)| {
            QueueEvent::RerunPending {
                head,
                attempt,
                status,
            }
        }),
    ]
}

proptest! {
    #![proptest_config(Config {
        cases: 1024,
        rng_seed: RngSeed::Fixed(0x751760b2),
        failure_persistence: Some(Box::new(FileFailurePersistence::Direct(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/enqueue-regressions.txt")))),
        ..Config::default()
    })]
    #[test]
    fn real_decide_matches_verbatim_rust_fv(state in state(), event in event()) {
        prop_assert_eq!(decide(state, event), reference::decide(state, event));
    }
}

// Minimized input from the red production-stub trial. Keep it independent of
// proptest's seed encoding and of Bazel's read-only runfiles directory.
#[test]
fn replay_red_unconfirmed_rerun() {
    let state = EnqueueState::default();
    let event = QueueEvent::RerunRead {
        head: Head::Current,
        attempt: 0,
        status: Status::Queued,
        writable: None,
    };
    assert_eq!(decide(state, event), reference::decide(state, event));
}
