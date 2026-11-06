//! #860: compare admitted reply extraction with the verbatim original body.
use crate::*;
use proptest::prelude::*;
use std::panic::{AssertUnwindSafe, catch_unwind};

mod reference;

fn outcome() -> impl Strategy<Value = Outcome> {
    prop::sample::select(vec![
        Outcome::Merged,
        Outcome::Stopped,
        Outcome::Replaced,
        Outcome::Duplicate,
        Outcome::AlreadyDone,
        Outcome::NotActionable,
    ])
}

fn resume(tag: u8) -> Resume {
    [
        Resume::Making,
        Resume::Checking,
        Resume::Reviewing,
        Resume::Publishing,
        Resume::Rebasing,
    ][usize::from(tag % 5)]
}

fn effect_kind(tag: u8, role: u8) -> EffectKind {
    match tag % 12 {
        0 => EffectKind::StartRun(
            [
                Role::Ingest,
                Role::Audit,
                Role::Plan,
                Role::Make,
                Role::Review,
            ][usize::from(role % 5)],
        ),
        1 => EffectKind::SubmitChecks,
        2 => EffectKind::StartRebase,
        3 => EffectKind::PrepareWorktree,
        4 => EffectKind::Push,
        5 => EffectKind::PostApproval,
        6 => EffectKind::OpenPr,
        7 => EffectKind::Enqueue,
        8 => EffectKind::StopRun,
        9 => EffectKind::WithdrawJob,
        10 => EffectKind::ClosePr,
        _ => EffectKind::RemoveWorktree,
    }
}

fn phase() -> impl Strategy<Value = Phase> {
    (0u8..14, any::<bool>(), outcome(), any::<u8>()).prop_map(|(tag, split, o, r)| match tag {
        0 => Phase::Ingesting,
        1 => Phase::Auditing,
        2 => Phase::Unplanned,
        3 => Phase::Planning { must_split: split },
        4 => Phase::Ready(resume(r)),
        5 => Phase::Making,
        6 => Phase::Checking,
        7 => Phase::Reviewing,
        8 => Phase::Publishing,
        9 => Phase::Queued,
        10 => Phase::Rebasing,
        11 => Phase::AwaitingRob,
        12 => Phase::Stopping(o),
        _ => Phase::Closed(o),
    })
}

fn question() -> impl Strategy<Value = Question> {
    (
        any::<u64>(),
        0u8..6,
        0u8..7,
        any::<u8>(),
        any::<bool>(),
        outcome(),
        any::<bool>(),
        any::<u8>(),
    )
        .prop_map(|(id, kind, cont, cost, clarified, o, split, r)| Question {
            id,
            kind: [
                QKind::AuditDisagreement,
                QKind::NeedsSplitting,
                QKind::CantMeet,
                QKind::OutOfCredit,
                QKind::EffectFailed,
                QKind::CleanupFailed,
            ][usize::from(kind)],
            cont: match cont {
                0 => Cont::Ingesting,
                1 => Cont::Auditing,
                2 => Cont::Unplanned,
                3 => Cont::Planning { must_split: split },
                4 => Cont::Ready(resume(r)),
                5 => Cont::Stop(o),
                _ => Cont::Nowhere,
            },
            cost,
            clarified,
        })
}

