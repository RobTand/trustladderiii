//! Executable command safety contracts, split to bound each required query.
use super::*;

kernel_fn! {
/// Question continuation rows / #771: fund and pay before changing phase.
/// The node work is continue_node; this world entry frames the other nodes.
pub(super) fn go_on(w: &mut World, i: usize, q: Question, deposit: u8,
    out: &mut Outbox) -> (result: ()) [
    requires i < old(w).nodes@.len(), old(out).len < OUTBOX, q.cost <= 1,
        old(w).nodes@[i as int].generation < u64::MAX,
        old(w).nodes@[i as int].credit.start as int
            + old(w).nodes@[i as int].credit.earned as int
            + old(w).nodes@[i as int].credit.added as int
                >= old(w).nodes@[i as int].credit.spent as int,
        old(w).nodes@[i as int].credit.withheld ==> q.cost == 0,
        old(w).nodes@[i as int].credit.added as int + 255 <= u64::MAX as int,
        old(w).nodes@[i as int].credit.spent < u64::MAX,
    ensures (old(w).nodes@[i as int].phase == Phase::AwaitingRob
            && old(w).nodes@[i as int].question == Some(q) && !old(w).nodes@[i as int].paused
            && !old(w).nodes@[i as int].waiting_incident && old(w).nodes@[i as int].effect.is_none()
            ==> distance(final(w).nodes@[i as int]) < distance(old(w).nodes@[i as int])),
        final(w).nodes@[i as int].repo == old(w).nodes@[i as int].repo,
        final(w).nodes@[i as int].footprint == old(w).nodes@[i as int].footprint,
        (settle_shape(old(w).nodes@[i as int]) ==> settle_shape(final(w).nodes@[i as int])),
        final(w).nodes@.len() == old(w).nodes@.len(),
        forall|j: int| 0 <= j < old(w).nodes@.len() && j != i
            ==> final(w).nodes@[j] == old(w).nodes@[j],
        final(w).nodes@[i as int].generation as int <= old(w).nodes@[i as int].generation as int + 1,
        final(w).nodes@[i as int].next_seq == old(w).nodes@[i as int].next_seq,
        final(w).nodes@[i as int].credit.start == old(w).nodes@[i as int].credit.start,
        final(w).nodes@[i as int].credit.earned == old(w).nodes@[i as int].credit.earned,
        final(w).nodes@[i as int].credit.added as int <= old(w).nodes@[i as int].credit.added as int + 255,
        final(w).nodes@[i as int].credit.spent as int <= old(w).nodes@[i as int].credit.spent as int + 1,
        final(w).nodes@[i as int].last_cmd == old(w).nodes@[i as int].last_cmd,
        final(w).nodes@[i as int].question.is_none(),
        final(w).nodes@[i as int].proposed == old(w).nodes@[i as int].proposed,
        final(w).nodes@[i as int].adopted == old(w).nodes@[i as int].adopted,
        final(w).nodes@[i as int].review_stop == old(w).nodes@[i as int].review_stop,
        final(w).nodes@[i as int].attempt == old(w).nodes@[i as int].attempt,
        final(w).nodes@[i as int].replanned == old(w).nodes@[i as int].replanned,
        old(w).nodes@[i as int].phase == Phase::AwaitingRob
            ==> handler_safety::command_step(old(w).nodes@[i as int], final(w).nodes@[i as int]),
        final(w).next_ticket == old(w).next_ticket,
        final(w).next_question == old(w).next_question,
        final(w).gates@ == old(w).gates@,
        final(w).pool_capacity == old(w).pool_capacity,
        final(w).fail_fast == old(w).fail_fast,
        final(w).file_sets@ == old(w).file_sets@,
        final(w).dependencies@ == old(w).dependencies@,
        final(out).len <= old(out).len + 1,
] {
    kernel_snapshot! { before_go_on = w.nodes@[i as int] }
    continue_node(&mut w.nodes[i], i, q, deposit, out);
    w.withhold_dependency_credit(i);
    kernel_proof! {
        if before_go_on@.phase == Phase::AwaitingRob {
            handler_safety::continue_step(before_go_on@, w.nodes@[i as int]);
        }
    }
}
}

kernel_fn! {
/// Question continuation rows / #771: go_on's node work, verified against one
/// node so the world frame is proved once at the call.
pub(super) fn continue_node(m: &mut Node, i: usize, q: Question, deposit: u8,
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
        final(out).len <= old(out).len + 1,
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
            if !must_split && m.replanned && q.kind == QKind::OutOfCredit {
                notice(out, i, Notice::Note(NoteKind::Replanned));
            }
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
