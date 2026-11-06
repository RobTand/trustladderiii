//! Q39 / #806: public pool keys and the derived Rust priority order.
use proptest::prelude::*;
use tl3_kernel::*;

const PRIORITIES: [Priority; 5] = [
    Priority::P0,
    Priority::P1,
    Priority::P2,
    Priority::P3,
    Priority::Unranked,
];
const RESUMES: [Resume; 5] = [
    Resume::Making,
    Resume::Checking,
    Resume::Reviewing,
    Resume::Publishing,
    Resume::Rebasing,
];
const OUTCOMES: [Outcome; 6] = [
    Outcome::Merged,
    Outcome::Stopped,
    Outcome::Replaced,
    Outcome::Duplicate,
    Outcome::AlreadyDone,
    Outcome::NotActionable,
];

fn arbitrary_node() -> impl Strategy<Value = Node> {
    let mut phases = vec![
        Phase::Ingesting,
        Phase::Auditing,
        Phase::Unplanned,
        Phase::Planning { must_split: false },
        Phase::Planning { must_split: true },
        Phase::Making,
        Phase::Checking,
        Phase::Reviewing,
        Phase::Publishing,
        Phase::Queued,
        Phase::Rebasing,
        Phase::AwaitingRob,
    ];
    phases.extend(RESUMES.map(Phase::Ready));
    phases.extend(OUTCOMES.map(Phase::Stopping));
    phases.extend(OUTCOMES.map(Phase::Closed));
    let mut effects = vec![
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
    effects.extend(
        [
            Role::Ingest,
            Role::Audit,
            Role::Plan,
            Role::Make,
            Role::Review,
        ]
        .map(EffectKind::StartRun),
    );
    let mut continuations = vec![
        Cont::Ingesting,
        Cont::Auditing,
        Cont::Unplanned,
        Cont::Planning { must_split: false },
        Cont::Planning { must_split: true },
        Cont::Nowhere,
    ];
    continuations.extend(RESUMES.map(Cont::Ready));
    continuations.extend(OUTCOMES.map(Cont::Stop));
    (
        any::<[u64; 19]>(),
        any::<[u8; 11]>(),
        any::<[bool; 32]>(),
        prop::sample::select(phases),
        prop::sample::select(PRIORITIES.to_vec()),
        prop::sample::select(effects),
        prop::sample::select(continuations),
        prop::sample::select(vec![
            QKind::AuditDisagreement,
            QKind::NeedsSplitting,
            QKind::CantMeet,
            QKind::OutOfCredit,
            QKind::EffectFailed,
            QKind::CleanupFailed,
        ]),
        prop::sample::select(vec![
            ReviewStop::ChecksFailed,
            ReviewStop::ChangesRequested,
            ReviewStop::CiFailed,
            ReviewStop::MakerCantMeet,
            ReviewStop::MakerNeedsFiles,
            ReviewStop::MakerOutOfRoom,
            ReviewStop::ReviewerOutOfRoom,
            ReviewStop::PlanCantMeet,
            ReviewStop::PublicationBody,
        ]),
        prop::sample::select(vec![
            EarlyEnd::Duplicate,
            EarlyEnd::AlreadyDone,
            EarlyEnd::NotActionable,
        ]),
    )
        .prop_map(
            |(
                u,
                bytes,
                bits,
                phase,
                priority,
                kind,
                cont,
                question_kind,
                review_stop,
                early_end,
            )| {
                let mut world = World::new(&[], 0);
                let mut files: Vec<_> = (0..64)
                    .filter(|i| u[2] & (1 << i) != 0)
                    .map(FileId)
                    .collect();
                if bits[30] {
                    files.push(FileId(u[2] | 64));
                }
                let footprint = world.intern_footprint(files);
                Node {
                    phase,
                    priority,
                    ticket: bits[0].then_some(u[0]),
                    pos: bytes[0],
                    repo: RepoId(u[1]),
                    footprint,
                    generation: u[3],
                    next_seq: u[4],
                    effect: bits[1].then_some(InFlight {
                        seq: u[5],
                        kind,
                        cancelling: bits[2],
                    }),
                    run: bits[3].then_some(u[6]),
                    job: bits[4].then_some(u[7]),
                    pool: bits[5],
                    holds: bits[6],
                    paused: bits[7],
                    last_cmd: u[8],
                    question: bits[8].then_some(Question {
                        id: u[9],
                        kind: question_kind,
                        cont,
                        cost: bytes[1],
                        clarified: bits[9],
                    }),
                    attempt: bytes[2],
                    round: bytes[3],
                    replanned: bits[10],
                    plan_retries: bytes[4],
                    audit_rounds: bytes[5],
                    early_end: bits[11].then_some(early_end),
                    run_failures: bytes[6],
                    run_incidents: bytes[7],
                    fx_failures: bytes[8],
                    fx_incidents: bytes[9],
                    waiting_incident: bits[12],
                    incident_needs_rob: bits[13],
                    proposed: bits[14].then_some(HeadId(u[10])),
                    adopted: bits[15],
                    head: bits[16].then_some(HeadId(u[11])),
                    checked: bits[17].then_some(HeadId(u[12])),
                    approved: bits[18].then_some(HeadId(u[13])),
                    merged: bits[19].then_some(HeadId(u[14])),
                    pr_open: bits[20],
                    had_pr: bits[21],
                    worktree: bits[22],
                    rechecks: bytes[10],
                    review_restarted: bits[23],
                    credit: Credit {
                        start: u[15],
                        earned: u[16],
                        added: u[17],
                        spent: u[18],
                        ready: bits[24],
                        merge_credited: bits[25],
                        withheld: bits[26],
                    },
                    review_stop: bits[27].then_some(review_stop),
                    planning_started: bits[28],
                    waiting_prerequisite: bits[29].then_some(u[9]),
                }
            },
        )
}

// The pre-port body, kept verbatim as the equivalence reference.
fn old_pool_key(n: &Node) -> Option<(Priority, u64)> {
    let ticket = n.ticket.filter(|_| !matches!(n.phase, Phase::Ready(_)))?;
    Some((n.priority, ticket))
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(1024))]
    #[test]
    fn pool_key_matches_the_old_filter_body(node in arbitrary_node()) {
        prop_assert_eq!(pool_key(&node), old_pool_key(&node));
    }
}

