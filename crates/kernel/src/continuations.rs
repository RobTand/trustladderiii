//! Executable command safety contracts, split to bound each required query.
use super::*;

kernel_fn! {
/// Question continuation rows / #771: fund and pay before changing phase, then
/// the replanned note. The phase work is continue_node, which emits nothing.
pub(super) fn go_on(m: &mut Node, i: usize, q: Question, deposit: u8,
    out: &mut Outbox) -> (result: ()) [
    requires old(out).len < OUTBOX, q.cost <= 1,
        old(m).generation < u64::MAX,
        old(m).credit.start as int + old(m).credit.earned as int + old(m).credit.added as int
            >= old(m).credit.spent as int,
        old(m).credit.withheld ==> q.cost == 0,
        old(m).credit.added as int + 255 <= u64::MAX as int,
        old(m).credit.spent < u64::MAX,
    ensures (old(m).phase == Phase::AwaitingRob && old(m).question == Some(q) && !old(m).paused
            && !old(m).waiting_incident && old(m).effect.is_none()
            ==> distance(*final(m)) < distance(*old(m))),
        final(m).repo == old(m).repo, final(m).footprint == old(m).footprint,
        (settle_shape(*old(m)) ==> settle_shape(*final(m))),
        final(m).generation as int <= old(m).generation as int + 1,
        final(m).next_seq == old(m).next_seq,
        final(m).credit.start == old(m).credit.start, final(m).credit.earned == old(m).credit.earned,
        final(m).credit.added as int <= old(m).credit.added as int + 255,
        final(m).credit.spent as int <= old(m).credit.spent as int + 1,
        final(m).last_cmd == old(m).last_cmd,
        final(m).question.is_none(),
        final(m).proposed == old(m).proposed, final(m).adopted == old(m).adopted,
        final(m).review_stop == old(m).review_stop, final(m).attempt == old(m).attempt,
        final(m).replanned == old(m).replanned,
        old(m).phase == Phase::AwaitingRob ==> handler_safety::command_step(*old(m), *final(m)),
        final(out).len <= old(out).len + 1,
        final(out).len == old(out).len || (final(out).len == old(out).len + 1
            && final(out).items@[old(out).len as int]
                == Some(Out::Notice { node: NodeId(i as u64), notice: Notice::Note(NoteKind::Replanned) })),
] {
    kernel_snapshot! { before_go_on = *m }
    continue_node(m, q, deposit);
    match q.cont {
        Cont::Planning { must_split } => {
            if !must_split && m.replanned && q.kind == QKind::OutOfCredit {
                notice(out, i, Notice::Note(NoteKind::Replanned));
            }
        }
        _ => {}
    }
    kernel_proof! {
        if before_go_on@.phase == Phase::AwaitingRob {
            handler_safety::continue_step(before_go_on@, *m);
        }
    }
}
}

kernel_fn! {
/// Question continuation rows / #771: go_on's phase work, verified against one
/// node with no outbox.
pub(super) fn continue_node(m: &mut Node, q: Question, deposit: u8) -> (result: ()) [
    requires q.cost <= 1,
        old(m).generation < u64::MAX,
        old(m).credit.start as int + old(m).credit.earned as int + old(m).credit.added as int
            >= old(m).credit.spent as int,
        old(m).credit.withheld ==> q.cost == 0,
        old(m).credit.added as int + 255 <= u64::MAX as int,
        old(m).credit.spent < u64::MAX,
    ensures (old(m).phase == Phase::AwaitingRob && old(m).question == Some(q) && !old(m).paused
            && !old(m).waiting_incident && old(m).effect.is_none()
            ==> distance(*final(m)) < distance(*old(m))),
        final(m).repo == old(m).repo, final(m).footprint == old(m).footprint,
        (settle_shape(*old(m)) ==> settle_shape(*final(m))),
        final(m).generation as int <= old(m).generation as int + 1,
        final(m).next_seq == old(m).next_seq,
        final(m).credit.start == old(m).credit.start, final(m).credit.earned == old(m).credit.earned,
        final(m).credit.added as int <= old(m).credit.added as int + 255,
        final(m).credit.spent as int <= old(m).credit.spent as int + 1,
        final(m).last_cmd == old(m).last_cmd,
        final(m).question.is_none(),
        final(m).proposed == old(m).proposed, final(m).adopted == old(m).adopted,
        final(m).review_stop == old(m).review_stop, final(m).attempt == old(m).attempt,
        final(m).replanned == old(m).replanned,
] proof [reveal(distance);] {
    m.credit.fund_and_pay(deposit, q.cost);
    m.question = None;
    match q.cont {
        Cont::Ingesting => set_phase(m, Phase::Ingesting),
        Cont::Auditing => set_phase(m, Phase::Auditing),
        Cont::Unplanned => set_phase(m, Phase::Unplanned),
        Cont::Planning { must_split } => {
            m.plan_retries = 0;
            set_phase(m, Phase::Planning { must_split });
        }
        Cont::Ready(r) => set_phase(m, Phase::Ready(r)),
        Cont::Stop(o) => {
            kernel_snapshot! { before_stop = *m }
            enter_stopping(m, o);
            kernel_proof! {
                assert((*m).proposed == before_stop@.proposed && (*m).adopted == before_stop@.adopted
                    && (*m).attempt == before_stop@.attempt && (*m).replanned == before_stop@.replanned)
                    by { reveal(retirement::stopped); }
            }
        }
        Cont::Nowhere => {
            kernel_snapshot! { before_nowhere = *m }
            enter_stopping(m, Outcome::Stopped);
            kernel_proof! {
                assert((*m).proposed == before_nowhere@.proposed && (*m).adopted == before_nowhere@.adopted
                    && (*m).attempt == before_nowhere@.attempt && (*m).replanned == before_nowhere@.replanned)
                    by { reveal(retirement::stopped); }
            }
        }
    }
}
}
