//! #783: fixture logs detect corrupt snapshots and invalid real kernel steps.
use serde_json::json;
use tl3_controller::replay::{ReplayRecord, StepRecord, check_record};
use tl3_kernel::{Event, Priority, RepoId, World};
use tl3_store::{CommitReceipt, Epoch, EventRecord, Revision};

fn fixture() -> (EventRecord, World) {
    let before = World::new(&[1], 1);
    let event = Event::NewRequest {
        repo: RepoId(0),
        priority: Priority::Unranked,
    };
    let (after, _) = tl3_kernel::step(before.clone(), event);
    let replay = ReplayRecord {
        before: before.clone(),
        steps: vec![StepRecord {
            input: before.clone(),
            event,
            after: after.clone(),
        }],
        after,
    };
    (
        EventRecord {
            key: "fixture:event-2".into(),
            payload: json!({}),
            receipt: CommitReceipt {
                revision: Revision::try_from(2).unwrap(),
                epoch: Epoch::ZERO,
                result: json!({"kernel_replay":replay}),
            },
        },
        before,
    )
}

/// #783: recompute the stored fixture with the real step function.
#[test]
fn clean_fixture_replays() {
    let (record, before) = fixture();
    let after = check_record(&record, Some(&before))
        .unwrap()
        .expect("checked world");
    assert_eq!(
        json!(after),
        record.receipt.result["kernel_replay"]["after"]
    );
}

/// #783: corruption must be attributed to the first event, not skipped.
#[test]
fn corrupt_snapshot_names_event() {
    let (mut record, before) = fixture();
    record.receipt.result["kernel_replay"]["steps"][0]["after"]["pool_capacity"] = json!(2);
    let error = check_record(&record, Some(&before)).unwrap_err();
    assert!(error.contains("fixture:event-2"), "{error}");
    assert!(error.contains("snapshot"), "{error}");
}

/// #783 / Q27: runtime verification observes existing rules without changing them.
#[test]
fn invalid_event_result_names_invariant() {
    let (mut record, _) = fixture();
    record.receipt.result["kernel_replay"]["steps"][0]["input"]["pool_capacity"] = json!(0);
    let error = check_record(&record, None).unwrap_err();
    assert!(error.contains("fixture:event-2"), "{error}");
    assert!(error.contains("invariant"), "{error}");
}

/// #783: changing the baseline between event records must not hide divergence.
#[test]
fn disconnected_history_fails() {
    let (record, mut previous) = fixture();
    previous.pool_capacity = 2;
    assert!(
        check_record(&record, Some(&previous))
            .unwrap_err()
            .contains("fixture:event-2")
    );
}

/// #783: pre-instrumentation history lacks snapshots and must fail explicitly.
#[test]
fn legacy_kernel_event_is_not_a_pass() {
    let (mut record, _) = fixture();
    record.receipt.result = json!({"kernel_event": {"NewRequest":{"repo":0}}});
    assert!(
        check_record(&record, None)
            .unwrap_err()
            .contains("fixture:event-2")
    );
}

/// #783: measure the added invariant scan on real settled worlds, without a timing assertion.
#[test]
fn measure_invariant_cost_per_step() {
    use std::{hint::black_box, time::Instant};
    let mut world = World::new(&[1], 1);
    for count in [1, 16, 64, 256] {
        while world.nodes.len() < count {
            world = tl3_kernel::step(
                world,
                Event::NewRequest {
                    repo: RepoId(0),
                    priority: Priority::Unranked,
                },
            )
            .0;
        }
        let iterations = 200;
        let start = Instant::now();
        for _ in 0..iterations {
            tl3_controller::replay::check_world(black_box(&world)).unwrap();
        }
        println!(
            "invariant_cost nodes={count} iterations={iterations} ns_per_step={}",
            start.elapsed().as_nanos() / iterations
        );
    }
}
