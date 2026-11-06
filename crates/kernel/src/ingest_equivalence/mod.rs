//! #806: compare the real dispatcher to its verbatim body before private extraction.
use super::*;
use proptest::prelude::*;

mod reference;

fn option<T: Copy>(word: u64, value: T) -> Option<T> {
    (word & 1 != 0).then_some(value)
}

fn phase(word: u64) -> Phase {
    let phases = [
        Phase::Ingesting, Phase::Auditing, Phase::Unplanned,
        Phase::Planning { must_split: false }, Phase::Planning { must_split: true },
        Phase::Ready(Resume::Making), Phase::Ready(Resume::Rebasing), Phase::Making,
        Phase::Checking, Phase::Reviewing, Phase::Publishing, Phase::Queued,
        Phase::Rebasing, Phase::AwaitingRob, Phase::Stopping(Outcome::Stopped),
        Phase::Closed(Outcome::Merged),
    ];
    phases[word as usize % phases.len()]
}

fn node(x: &[u64; 64]) -> Node {
    let effects = [EffectKind::StartRun(Role::Ingest), EffectKind::StartRun(Role::Audit),
        EffectKind::StartRun(Role::Plan), EffectKind::StartRun(Role::Make),
        EffectKind::StartRun(Role::Review), EffectKind::SubmitChecks, EffectKind::StartRebase,
        EffectKind::PrepareWorktree, EffectKind::Push, EffectKind::PostApproval,
        EffectKind::OpenPr, EffectKind::Enqueue, EffectKind::StopRun, EffectKind::WithdrawJob,
        EffectKind::ClosePr, EffectKind::RemoveWorktree];
    let early = [EarlyEnd::Duplicate, EarlyEnd::AlreadyDone, EarlyEnd::NotActionable];
    let stops = [ReviewStop::ChecksFailed, ReviewStop::ChangesRequested, ReviewStop::CiFailed,
        ReviewStop::MakerCantMeet, ReviewStop::MakerNeedsFiles, ReviewStop::MakerOutOfRoom,
        ReviewStop::ReviewerOutOfRoom, ReviewStop::PlanCantMeet, ReviewStop::PublicationBody,
        ReviewStop::AuditDisagreement];
    Node {
        phase: phase(x[0]), pos: x[1] as u8, repo: RepoId(x[2]),
        footprint: Footprint::inline(x[3]), generation: x[4].min(u64::MAX - 1),
        next_seq: x[5], effect: option(x[6], InFlight { seq: x[7],
            kind: effects[x[8] as usize % effects.len()], cancelling: x[9] & 1 != 0 }),
        run: option(x[10], x[11]), job: option(x[12], x[13]), pool: x[14] & 1 != 0,
        ticket: option(x[15], x[16]), holds: x[17] & 1 != 0, paused: x[18] & 1 != 0,
        last_cmd: x[19], question: option(x[20], Question { id: x[21],
            kind: QKind::EffectFailed, cont: Cont::Planning { must_split: x[22] & 1 != 0 },
            cost: x[23] as u8, clarified: x[24] & 1 != 0 }),
        attempt: x[25] as u8, round: x[26] as u8, replanned: x[27] & 1 != 0,
        plan_retries: x[28] as u8, audit_rounds: x[29] as u8,
        early_end: option(x[30], early[x[31] as usize % early.len()]),
        run_failures: x[32] as u8, run_incidents: x[33] as u8,
        fx_failures: x[34] as u8, fx_incidents: x[35] as u8,
        waiting_incident: x[36] & 1 != 0, incident_needs_rob: x[37] & 1 != 0,
        proposed: option(x[38], HeadId(x[39])), adopted: x[40] & 1 != 0,
        head: option(x[41], HeadId(x[42])), checked: option(x[43], HeadId(x[44])),
        approved: option(x[45], HeadId(x[46])), merged: option(x[47], HeadId(x[48])),
        pr_open: x[49] & 1 != 0, had_pr: x[50] & 1 != 0, worktree: x[51] & 1 != 0,
        rechecks: x[52] as u8, review_restarted: x[53] & 1 != 0,
        credit: Credit { ready: x[54] & 1 != 0, start: x[55], earned: x[56],
            added: x[57], spent: x[58], merge_credited: x[59] & 1 != 0,
            withheld: x[60] & 1 != 0 },
        priority: [Priority::P0, Priority::P1, Priority::P2, Priority::P3,
            Priority::Unranked][x[61] as usize % 5],
        review_stop: option(x[62], stops[x[63] as usize % stops.len()]),
        planning_started: x[0] & 2 != 0, waiting_prerequisite: option(x[1], x[2]),
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 256, ..ProptestConfig::default() })]
    #[test]
    fn dispatcher_matches_verbatim_reference(
        words in prop::collection::vec(any::<[u64; 64]>(), 1..5),
        meta in any::<[u64; 16]>(), size in any::<u8>(),
    ) {
        let i = meta[0] as usize % words.len();
        let mut before = World::new(&[meta[1] as u8, meta[2] as u8], meta[3] as u8);
        before.nodes = words.iter().map(node).collect();
        before.next_ticket = meta[4];
        before.next_question = meta[5].min(u64::MAX - 1);
        before.fail_fast = meta[6] & 1 != 0;
        before.file_sets = vec![vec![FileId(meta[7]), FileId(meta[8])], vec![]];
        before.dependencies = words.iter().enumerate().map(|(j, x)| Dependency {
            node: NodeId((j % words.len()) as u64), target: DependencyTarget {
                kind: if x[0] & 1 == 0 { PrerequisiteKind::Issue } else { PrerequisiteKind::PullRequest },
                number: x[1], request: option(x[2], NodeId(x[3])),
            }, status: [DependencyStatus::Pending, DependencyStatus::Delivered,
                DependencyStatus::Abandoned][x[4] as usize % 3], rejected: x[5] & 1 != 0,
        }).collect();
        let mut initial_out = Outbox { items: [None; OUTBOX], len: meta[9] as usize % OUTBOX };
        for (k, slot) in initial_out.items.iter_mut().enumerate() {
            *slot = match meta[10 + k] % 3 {
                0 => None,
                1 => Some(Out::Notice { node: NodeId(meta[k]), notice: Notice::Ask { question: meta[15] } }),
                _ => Some(Out::Effect { node: NodeId(meta[k]), seq: meta[15], kind: EffectKind::Push,
                    head: option(meta[k], HeadId(meta[15])) }),
            };
        }
        // Each generated world executes both audit branches, all early outcomes,
        // saturation, every dispatch arm, and the no-op arm. Other fields stay arbitrary.
        for early in [None, Some(EarlyEnd::Duplicate), Some(EarlyEnd::AlreadyDone), Some(EarlyEnd::NotActionable)] {
            for rounds in [0, 1, 2, u8::MAX, before.nodes[i].audit_rounds] {
                before.nodes[i].early_end = early;
                before.nodes[i].audit_rounds = rounds;
                for reply in [Reply::IngestDone { size }, Reply::IngestEarlyEnd(EarlyEnd::Duplicate),
                    Reply::IngestEarlyEnd(EarlyEnd::AlreadyDone), Reply::IngestEarlyEnd(EarlyEnd::NotActionable),
                    Reply::AuditAgrees, Reply::AuditDisagrees, Reply::ProtectedPaths, Reply::PlanSplit] {
                    let mut actual = before.clone();
                    let mut expected = before.clone();
                    let mut out_actual = initial_out;
                    let mut out_expected = initial_out;
                    reply_ingest(&mut actual, i, reply, &mut out_actual);
                    reference::reference(&mut expected, i, reply, &mut out_expected);
                    prop_assert_eq!(actual, expected, "reply={:?} early={:?} rounds={}", reply, early, rounds);
                    prop_assert_eq!(out_actual, out_expected, "reply={:?} early={:?} rounds={}", reply, early, rounds);
                }
            }
        }
    }
}
