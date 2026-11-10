//! Differential check (lead condition, 11-10): the reordered answer_command (on_answer, then
//! record_last_cmd) against the pre-reorder body, and on_command (the public command path) against
//! the pre-reorder dispatch, on generated worlds: refused answers (no question, wrong id, not
//! allowed, wrong phase), repeated and stale ids, Unclear, every answer kind, and Continue/AddCredit
//! with and without the N4-W withhold guard (dependency rows pending, delivered, abandoned, rejected).
//! Both sides run under catch_unwind; results (world, outbox) or panics must be equal.
use super::*;
use proptest::prelude::*;
use std::panic::{catch_unwind, AssertUnwindSafe};

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


const ANSWERS: [Answer; 9] = [
    Answer::Continue,
    Answer::TryAgain,
    Answer::AddCredit(0),
    Answer::AddCredit(3),
    Answer::ChangeRequest,
    Answer::Stop,
    Answer::Unclear,
    Answer::Retry,
    Answer::MarkHandled,
];
const STATUSES: [DependencyStatus; 3] =
    [DependencyStatus::Pending, DependencyStatus::Delivered, DependencyStatus::Abandoned];

/// A two-node world around an arbitrary node 0, biased to the Answer paths: AwaitingRob or Stopping,
/// an open question with a small id, small last_cmd, and a dependency table for node 0.
fn answer_world() -> impl Strategy<Value = World> {
    (arbitrary_node(), arbitrary_node(), any::<[u8; 12]>(), any::<[bool; 8]>()).prop_map(|(a, other, b, f)| {
        let mut n = a;
        n.phase = match b[0] % 4 {
            0 | 1 => Phase::AwaitingRob,
            2 => Phase::Stopping(OUTCOMES[b[1] as usize % 6]),
            _ => n.phase,
        };
        n.last_cmd = (b[2] % 4) as u64;
        if let Some(q) = n.question.as_mut() {
            q.id = (b[3] % 4) as u64;
        }
        if f[0] {
            n.question = None;
        }
        // Small counters on half the cases so the answer's arithmetic stays in range.
        if f[1] {
            n.generation %= 1 << 20;
            n.next_seq %= 1 << 20;
            n.credit.start %= 1 << 20;
            n.credit.earned %= 1 << 20;
            n.credit.added %= 1 << 20;
            n.credit.spent %= 1 << 20;
            n.attempt %= 8;
            n.round %= 8;
            n.plan_retries %= 8;
            n.audit_rounds %= 8;
        }
        let mut w = World::new(&[3, 3], 2);
        w.nodes.push(n);
        w.nodes.push(other);
        w.next_question = match b[4] % 4 {
            0 => (b[5] % 6) as u64,
            1 => u64::MAX - 1,
            2 => u64::MAX,
            _ => 1 << 30,
        };
        let rows = (b[6] % 4) as usize;
        for r in 0..rows {
            w.dependencies.push(Dependency {
                node: NodeId(if f[2 + r % 2] { 0 } else { 1 }),
                target: DependencyTarget { kind: PrerequisiteKind::Issue, number: r as u64, request: None },
                status: STATUSES[b[7 + r] as usize % 3],
                rejected: f[4 + r],
            });
        }
        w
    })
}

type Outcome2 = Result<(World, Outbox), ()>;

fn run(f: impl FnOnce(&mut World, &mut Outbox), w: &World, out: &Outbox) -> Outcome2 {
    let (mut w, mut out) = (w.clone(), *out);
    catch_unwind(AssertUnwindSafe(move || {
        f(&mut w, &mut out);
        (w, out)
    }))
    .map_err(|_| ())
}

fn compare(w: World, id: u64, question: u64, answer: Answer) {
    let out = Outbox::new();
    if id > w.nodes[0].last_cmd {
        let new = run(|w, o| crate::commands::answer_command(w, 0, id, question, answer, o), &w, &out);
        let old = run(|w, o| reference::answer_command(w, 0, id, question, answer, o), &w, &out);
        assert_eq!(new, old, "answer_command differs: id {id} q {question} {answer:?}\n{w:#?}");
    }
    let command = Command::Answer { question, answer };
    let new = run(|w, o| crate::commands::on_command(w, 0, id, command, o), &w, &out);
    let old = run(|w, o| reference::on_command(w, 0, id, command, o), &w, &out);
    assert_eq!(new, old, "on_command differs: id {id} q {question} {answer:?}\n{w:#?}");
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 4096, failure_persistence: None, ..ProptestConfig::default() })]
    #[test]
    fn reordered_answer_matches_the_pre_reorder_body(w in answer_world(), id in 0u64..6, question in 0u64..5) {
        for answer in ANSWERS {
            compare(w.clone(), id, question, answer);
        }
    }
}

/// Deterministic sweep of the withhold guard: Continue / AddCredit on an AwaitingRob node whose open
/// question is answered, for every question kind, ready flag, planning flag, early phase continuation
/// and dependency row state (none, pending, delivered, abandoned, rejected).
#[test]
fn withhold_guard_paths_match() {
    let base = arbitrary_node_fixed();
    let kinds = [QKind::AuditDisagreement, QKind::NeedsSplitting, QKind::CantMeet,
        QKind::OutOfCredit, QKind::EffectFailed, QKind::CleanupFailed];
    let conts = [Cont::Ingesting, Cont::Auditing, Cont::Unplanned,
        Cont::Planning { must_split: false }, Cont::Ready(RESUMES[0]), Cont::Stop(Outcome::Stopped), Cont::Nowhere];
    let mut cases = 0u32;
    for kind in kinds {
        for cont in conts {
            for ready in [false, true] {
                for started in [false, true] {
                    for rows in 0..5 {
                        for answer in [Answer::Continue, Answer::AddCredit(0), Answer::AddCredit(2)] {
                            for cost in [0u8, 1] {
                                let mut n = base;
                                n.phase = Phase::AwaitingRob;
                                n.last_cmd = 0;
                                n.planning_started = started;
                                n.credit.ready = ready;
                                n.credit.withheld = false;
                                n.question = Some(Question { id: 1, kind, cont, cost, clarified: false });
                                let mut w = World::new(&[3], 2);
                                w.nodes.push(n);
                                w.next_question = 5;
                                if rows > 0 {
                                    w.dependencies.push(Dependency {
                                        node: NodeId(0),
                                        target: DependencyTarget { kind: PrerequisiteKind::Issue, number: 7, request: None },
                                        status: STATUSES[(rows - 1) % 3],
                                        rejected: rows == 4,
                                    });
                                }
                                compare(w.clone(), 1, 1, answer);
                                compare(w.clone(), 1, 2, answer); // wrong question id
                                compare(w, 0, 1, answer); // repeated id
                                cases += 3;
                            }
                        }
                    }
                }
            }
        }
    }
    assert!(cases > 10_000, "{cases}");
}

fn arbitrary_node_fixed() -> Node {
    Node::new(RepoId(0))
}
