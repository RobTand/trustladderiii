//! Compare the private incident-call extraction over the original handler domain.
use super::*;

#[path = "run_failed_reference.rs"]
mod reference;

fn compare_failed(world: World, i: usize, out: Outbox, prefix: Vec<ReportEntry>) {
    assert!(out.len < OUTBOX);
    assert!(world.nodes[i].generation <= u64::MAX - 2);
    assert!(world.nodes[i].run_failures <= 1);
    assert!(world.nodes[i].run_incidents <= 2);
    let mut actual = world.clone();
    let mut expected = world;
    let mut actual_out = out;
    let mut expected_out = out;
    let mut actual_report = Report { entries: prefix.clone() };
    let mut expected_report = Report { entries: prefix };
    reference::run_failed(&mut expected, i, &mut expected_out, &mut expected_report);
    crate::retries::run_failed(&mut actual, i, &mut actual_out, &mut actual_report);
    assert_eq!(actual, expected, "failed-run world differs");
    assert_eq!(actual_out, expected_out, "failed-run outbox differs");
    assert_eq!(actual_report, expected_report, "failed-run report differs");
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 128, failure_persistence: None, ..ProptestConfig::default() })]

    #[test]
    fn incident_call_matches_original(n in node(), other in node(), out in outbox(),
        selected in 0usize..3, next in any::<u64>(), ticket in any::<u64>(),
        capacity in any::<u8>(), fail_fast in any::<bool>(),
        files in prop::collection::vec(any::<u64>(), 0..5),
        prefix in prop::collection::vec((any::<u64>(), 0u8..3, any::<u8>()), 0..12)) {
        let prefix = prefix.into_iter().map(|(node, tag, cause)| {
            let node = NodeId(node);
            match tag {
                0 => ReportEntry::FailedRound { node, cause: [FailedRoundCause::ChecksFailed,
                    FailedRoundCause::ChangesRequested, FailedRoundCause::MakerCantMeet,
                    FailedRoundCause::CiFailed, FailedRoundCause::RunFailed][usize::from(cause % 5)] },
                1 => ReportEntry::Stopped { node, cause: StopCause::ReviewStop(ReviewStop::CiFailed) },
                _ => ReportEntry::Stale { node, reply_kind: ReplyKind::RunFailed },
            }
        }).collect::<Vec<_>>();
        // The shared reply generator includes invalid lengths. Normalize BEFORE either arm.
        let mut out = out;
        out.len %= OUTBOX;
        let phases = [n.phase, Phase::Ingesting, Phase::Auditing, Phase::Unplanned,
            Phase::Planning { must_split: false }, Phase::Ready(Resume::Making),
            Phase::Making, Phase::Checking, Phase::Reviewing, Phase::Publishing,
            Phase::Queued, Phase::Rebasing, Phase::AwaitingRob,
            Phase::Stopping(Outcome::Stopped), Phase::Closed(Outcome::Stopped)];
        for phase in phases {
            for failures in 0..=1 {
                for incidents in 0..=2 {
                    let mut n = n;
                    n.phase = phase;
                    n.generation = n.generation.min(u64::MAX - 2);
                    n.run_failures = failures;
                    n.run_incidents = incidents;
                    let mut world = World::new(&[1, 2, 3], capacity);
                    world.nodes = vec![other; 3];
                    world.nodes[selected] = n;
                    world.next_ticket = ticket;
                    world.next_question = next;
                    world.fail_fast = fail_fast;
                    world.file_sets = vec![files.iter().copied().map(FileId).collect()];
                    compare_failed(world, selected, out, prefix.clone());
                }
            }
        }
    }
}

#[test]
fn second_failure_emits_one_incident() {
    let mut world = World::new(&[1], 1);
    world.nodes[0].phase = Phase::Making;
    world.nodes[0].run_failures = 1;
    world.nodes[0].run_incidents = 0;
    // Two notices fit: duplication must fail equivalence, not overflow/headroom.
    compare_failed(world, 0, Outbox::new(), Vec::new());
}