fn node() -> impl Strategy<Value = Node> {
    (
        any::<[u64; 20]>(),
        any::<[u8; 16]>(),
        any::<[bool; 32]>(),
        phase(),
        prop::option::of(question()),
        prop::option::of((any::<u64>(), any::<u8>(), any::<u8>(), any::<bool>())),
    )
        .prop_map(|(w, b, f, phase, question, effect)| {
            let mut n = Node::new(RepoId(w[0]));
            n.phase = phase;
            n.pos = b[0];
            n.footprint = Footprint::inline(w[1]);
            n.generation = w[2];
            n.next_seq = w[3];
            n.effect = effect.map(|(seq, tag, role, cancelling)| InFlight {
                seq,
                kind: effect_kind(tag, role),
                cancelling,
            });
            n.run = f[0].then_some(w[4]);
            n.job = f[1].then_some(w[5]);
            n.ticket = f[2].then_some(w[6]);
            n.last_cmd = w[7];
            n.question = question;
            n.attempt = b[1];
            n.round = b[2];
            n.plan_retries = b[3];
            n.audit_rounds = b[4];
            n.early_end = f[3].then_some(
                [
                    EarlyEnd::Duplicate,
                    EarlyEnd::AlreadyDone,
                    EarlyEnd::NotActionable,
                ][usize::from(b[5] % 3)],
            );
            n.run_failures = b[6];
            n.run_incidents = b[7];
            n.fx_failures = b[8];
            n.fx_incidents = b[9];
            n.rechecks = b[10];
            n.proposed = f[4].then_some(HeadId(w[8]));
            n.head = f[5].then_some(HeadId(w[9]));
            n.checked = f[6].then_some(HeadId(w[10]));
            n.approved = f[7].then_some(HeadId(w[11]));
            n.merged = f[8].then_some(HeadId(w[12]));
            n.waiting_prerequisite = f[9].then_some(w[13]);
            n.credit = Credit {
                start: w[14],
                earned: w[15],
                added: w[16],
                spent: w[17],
                ready: f[10],
                merge_credited: f[11],
                withheld: f[12],
            };
            n.pool = f[13];
            n.holds = f[14];
            n.paused = f[15];
            n.replanned = f[16];
            n.waiting_incident = f[17];
            n.incident_needs_rob = f[18];
            n.adopted = f[19];
            n.pr_open = f[20];
            n.had_pr = f[21];
            n.worktree = f[22];
            n.review_restarted = f[23];
            n.planning_started = f[24];
            n.priority = [
                Priority::P0,
                Priority::P1,
                Priority::P2,
                Priority::P3,
                Priority::Unranked,
            ][usize::from(b[11] % 5)];
            n.review_stop = f[25].then_some(
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
                ][usize::from(b[12] % 9)],
            );
            n
        })
}

fn output() -> impl Strategy<Value = Out> {
    (
        any::<bool>(),
        any::<u64>(),
        any::<u64>(),
        any::<u8>(),
        any::<u8>(),
        prop::option::of(any::<u64>()),
        any::<bool>(),
        outcome(),
    )
        .prop_map(|(effect, node, seq, tag, role, head, flag, o)| {
            let node = NodeId(node);
            if effect {
                Out::Effect {
                    node,
                    seq,
                    kind: effect_kind(tag, role),
                    head: head.map(HeadId),
                }
            } else {
                let note = match role % 8 {
                    0 => NoteKind::UnapprovedMerge,
                    1 => NoteKind::Replanned,
                    2 => NoteKind::PrClosed,
                    3 => NoteKind::PrDequeued,
                    4 => NoteKind::OutsidePush,
                    5 => NoteKind::LateMerge,
                    6 => NoteKind::DependencyAbandoned { number: seq },
                    _ => NoteKind::DependencyCycle { number: seq },
                };
                Out::Notice {
                    node,
                    notice: match tag % 6 {
                        0 => Notice::Ask { question: seq },
                        1 => Notice::MeasuresPosted,
                        2 => Notice::Summary(o),
                        3 => Notice::FileReplacement,
                        4 => Notice::Incident { needs_rob: flag },
                        _ => Notice::Note(note),
                    },
                }
            }
        })
}

fn outbox() -> impl Strategy<Value = Outbox> {
    (
        prop::array::uniform5(prop::option::of(output())),
        prop_oneof![4 => 0usize..=OUTBOX, 1 => any::<usize>()],
    )
        .prop_map(|(items, len)| Outbox { items, len })
}

