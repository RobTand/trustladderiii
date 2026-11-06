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

// Frozen rust-fv 4a7fa87a source, Git blob c6d3fc41e97e74f41afa282d81445eb67331e42b, not a new implementation.
const BASE: &str = include_str!("enqueue_base.rs.txt");
const REFERENCE: &str = include_str!("enqueue_reference.rs.txt");

fn sha256(source: &str) -> String {
    use std::io::Write;
    use std::process::{Command, Stdio};
    // Use the executor's coreutils instead of adding a cryptography dependency.
    let mut child = Command::new("sha256sum")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("sha256sum is required for frozen-reference fidelity");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(source.as_bytes())
        .unwrap();
    let result = child.wait_with_output().unwrap();
    assert!(result.status.success());
    String::from_utf8(result.stdout)
        .unwrap()
        .split_whitespace()
        .next()
        .unwrap()
        .into()
}

fn original_function<'a>(source: &'a str, name: &str) -> &'a str {
    let start = source
        .find(&format!("fn {name}("))
        .expect("missing original function");
    let body = start + source[start..].find('{').unwrap();
    let mut depth = 0;
    for (offset, byte) in source.as_bytes()[body..].iter().enumerate() {
        if *byte == b'{' {
            depth += 1;
        }
        if *byte == b'}' {
            depth -= 1;
            if depth == 0 {
                return &source[start..=body + offset];
            }
        }
    }
    panic!("unclosed original function");
}

#[test]
fn reference_is_pinned_and_matches_original_rust_fv_source() {
    assert_eq!(
        sha256(REFERENCE),
        "6ddc0fc592d5e34373f316c4031ddae65e7a49ac67a0f02cfa5eedf67dac1121"
    );
    assert_eq!(
        sha256(BASE),
        "522ef8320a2210882c79bf978db461de8430307802b186a4fa48b4a5606a813a"
    );
    for name in ["command_completion", "reconcile_issue", "decide"] {
        assert_eq!(
            original_function(REFERENCE, name),
            original_function(BASE, name),
            "frozen {name} differs from rust-fv 4a7fa87a"
        );
    }
}
