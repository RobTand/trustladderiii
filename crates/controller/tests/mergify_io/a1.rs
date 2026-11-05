use super::*;
use tl3_mergify::decide::{Head, Time, completion as c};

/// #608/#373: witnesses for each completion mapping, receipt and conflict branch.
#[test]
fn mandatory_a1_completion_mapping() {
    for progress in [false, true] {
        for conflict in [false, true] {
            for observed in [false, true] {
                assert_eq!(
                    c::resolve::<u8>(c::Complete::Confirmed { progress, conflict }, observed),
                    c::Resolution::Found {
                        ack: c::Ack::Confirmed,
                        conflict,
                        progress: progress && observed
                    }
                );
            }
        }
    }
    assert_eq!(
        c::resolve(c::Complete::<u8>::Retryable, true),
        c::Resolution::Found {
            ack: c::Ack::Retryable,
            conflict: false,
            progress: false
        }
    );
    assert_eq!(
        c::resolve(c::Complete::Permanent(7u8), true),
        c::Resolution::Found {
            ack: c::Ack::Permanent(7),
            conflict: false,
            progress: false
        }
    );
    assert_eq!(
        c::resolve(c::Complete::<u8>::Pending, true),
        c::Resolution::Pending
    );
    assert_eq!(
        c::resolve(c::Complete::<u8>::Absent, true),
        c::Resolution::Absent
    );
    for receipt in [c::Receipt::Found, c::Receipt::Pending, c::Receipt::Absent] {
        assert_eq!(c::confirmed(receipt), receipt == c::Receipt::Found);
    }
    assert_eq!(c::head(true), Head::Current);
    assert_eq!(c::head(false), Head::Other);
    for mergeable in [None, Some(true), Some(false)] {
        assert_eq!(c::conflicts(mergeable), mergeable == Some(false));
    }
}

/// #608/#373: merged holds skip the projection, baselines skip repaired_head.
#[test]
fn mandatory_a1_person_hold_and_freshness() {
    for queue in [QueueState::Merged, QueueState::Waiting] {
        for unresolved in [None, Some(false), Some(true)] {
            assert_eq!(
                c::held(queue, unresolved),
                if queue == QueueState::Merged {
                    Some(false)
                } else {
                    unresolved
                }
            );
        }
    }
    for repaired in [None, Some(false), Some(true)] {
        assert_eq!(c::person_events(true, repaired), Some(true));
        assert_eq!(c::person_events(false, repaired), repaired);
    }
    assert_eq!(c::person_event(c::PersonEvidence::Dequeued(2)), Some(2));
    assert_eq!(c::person_event(c::PersonEvidence::Closed(3)), Some(3));
    assert_eq!(c::person_event(c::PersonEvidence::Other), None);
    for event in [0, 9, 10, 11, u64::MAX] {
        assert!(!c::fresh(event, c::Freshness::Intent { last_event: None }));
        assert_eq!(
            c::fresh(
                event,
                c::Freshness::Intent {
                    last_event: Some(10)
                }
            ),
            event > 10
        );
        assert!(!c::fresh(
            event,
            c::Freshness::Snapshot { unresolved: false }
        ));
        assert!(c::fresh(event, c::Freshness::Snapshot { unresolved: true }));
    }
}

/// #608: newest label is the largest id, last on a tie, irrespective of action.
#[test]
fn mandatory_a1_labels_dequeue_and_time() {
    let labels = [
        c::Label::Queue,
        c::Label::Queued,
        c::Label::Dequeued,
        c::Label::Other,
    ];
    for label in labels {
        assert!(!c::has_label(&[], label));
        assert!(c::has_label(&labels, label));
        assert!(!c::has_label(&[c::Label::Other], c::Label::Queue));
    }
    for queue in [
        QueueState::Waiting,
        QueueState::Queued,
        QueueState::ChecksRunning,
        QueueState::Merged,
        QueueState::Dequeued,
        QueueState::FailedCi,
        QueueState::Conflict,
        QueueState::DequeuedByPerson,
        QueueState::Unknown,
    ] {
        assert!(c::retry(true, queue));
        assert_eq!(
            c::retry(false, queue),
            matches!(
                queue,
                QueueState::Dequeued | QueueState::FailedCi | QueueState::Conflict
            )
        );
    }
    for retry in [false, true] {
        for concluded in [false, true] {
            for label in [false, true] {
                assert_eq!(
                    c::dequeue_gate(retry, concluded, label),
                    if !retry || concluded {
                        c::DequeueGate::Done
                    } else if !label {
                        c::DequeueGate::Pending
                    } else {
                        c::DequeueGate::Events
                    }
                );
            }
        }
    }
    let event = |id, label, action| c::LabelEvent {
        id,
        label,
        action,
        at: Time {
            secs: -1,
            nanos: 1_999_999_999,
        },
    };
    let events = [
        event(2, c::Label::Dequeued, c::LabelAction::Labeled),
        event(3, c::Label::Queued, c::LabelAction::Labeled),
        event(1, c::Label::Dequeued, c::LabelAction::Other),
        event(2, c::Label::Dequeued, c::LabelAction::Other),
    ];
    assert_eq!(c::newest(&[], c::Label::Dequeued), None);
    assert_eq!(c::newest(&events, c::Label::Queue), None);
    assert_eq!(c::newest(&events, c::Label::Dequeued), Some(3));
    assert_eq!(c::newest(&events, c::Label::Queued), Some(1));
    assert!(c::labeled(c::LabelAction::Labeled));
    assert!(!c::labeled(c::LabelAction::Other));
    for secs in [i64::MIN, -1, 0, i64::MAX] {
        for nanos in [0, 999_999_999, 1_999_999_999, u32::MAX] {
            assert_eq!(c::time(secs, nanos), Time { secs, nanos });
        }
    }
}

/// #447/Q52: proof checks and body heals retain the original early returns.
#[test]
fn mandatory_a1_read_gates() {
    assert!(!c::proof_missing(c::Proof::Empty));
    assert!(!c::proof_missing(c::Proof::Present));
    assert!(c::proof_missing(c::Proof::Missing));
    assert!(c::reject_queue_pr(true));
    assert!(!c::reject_queue_pr(false));
    assert!(c::read_facts(false));
    assert!(!c::read_facts(true));
}

/// #760: each new boundary projection is the production function verbatim.
#[test]
fn mandatory_a1_boundary_source() {
    fn function<'a>(source: &'a str, name: &str) -> &'a str {
        let start = source.find(&format!("fn {name}(")).unwrap();
        let end = start + source[start..].find("\n}\n").unwrap() + 3;
        &source[start..end]
    }
    for name in [
        "lifecycle_head",
        "completion_label",
        "completion_receipt",
        "completion_evidence",
        "completion_check",
        "completion_hold",
    ] {
        assert_eq!(
            function(include_str!("../../src/adapters/mergify.rs"), name),
            function(include_str!("current.rs"), name),
            "{name}"
        );
    }
}

/// #760: every canonical field can acknowledge change, while absent checks differ.
#[test]
fn mandatory_a1_check_comparison() {
    assert!(!c::check_changed(c::CheckComparison::BothMissing));
    assert!(c::check_changed(c::CheckComparison::OneMissing));
    assert!(!c::check_changed(c::CheckComparison::BothPresent {
        fields: [true; 10]
    }));
    for changed in 0..10 {
        let mut fields = [true; 10];
        fields[changed] = false;
        assert!(c::check_changed(c::CheckComparison::BothPresent { fields }));
    }
}
