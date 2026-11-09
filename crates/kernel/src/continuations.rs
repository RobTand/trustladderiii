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
    let m = continue_node(w.nodes[i], q, deposit);
    w.nodes[i] = m;
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
            handler_safety::continue_step(before_go_on@, w.nodes@[i as int]);
        }
    }
}
}

kernel_fn! {
/// Question continuation rows / #771: go_on's node work as a value function
/// (K5): the node comes in by value and the continued node is returned, so no
/// mutable reference chain enters this query. Emits nothing; the replanned
/// note is go_on's.
pub(super) fn continue_node(n: Node, q: Question, deposit: u8) -> (result: Node) [
    requires q.cost <= 1,
        n.generation < u64::MAX,
        n.credit.start as int + n.credit.earned as int + n.credit.added as int
            >= n.credit.spent as int,
        n.credit.withheld ==> q.cost == 0,
        n.credit.added as int + 255 <= u64::MAX as int,
        n.credit.spent < u64::MAX,
    ensures (n.phase == Phase::AwaitingRob && n.question == Some(q) && !n.paused
            && !n.waiting_incident && n.effect.is_none()
            ==> distance(result) < distance(n)),
        result.repo == n.repo, result.footprint == n.footprint,
        (settle_shape(n) ==> settle_shape(result)),
        result.generation as int <= n.generation as int + 1,
        result.next_seq == n.next_seq,
        result.credit.start == n.credit.start, result.credit.earned == n.credit.earned,
        result.credit.added as int <= n.credit.added as int + 255,
        result.credit.spent as int <= n.credit.spent as int + 1,
        result.last_cmd == n.last_cmd,
        result.question.is_none(),
        result.proposed == n.proposed, result.adopted == n.adopted,
        result.review_stop == n.review_stop, result.attempt == n.attempt,
        result.replanned == n.replanned,
] proof [reveal(distance);] {
    let mut m = n;
    m.credit.fund_and_pay(deposit, q.cost);
    m.question = None;
    match q.cont {
        Cont::Ingesting => m = set_phase_v(m, Phase::Ingesting),
        Cont::Auditing => m = set_phase_v(m, Phase::Auditing),
        Cont::Unplanned => m = set_phase_v(m, Phase::Unplanned),
        Cont::Planning { must_split } => {
            m.plan_retries = 0;
            m = set_phase_v(m, Phase::Planning { must_split });
        }
        Cont::Ready(r) => m = set_phase_v(m, Phase::Ready(r)),
        Cont::Stop(o) => {
            kernel_snapshot! { before_stop = m }
            m = enter_stopping_v(m, o);
            kernel_proof! {
                assert(m.proposed == before_stop@.proposed && m.adopted == before_stop@.adopted
                    && m.attempt == before_stop@.attempt && m.replanned == before_stop@.replanned)
                    by { reveal(retirement::stopped); }
            }
        }
        Cont::Nowhere => {
            kernel_snapshot! { before_nowhere = m }
            m = enter_stopping_v(m, Outcome::Stopped);
            kernel_proof! {
                assert(m.proposed == before_nowhere@.proposed && m.adopted == before_nowhere@.adopted
                    && m.attempt == before_nowhere@.attempt && m.replanned == before_nowhere@.replanned)
                    by { reveal(retirement::stopped); }
            }
        }
    }
    m
}
}

kernel_fn! {
/// K5 value form of set_phase: the same contract over the returned node, with the
/// single borrow kept out of the callers' queries.
fn set_phase_v(n: Node, phase: Phase) -> (result: Node) [
    requires n.generation < u64::MAX,
    ensures
        result == (Node {
        generation: (n.generation + 1) as u64,
        phase, pos: 0, ticket: None,
        effect: match n.effect {
        Some(e) => Some(InFlight { cancelling: true, ..e }), None => None,
        },
        ..n
        }),
        result.generation as int == n.generation as int + 1,
        result.phase == phase, result.pos == 0, result.ticket.is_none(),
        result.effect == match n.effect {
        Some(e) => Some(InFlight { cancelling: true, ..e }),
        None => None,
        },
        result.credit == n.credit, result.next_seq == n.next_seq,
        result.last_cmd == n.last_cmd, result.question == n.question,
        result.run == n.run, result.job == n.job,
        result.pool == n.pool, result.holds == n.holds,
        result.paused == n.paused, result.waiting_incident == n.waiting_incident,
] {
    let mut m = n;
    set_phase(&mut m, phase);
    m
}
}

kernel_fn! {
/// K5 value form of enter_stopping: the same contract over the returned node, with the
/// single borrow kept out of the callers' queries.
fn enter_stopping_v(n: Node, outcome: Outcome) -> (result: Node) [
    requires !(n.phase is Closed || n.phase is Stopping)
        ==> n.generation < u64::MAX,
    ensures result == retirement::stopped(n, outcome),
        (!n.paused && !n.waiting_incident && n.effect.is_none()
        && !(n.phase is Closed || n.phase is Stopping)
        ==> distance(result) < distance(n)),
        result.repo == n.repo, result.footprint == n.footprint,
        (settle_shape(n) ==> settle_shape(result)),
        result.generation as int == n.generation as int
        + if n.phase is Closed || n.phase is Stopping { 0int } else { 1int },
        result.phase == match n.phase {
        Phase::Closed(o) => Phase::Closed(o),
        Phase::Stopping(o) => Phase::Stopping(if outcome == Outcome::Merged { outcome } else { o }),
        _ => Phase::Stopping(outcome),
        },
        result.next_seq == n.next_seq, result.credit == n.credit,
        result.last_cmd == n.last_cmd,
        result.review_stop == n.review_stop,
        (n.phase is Closed || n.phase is Stopping) ==> result == (Node {
        phase: result.phase, ..n
        }),
        !(n.phase is Closed || n.phase is Stopping) ==>
        result.pos == 0 && result.ticket.is_none() && result.question.is_none()
        && !result.waiting_incident && !result.incident_needs_rob,
] {
    let mut m = n;
    enter_stopping(&mut m, outcome);
    m
}
}
