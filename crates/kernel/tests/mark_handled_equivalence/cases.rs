//! Compare teardown handling with its pre-extraction body.
use super::*;
use proptest::prelude::*;

#[path = "reference.rs"]
mod reference;

const OUTCOMES: [Outcome; 6] = [
    Outcome::Merged,
    Outcome::Stopped,
    Outcome::Replaced,
    Outcome::Duplicate,
    Outcome::AlreadyDone,
    Outcome::NotActionable,
];
const RESUMES: [Resume; 5] = [
    Resume::Making,
    Resume::Checking,
    Resume::Reviewing,
    Resume::Publishing,
    Resume::Rebasing,
];
const EFFECTS: [EffectKind; 16] = [
    EffectKind::StartRun(Role::Ingest),
    EffectKind::StartRun(Role::Audit),
    EffectKind::StartRun(Role::Plan),
    EffectKind::StartRun(Role::Make),
    EffectKind::StartRun(Role::Review),
    EffectKind::SubmitChecks,
    EffectKind::StartRebase,
    EffectKind::PrepareWorktree,
    EffectKind::Push,
    EffectKind::PostApproval,
    EffectKind::OpenPr,
    EffectKind::Enqueue,
    EffectKind::StopRun,
    EffectKind::WithdrawJob,
    EffectKind::ClosePr,
    EffectKind::RemoveWorktree,
];

fn phases(outcome: Outcome) -> [Phase; 20] {
    [
        Phase::Ingesting,
        Phase::Auditing,
        Phase::Unplanned,
        Phase::Planning { must_split: false },
        Phase::Planning { must_split: true },
        Phase::Ready(RESUMES[0]),
        Phase::Ready(RESUMES[1]),
        Phase::Ready(RESUMES[2]),
        Phase::Ready(RESUMES[3]),
        Phase::Ready(RESUMES[4]),
        Phase::Making,
        Phase::Checking,
        Phase::Reviewing,
        Phase::Publishing,
        Phase::AwaitingDelivery,
        Phase::Rebasing,
        Phase::AwaitingRob,
        Phase::Stopping(outcome),
        Phase::Closed(outcome),
        Phase::Stopping(Outcome::Stopped),
    ]
}

