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

// A4: direct old/new dequeue parity, independently of decide's reference.
// The reference is verbatim rust-fv 9a658a8e32 LabelEvent/dequeue_pending.
// Its SHA-256 is pinned by enqueue_a4_qualification.py and the source audit.
mod dequeue_reference {
    use tl3_mergify::decide::Time;
    include!("dequeue_reference.rs.txt");
}

fn dequeue_time() -> impl Strategy<Value = tl3_mergify::decide::Time> {
    (
        prop_oneof![
            any::<i64>(),
            Just(i64::MIN),
            Just(-1),
            Just(0),
            Just(i64::MAX)
        ],
        prop_oneof![
            any::<u32>(),
            Just(0),
            Just(999_999_999),
            Just(1_000_000_000),
            Just(u32::MAX)
        ],
    )
        .prop_map(|(secs, nanos)| tl3_mergify::decide::Time { secs, nanos })
}

fn dequeue_event() -> impl Strategy<Value = Option<LabelEvent>> {
    prop::option::of(
        (any::<u64>(), any::<bool>(), dequeue_time()).prop_map(|(id, labeled, at)| LabelEvent {
            id,
            labeled,
            at,
        }),
    )
}

// Keep independent full-domain samples and also make equality/adjacency common.
fn dequeue_samples() -> impl Strategy<
    Value = (
        Option<LabelEvent>,
        Option<u64>,
        Option<tl3_mergify::decide::Time>,
    ),
> {
    dequeue_event().prop_flat_map(|event| {
        let arbitrary_queued = prop::option::of(any::<u64>());
        let arbitrary_finished = prop::option::of(dequeue_time());
        let queued = match event {
            None => arbitrary_queued.boxed(),
            Some(event) => prop_oneof![
                arbitrary_queued,
                Just(Some(event.id)),
                Just(Some(event.id.saturating_sub(1))),
                Just(Some(event.id.saturating_add(1))),
            ]
            .boxed(),
        };
        let finished = match event {
            None => arbitrary_finished.boxed(),
            Some(event) => prop_oneof![
                arbitrary_finished,
                Just(Some(event.at)),
                Just(Some(tl3_mergify::decide::Time {
                    nanos: event.at.nanos.saturating_sub(1),
                    ..event.at
                })),
                Just(Some(tl3_mergify::decide::Time {
                    nanos: event.at.nanos.saturating_add(1),
                    ..event.at
                })),
            ]
            .boxed(),
        };
        (Just(event), queued, finished)
    })
}

fn old_dequeue_event(event: Option<LabelEvent>) -> Option<dequeue_reference::LabelEvent> {
    event.map(|event| dequeue_reference::LabelEvent {
        id: event.id,
        labeled: event.labeled,
        at: event.at,
    })
}

proptest! {
    #![proptest_config(Config {
        cases: 4096,
        rng_seed: RngSeed::Fixed(0x7517_60a4),
        failure_persistence: Some(Box::new(FileFailurePersistence::Direct(
            "tests/dequeue-regressions.txt"))),
        ..Config::default()
    })]
    #[test]
    fn dequeue_matches_verbatim_old(
        retry in any::<bool>(), concluded in any::<bool>(), labeled in any::<bool>(),
        (dequeued, queued, finished) in dequeue_samples(),
    ) {
        prop_assert_eq!(
            dequeue_pending(retry, concluded, labeled, dequeued, queued, finished),
            dequeue_reference::dequeue_pending(retry, concluded, labeled,
                old_dequeue_event(dequeued), queued, finished),
        );
    }
}

#[test]
fn dequeue_time_and_id_boundaries_match_verbatim_old() {
    use tl3_mergify::decide::Time;
    let times = [
        Time {
            secs: i64::MIN,
            nanos: 0,
        },
        Time {
            secs: -2,
            nanos: u32::MAX,
        },
        Time { secs: -1, nanos: 0 },
        Time {
            secs: -1,
            nanos: 999_999_999,
        },
        Time {
            secs: -1,
            nanos: 1_000_000_000,
        },
        Time { secs: 0, nanos: 0 },
        Time {
            secs: i64::MAX,
            nanos: u32::MAX,
        },
    ];
    for retry in [false, true] {
        for concluded in [false, true] {
            for labeled in [false, true] {
                for id in [0, 1, u64::MAX] {
                    for event_labeled in [false, true] {
                        for at in times {
                            let event = LabelEvent {
                                id,
                                labeled: event_labeled,
                                at,
                            };
                            for dequeued in [None, Some(event)] {
                                for queued in [
                                    None,
                                    Some(id),
                                    Some(id.saturating_sub(1)),
                                    Some(id.saturating_add(1)),
                                ] {
                                    for finished in
                                        std::iter::once(None).chain(times.into_iter().map(Some))
                                    {
                                        assert_eq!(
                                            dequeue_pending(
                                                retry, concluded, labeled, dequeued, queued,
                                                finished
                                            ),
                                            dequeue_reference::dequeue_pending(
                                                retry,
                                                concluded,
                                                labeled,
                                                old_dequeue_event(dequeued),
                                                queued,
                                                finished
                                            ),
                                            "retry={retry} concluded={concluded} labeled={labeled} dequeued={dequeued:?} queued={queued:?} finished={finished:?}",
                                        );
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