fn compare(world: World, generation: u64, reply: Reply, out: Outbox) {
    let mut actual = world.clone();
    let mut expected = world;
    let mut actual_out = out;
    let mut expected_out = out;
    let expected_panicked = catch_unwind(AssertUnwindSafe(|| {
        reference::on_reply(&mut expected, 0, generation, reply, &mut expected_out);
    }))
    .is_err();
    let actual_panicked = catch_unwind(AssertUnwindSafe(|| {
        super::on_reply(&mut actual, 0, generation, reply, &mut actual_out, &mut Report::new());
    }))
    .is_err();
    assert_eq!(actual_panicked, expected_panicked, "panic behavior differs");
    assert_eq!(actual, expected, "world differs for {reply:?}");
    assert_eq!(actual_out, expected_out, "outbox differs for {reply:?}");
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 128, failure_persistence: None, ..ProptestConfig::default() })]

    #[test]
    fn admitted_extraction_matches_original(n in node(), other in node(), out in outbox(),
        generation in any::<u64>(), next in any::<u64>(), ticket in any::<u64>(),
        head in any::<u64>(), files in any::<u64>(), size in any::<u8>(),
        flags in any::<[bool; 4]>()) {
        let phases = [n.phase, Phase::Ingesting, Phase::Auditing, Phase::Unplanned,
            Phase::Planning { must_split: flags[0] }, Phase::Ready(Resume::Making),
            Phase::Making, Phase::Checking, Phase::Reviewing, Phase::Publishing,
            Phase::Queued, Phase::Rebasing, Phase::AwaitingRob,
            Phase::Stopping(Outcome::Stopped), Phase::Closed(Outcome::Stopped)];
        let replies = [Reply::IngestDone { size }, Reply::IngestEarlyEnd(EarlyEnd::Duplicate),
            Reply::AuditAgrees, Reply::AuditDisagrees,
            Reply::PlanLeaf { footprint: Footprint::inline(files), fits: flags[0] },
            Reply::PlanSplit, Reply::PlanCantMeet, Reply::PlanCheckRejected,
            Reply::PlanCheckNeedsSplit, Reply::ProtectedPaths,
            Reply::MakerDone { head: HeadId(head) },
            Reply::MakerNeedsFiles { files: Footprint::inline(files), fits: flags[0] },
            Reply::MakerCantMeet, Reply::MakerOutOfRoom,
            Reply::ChecksDone { report: flags[0], pass: flags[1], in_footprint: flags[2] },
            Reply::Approved { head: HeadId(head) }, Reply::ChangesRequested,
            Reply::ReviewerOutOfRoom, Reply::RebaseClean { head: HeadId(head) },
            Reply::RebaseNeedsChanges, Reply::RunFailed];
        for phase in phases {
            for reply in replies {
                for fence in 0..4 {
                    let mut selected = n;
                    selected.phase = phase;
                    if fence != 0 {
                        selected.pos = 1;
                        selected.head = Some(HeadId(head));
                        selected.checked = Some(HeadId(head));
                        selected.run = Some(generation);
                        selected.job = None;
                    }
                    if fence == 2 { selected.run = Some(generation.wrapping_add(1)); }
                    if fence == 3 { selected.run = None; selected.job = Some(generation); }
                    let mut world = World::new(&[1, 2], 1);
                    world.nodes = vec![selected, other];
                    world.next_question = next;
                    world.next_ticket = ticket;
                    world.fail_fast = flags[3];
                    compare(world, generation, reply, out);
                }
            }
        }
    }
}

#[test]
fn audit_disagreement_runs_once_before_reset() {
    let mut world = World::new(&[1], 1);
    let mut n = Node::new(RepoId(0));
    n.phase = Phase::Auditing;
    n.pos = 1;
    n.run = Some(0);
    n.pool = true;
    n.credit.ready = true;
    n.credit.start = 4;
    n.run_failures = 1;
    world.nodes.push(n);
    compare(world, 0, Reply::AuditDisagrees, Outbox::new());
}