// Independent state-table levels. Do not use Priority::rank or enum casts.
fn spec_key(priority: Priority, ticket: u64) -> (u8, u64) {
    let level = match priority {
        Priority::P0 => 0,
        Priority::P1 => 1,
        Priority::P2 => 2,
        Priority::P3 => 3,
        Priority::Unranked => 4,
    };
    (level, ticket)
}

#[test]
fn derived_priority_order_matches_the_state_table_exhaustively() {
    // u64 comparison depends only on <, = or >. Exercise each relation at
    // both boundaries as well as next to u64::MAX, with all 25 enum pairs.
    let tickets = [
        (0, 0),
        (0, 1),
        (1, 0),
        (1, 1),
        (0, u64::MAX),
        (u64::MAX, 0),
        (u64::MAX, u64::MAX),
        (u64::MAX - 1, u64::MAX),
        (u64::MAX, u64::MAX - 1),
    ];
    let mut overtakes_older = 0;
    let mut tied_priority = 0;
    for pa in PRIORITIES {
        for pb in PRIORITIES {
            for (ta, tb) in tickets {
                let expected = spec_key(pa, ta) < spec_key(pb, tb);
                assert_eq!((pa, ta) < (pb, tb), expected, "{pa:?}/{ta} vs {pb:?}/{tb}");
                let mut a = Node::new(RepoId(0));
                let mut b = Node::new(RepoId(0));
                a.phase = Phase::Ingesting;
                b.phase = Phase::Ingesting;
                a.priority = pa;
                b.priority = pb;
                a.ticket = Some(ta);
                b.ticket = Some(tb);
                assert_eq!(pool_key(&a).unwrap() < pool_key(&b).unwrap(), expected);
                overtakes_older +=
                    usize::from(expected && spec_key(pa, 0).0 < spec_key(pb, 0).0 && ta > tb);
                tied_priority += usize::from(expected && pa == pb);
            }
        }
    }
    assert!(overtakes_older > 0, "original priority-overtaking cover");
    assert!(tied_priority > 0, "original tied-priority cover");
}