// Independent words, bytes and presence bits cover every public field without
// consistency assumptions. MarkHandled has no input precondition.
fn arbitrary_node() -> impl Strategy<Value = Node> {
    (any::<[u64; 24]>(), any::<[u8; 16]>(), any::<[bool; 40]>()).prop_map(|(v, b, f)| {
        let option = |index: usize| f[index].then_some(v[index]);
        let outcome = OUTCOMES[b[1] as usize % OUTCOMES.len()];
        let mut w = World::new(&[], 0);
        let footprint = if f[39] {
            Footprint::inline(v[1])
        } else {
            w.intern_footprint([FileId(v[1]), FileId(u64::MAX)])
        };
        Node {
            phase: phases(outcome)[b[0] as usize % 20],
            pos: b[2],
            repo: RepoId(v[0]),
            footprint,
            generation: v[2],
            next_seq: v[3],
            effect: f[24].then_some(InFlight {
                seq: v[4],
                kind: EFFECTS[b[3] as usize % 16],
                cancelling: f[25],
            }),
            run: option(5),
            job: option(6),
            pool: f[26],
            ticket: option(7),
            holds: f[27],
            paused: f[28],
            last_cmd: v[8],
            question: f[29].then_some(Question {
                id: v[9],
                kind: [
                    QKind::AuditDisagreement,
                    QKind::NeedsSplitting,
                    QKind::CantMeet,
                    QKind::OutOfCredit,
                    QKind::EffectFailed,
                    QKind::CleanupFailed,
                ][b[4] as usize % 6],
                cont: [
                    Cont::Ingesting,
                    Cont::Auditing,
                    Cont::Unplanned,
                    Cont::Planning { must_split: f[30] },
                    Cont::Ready(RESUMES[b[5] as usize % 5]),
                    Cont::Stop(outcome),
                    Cont::Nowhere,
                ][b[6] as usize % 7],
                cost: b[7],
                clarified: f[31],
            }),
            attempt: b[8],
            round: b[9],
            replanned: f[32],
            plan_retries: b[10],
            audit_rounds: b[11],
            early_end: f[33].then_some(
                [
                    EarlyEnd::Duplicate,
                    EarlyEnd::AlreadyDone,
                    EarlyEnd::NotActionable,
                ][b[12] as usize % 3],
            ),
            run_failures: b[13],
            run_incidents: b[14],
            fx_failures: b[15],
            fx_incidents: v[10] as u8,
            waiting_incident: f[34],
            incident_needs_rob: f[35],
            proposed: option(11).map(HeadId),
            adopted: f[36],
            head: option(12).map(HeadId),
            checked: option(13).map(HeadId),
            approved: option(14).map(HeadId),
            merged: option(15).map(HeadId),
            pr_open: f[0],
            had_pr: f[1],
            worktree: f[2],
            rechecks: v[16] as u8,
            review_restarted: f[3],
            credit: Credit {
                ready: f[4],
                start: v[17],
                earned: v[18],
                added: v[19],
                spent: v[20],
                merge_credited: f[16],
                withheld: f[17],
            },
            priority: [
                Priority::P0,
                Priority::P1,
                Priority::P2,
                Priority::P3,
                Priority::Unranked,
            ][v[21] as usize % 5],
            review_stop: f[18].then_some(
                [
                    ReviewStop::ChecksFailed,
                    ReviewStop::ChangesRequested,
                    ReviewStop::CiFailed,
                    ReviewStop::MakerCantMeet,
                    ReviewStop::MakerNeedsFiles,
                    ReviewStop::MakerOutOfRoom,
                    ReviewStop::ReviewerOutOfRoom,
                    ReviewStop::PlanCantMeet,
                    ReviewStop::PublicationBody,
                ][v[22] as usize % 9],
            ),
            planning_started: f[19],
            waiting_prerequisite: option(23),
        }
    })
}

fn compare(node: Node) {
    let mut actual = node;
    let mut expected = node;
    command_helpers::mark_handled(&mut actual);
    reference::mark_handled(&mut expected);
    assert_eq!(actual, expected, "before: {node:#?}");
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 512, failure_persistence: None, ..ProptestConfig::default() })]
    /// Arbitrary nodes and all u8 agenda positions are admitted.
    #[test]
    fn arbitrary_nodes_match_the_pre_extraction_body(node in arbitrary_node()) {
        compare(node);
        // Each generated resource state exercises all four Teardown variants,
        // every non-Teardown agenda entry, and None, including pos == u8::MAX.
        for phase in phases(OUTCOMES[node.pos as usize % 6]) {
            for pos in [0, 1, 2, 3, 4, 5, u8::MAX] {
                compare(Node { phase, pos, ..node });
            }
        }
    }
}

/// Compare every agenda branch to catch missing or duplicated resource writes.
#[test]
fn every_current_item_branch_is_compared() {
    let mut node = Node::new(RepoId(0));
    node.run = Some(7);
    node.job = Some(9);
    node.pool = true;
    node.pr_open = true;
    node.worktree = true;
    let mut teardown = [false; 4];
    let mut other = false;
    let mut none = false;
    for phase in phases(Outcome::Stopped) {
        for pos in 0..=u8::MAX {
            let input = Node { phase, pos, ..node };
            match input.current() {
                Some(Item::Teardown(kind)) => {
                    teardown[match kind {
                        EffectKind::StopRun => 0,
                        EffectKind::WithdrawJob => 1,
                        EffectKind::ClosePr => 2,
                        EffectKind::RemoveWorktree => 3,
                        _ => panic!("unexpected cleanup effect"),
                    }] = true;
                }
                Some(_) => other = true,
                None => none = true,
            }
            compare(input);
        }
    }
    assert_eq!(teardown, [true; 4]);
    assert!(other && none);
}
