//! #806 Ack/adoption rows: arbitrary-input equivalence for the standing-rule pure moves.
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
        Phase::Queued,
        Phase::Rebasing,
        Phase::AwaitingRob,
        Phase::Stopping(outcome),
        Phase::Closed(outcome),
        Phase::Stopping(Outcome::Stopped),
    ]
}

// Independent words, bytes and presence bits cover every public field without
// consistency assumptions. Both implementations see the same arbitrary input.
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

fn initial_out(node: Node, key: u64) -> Outbox {
    let mut out = Outbox::new();
    out.len = key as usize % (OUTBOX + 1);
    for slot in 0..OUTBOX {
        out.items[slot] = if key & (1 << slot) == 0 {
            None
        } else if node.pool {
            Some(Out::Effect {
                node: NodeId(key),
                seq: node.next_seq,
                kind: EFFECTS[slot],
                head: node.head,
            })
        } else {
            Some(Out::Notice {
                node: NodeId(key),
                notice: Notice::Ask {
                    question: node.last_cmd,
                },
            })
        };
    }
    out
}

fn compare_ack(node: Node, seq: u64, ack: Ack) {
    let make = || World {
        nodes: vec![node, Node::new(RepoId(1))],
        gates: vec![Gate { w: 1 }, Gate { w: 1 }],
        file_sets: Vec::new(),
        dependencies: Vec::new(),
        pool_capacity: 1,
        next_ticket: seq,
        next_question: seq,
        fail_fast: seq % 2 == 0,
    };
    let mut actual = make();
    let mut expected = make();
    let mut actual_out = initial_out(node, seq);
    let mut expected_out = actual_out;
    let a = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        on_ack(&mut actual, 0, seq, ack, &mut actual_out)
    }))
    .is_ok();
    let b = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        reference::on_ack(&mut expected, 0, seq, ack, &mut expected_out)
    }))
    .is_ok();
    assert_eq!(a, b);
    assert_eq!(actual, expected);
    assert_eq!(actual_out, expected_out);
}

fn compare_dispatch(node: Node, seq: u64, ack: Ack, target: NodeId) {
    let make = || World {
        nodes: vec![node, Node::new(RepoId(1))],
        gates: vec![Gate { w: 1 }, Gate { w: 1 }],
        file_sets: Vec::new(),
        dependencies: Vec::new(),
        pool_capacity: 1,
        next_ticket: seq,
        next_question: seq,
        fail_fast: seq % 2 == 0,
    };
    let mut actual = make();
    let mut expected = make();
    let mut actual_out = initial_out(node, seq);
    let mut expected_out = actual_out;
    let a = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        apply(
            &mut actual,
            Event::Ack {
                node: target,
                seq,
                ack,
            },
            &mut actual_out,
        )
    }))
    .is_ok();
    let b = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        reference::apply_ack_event(&mut expected, target, seq, ack, &mut expected_out)
    }))
    .is_ok();
    assert_eq!(a, b);
    assert_eq!(actual, expected);
    assert_eq!(actual_out, expected_out);
}

fn compare_agenda(node: Node, kind: EffectKind) {
    let mut actual = node;
    let mut expected = node;
    let a = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        confirmed_agenda(&mut actual, kind)
    }))
    .is_ok();
    let b = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        reference::confirmed_agenda(&mut expected, kind)
    }))
    .is_ok();
    assert_eq!(a, b);
    assert_eq!(actual, expected);
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 256, failure_persistence: None, ..ProptestConfig::default() })]
    /// #806 Ack rows: arbitrary node fields, every ack, matching/stale/cancelled keys and no effect.
    #[test]
    fn ack_match_extraction_preserves_every_write(node in arbitrary_node(), key in any::<u64>()) {
        for ack in [Ack::Confirmed, Ack::Retryable, Ack::Permanent, Ack::BodyPolicyRefused] {
            compare_ack(node, key, ack);
            for target in [NodeId(0), NodeId(1), NodeId(2), NodeId(u64::MAX)] {
                compare_dispatch(node, key, ack, target);
                compare_dispatch(Node { effect: Some(InFlight { seq: key, kind: EffectKind::OpenPr, cancelling: false }), ..node }, key, ack, target);
            }
            compare_ack(Node { effect: None, ..node }, key, ack);
            for kind in EFFECTS {
                for cancelling in [false, true] {
                    let n = Node { effect: Some(InFlight { seq: key, kind, cancelling }), ..node };
                    compare_ack(n, key, ack);
                    compare_ack(n, key.wrapping_add(1), ack);
                }
            }
        }
    }
    /// #806 adoption row: all agendas, positions and effect kinds include both adoption branches.
    #[test]
    fn adoption_extraction_preserves_every_write(node in arbitrary_node()) {
        for phase in phases(Outcome::Stopped) {
            for pos in [0, 1, 2, 3, 4, 5, u8::MAX] {
                for kind in EFFECTS { compare_agenda(Node { phase, pos, ..node }, kind); }
            }
        }
        for adopted in [false, true] {
            compare_agenda(Node { phase: Phase::Making, pos: 0, proposed: Some(HeadId(7)), adopted, ..node }, EffectKind::PrepareWorktree);
        }
    }
}

/// #806 arbitrary-input equivalence also covers a reused key outside checker-consistent worlds.
#[test]
fn ack_dispatch_preserves_reused_delivery_key() {
    let node = Node {
        phase: Phase::Publishing,
        pos: 0,
        next_seq: 0,
        effect: Some(InFlight {
            seq: 0,
            kind: EffectKind::Push,
            cancelling: false,
        }),
        ..Node::new(RepoId(0))
    };
    compare_dispatch(node, 0, Ack::Retryable, NodeId(0));
}
