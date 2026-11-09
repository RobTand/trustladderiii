//! Compare the extracted handlers with the verbatim original body.
use crate::*;
use proptest::prelude::*;

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
        9 => Phase::AwaitingDelivery,
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
                    ReviewStop::AuditDisagreement,
                ][usize::from(b[12] % 10)],
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
                    3 => NoteKind::PrClosed,
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
        0usize..=OUTBOX - 2,
    )
        .prop_map(|(items, len)| Outbox { items, len })
}

fn compare(n: Node, ticket: u64, c: AdvanceContext, out: Outbox, kind: EffectKind) {
    let mut actual = n;
    let mut expected = n;
    let mut actual_ticket = ticket;
    let mut expected_ticket = ticket;
    let mut actual_out = out;
    let mut expected_out = out;
    let expected_result = reference::advance_teardown(
        &mut expected,
        &mut expected_ticket,
        c,
        &mut expected_out,
        kind,
    );
    let actual_result = super::advance_teardown(
        &mut actual,
        &mut actual_ticket,
        c,
        &mut actual_out,
        &mut Report::new(),
        kind,
    );
    assert_eq!(actual_result, expected_result, "result differs");
    assert_eq!(actual, expected, "node differs");
    assert_eq!(actual_ticket, expected_ticket, "ticket differs");
    assert_eq!(actual_out, expected_out, "outbox differs");
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 512, failure_persistence: None, ..ProptestConfig::default() })]

    #[test]
    fn branch_extraction_matches_original(n in node(), q in question(), out in outbox(),
        ticket in any::<u64>(), node_id in any::<usize>(), flags in any::<[bool; 3]>()) {
        let c = AdvanceContext { node: node_id, admit: flags[0], pool: flags[1], slot: flags[2] };
        for outcome in [Outcome::Merged, Outcome::Stopped, Outcome::Replaced,
            Outcome::Duplicate, Outcome::AlreadyDone, Outcome::NotActionable] {
            for (pos, kind) in [(0, EffectKind::StopRun), (1, EffectKind::WithdrawJob),
                (2, EffectKind::ClosePr), (4, EffectKind::RemoveWorktree)] {
                for question in [None, Some(q)] {
                    for gone in [false, true] {
                        let mut selected = n;
                        selected.phase = Phase::Stopping(outcome);
                        selected.pos = pos;
                        selected.question = question;
                        selected.waiting_incident = false;
                        selected.effect = None;
                        selected.next_seq = selected.next_seq.min(u64::MAX - 1);
                        if !selected.credit.merge_credited {
                            selected.credit.earned = selected.credit.earned.min(u64::MAX - 1);
                        }
                        match kind {
                            EffectKind::StopRun => selected.run = (!gone).then_some(n.generation),
                            EffectKind::WithdrawJob => selected.job = (!gone).then_some(n.generation),
                            EffectKind::ClosePr => selected.pr_open = !gone,
                            EffectKind::RemoveWorktree => selected.worktree = !gone,
                            _ => unreachable!(),
                        }
                        compare(selected, ticket, c, out, kind);
                    }
                }
            }
        }
    }
}
