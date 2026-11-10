//! Executable command safety contracts, split to bound each required query.
use super::*;

kernel_fn! {
/// Question continuation rows / #771: fund and pay before changing phase, then
/// the replanned note, as a value function (K5): the continued node and the
/// optional note are returned; on_command writes them back.
pub(super) fn go_on(n: Node, q: Question, deposit: u8) -> (result: (Node, Option<Notice>)) [
    requires q.cost <= 1,
        n.generation < u64::MAX,
        n.credit.start as int + n.credit.earned as int + n.credit.added as int
            >= n.credit.spent as int,
        n.credit.withheld ==> q.cost == 0,
        n.credit.added as int + 255 <= u64::MAX as int,
        n.credit.spent < u64::MAX,
    ensures (n.phase == Phase::AwaitingRob && n.question == Some(q) && !n.paused
            && !n.waiting_incident && n.effect.is_none()
            ==> distance(result.0) < distance(n)),
        result.0.repo == n.repo, result.0.footprint == n.footprint,
        (settle_shape(n) ==> settle_shape(result.0)),
        result.0.generation as int <= n.generation as int + 1,
        result.0.next_seq == n.next_seq,
        result.0.credit.start == n.credit.start, result.0.credit.earned == n.credit.earned,
        result.0.credit.added as int <= n.credit.added as int + 255,
        result.0.credit.spent as int <= n.credit.spent as int + 1,
        result.0.last_cmd == n.last_cmd,
        result.0.question.is_none(),
        result.0.proposed == n.proposed, result.0.adopted == n.adopted,
        result.0.review_stop == n.review_stop, result.0.attempt == n.attempt,
        result.0.replanned == n.replanned,
        n.phase == Phase::AwaitingRob ==> handler_safety::command_step(n, result.0),
        result.1 == (if (match q.cont { Cont::Planning { must_split } => !must_split, _ => false })
            && n.replanned && q.kind == QKind::OutOfCredit {
            Some(Notice::Note(NoteKind::Replanned))
        } else { None }),

        (result.0.holds ==> n.holds),
        (result.0.pool ==> n.pool),
        result.0.ticket == n.ticket || result.0.ticket.is_none(),
        result.0.credit.withheld == n.credit.withheld,
        (n.phase == Phase::AwaitingRob && n.question == Some(q) && (deposit == 0 || q.kind == QKind::OutOfCredit) && handler_safety::node_rules_v(n, u64::MAX) ==> handler_safety::node_rules_v(result.0, u64::MAX)),
] {
    let m = continue_node(n, q, deposit);
    let mut note: Option<Notice> = None;
    match q.cont {
        Cont::Planning { must_split } => {
            if !must_split && m.replanned && q.kind == QKind::OutOfCredit {
                note = Some(Notice::Note(NoteKind::Replanned));
            }
        }
        _ => {}
    }
    kernel_proof! {
        if n.phase == Phase::AwaitingRob {
            handler_safety::continue_step(n, m);
        }
    }
    (m, note)
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
        result.credit.merge_credited == n.credit.merge_credited,
        result.credit.withheld == n.credit.withheld,

        (result.holds ==> n.holds),
        (result.pool ==> n.pool),
        result.ticket == n.ticket || result.ticket.is_none(),
        result.credit.withheld == n.credit.withheld,
        (n.phase == Phase::AwaitingRob && n.question == Some(q) && (deposit == 0 || q.kind == QKind::OutOfCredit) && handler_safety::node_rules_v(n, u64::MAX) ==> handler_safety::node_rules_v(result, u64::MAX)),
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
    kernel_proof! {
        if n.phase == Phase::AwaitingRob && n.question == Some(q) && (deposit == 0 || q.kind == QKind::OutOfCredit) && handler_safety::node_rules_v(n, u64::MAX) {
            handler_safety::continued_rules_v(n, q, deposit, u64::MAX);
            reveal(handler_safety::continued_node);
            assert(m == handler_safety::continued_node(n, q, deposit));
        }
    }

    m
}
}

kernel_fn! {
/// K5 value form of set_phase: the same contract over the returned node, with the
/// single borrow kept out of the callers' queries.
pub(super) fn set_phase_v(n: Node, phase: Phase) -> (result: Node) [
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
pub(super) fn enter_stopping_v(n: Node, outcome: Outcome) -> (result: Node) [
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

        (result.holds ==> n.holds),
        (result.pool ==> n.pool),
        result.ticket == n.ticket || result.ticket.is_none(),
        result.credit.withheld == n.credit.withheld,
] {
    let mut m = n;
    enter_stopping(&mut m, outcome);
    kernel_proof! { handler_safety::stopped_no_grants(n, outcome); }
    m
}
}

kernel_fn! {
pub(super) fn continue_exact(n: Node, q: Question, deposit: u8) -> (result: Node) [
    requires q.cost <= 1,
        n.generation < u64::MAX,
        n.credit.start as int + n.credit.earned as int + n.credit.added as int
            >= n.credit.spent as int,
        n.credit.withheld ==> q.cost == 0,
        n.credit.added as int + 255 <= u64::MAX as int,
        n.credit.spent < u64::MAX,
    ensures result == handler_safety::continued_node(n, q, deposit),
] {
    let result = match q.cont {
        Cont::Ingesting => continue_ingesting_v(n, q, deposit),
        Cont::Auditing => continue_auditing_v(n, q, deposit),
        Cont::Unplanned => continue_unplanned_v(n, q, deposit),
        Cont::Planning { must_split } => continue_planning_v(n, q, deposit, must_split),
        Cont::Ready(r) => continue_ready_v(n, q, deposit, r),
        Cont::Stop(o) => continue_stop_v(n, q, deposit, o),
        Cont::Nowhere => continue_nowhere_v(n, q, deposit),
    };
    kernel_proof! { apply_answer_safety::continued_arms_bridge(n, q, deposit); }
    result
}
}

kernel_fn! {
pub(super) fn go_on_exact(n: Node, q: Question, deposit: u8) -> (result: (Node, Option<Notice>)) [
    requires q.cost <= 1,
        n.generation < u64::MAX,
        n.credit.start as int + n.credit.earned as int + n.credit.added as int
            >= n.credit.spent as int,
        n.credit.withheld ==> q.cost == 0,
        n.credit.added as int + 255 <= u64::MAX as int,
        n.credit.spent < u64::MAX,
    ensures result == apply_answer_values::continued_answer(n, q, deposit),
] {

    let m = continue_exact(n, q, deposit);
    let mut note: Option<Notice> = None;
    match q.cont {
        Cont::Planning { must_split } => {
            if !must_split && m.replanned && q.kind == QKind::OutOfCredit {
                note = Some(Notice::Note(NoteKind::Replanned));
            }
        }
        _ => {}
    }

    kernel_proof! { reveal(apply_answer_values::continued_answer); }
    (m, note)

}
}

kernel_fn! {
/// K5 value form of Credit::fund_and_pay: the same update returned whole, so
/// exact callers see the spec's own paid_credit term, not a prophecy Credit.
pub(super) fn fund_and_pay_v(c: Credit, deposit: u8, cost: u8) -> (result: Credit) [
    requires cost <= 1,
        c.start as int + c.earned as int + c.added as int >= c.spent as int,
        c.withheld ==> cost == 0,
        c.added as int + 255 <= u64::MAX as int,
        c.spent as int + cost as int <= u64::MAX as int,
    ensures result == handler_safety::paid_credit(c, deposit, cost),
] {
    let mut m = c;
    m.fund_and_pay(deposit, cost);
    m
}
}

kernel_fn! {
/// Exact-only view of enter_stopping_v for exact-result callers: the same call.
pub(super) fn stopped_v(n: Node, outcome: Outcome) -> (result: Node) [
    requires !(n.phase is Closed || n.phase is Stopping) ==> n.generation < u64::MAX,
    ensures result == retirement::stopped(n, outcome),
] {
    enter_stopping_v(n, outcome)
}
}

kernel_fn! {
fn continue_ingesting_v(n: Node, q: Question, deposit: u8) -> (result: Node) [
    requires q.cost <= 1,
        n.generation < u64::MAX,
        n.credit.start as int + n.credit.earned as int + n.credit.added as int
            >= n.credit.spent as int,
        n.credit.withheld ==> q.cost == 0,
        n.credit.added as int + 255 <= u64::MAX as int,
        n.credit.spent < u64::MAX,
    ensures result == apply_answer_values::continue_ingesting(n, q, deposit),
] {
    let mut m = n;
    m.credit = fund_and_pay_v(m.credit, deposit, q.cost);
    m.question = None;
    m = set_phase_v(m, Phase::Ingesting);
    m
}
}

kernel_fn! {
fn continue_auditing_v(n: Node, q: Question, deposit: u8) -> (result: Node) [
    requires q.cost <= 1,
        n.generation < u64::MAX,
        n.credit.start as int + n.credit.earned as int + n.credit.added as int
            >= n.credit.spent as int,
        n.credit.withheld ==> q.cost == 0,
        n.credit.added as int + 255 <= u64::MAX as int,
        n.credit.spent < u64::MAX,
    ensures result == apply_answer_values::continue_auditing(n, q, deposit),
] {
    let mut m = n;
    m.credit = fund_and_pay_v(m.credit, deposit, q.cost);
    m.question = None;
    m = set_phase_v(m, Phase::Auditing);
    m
}
}

kernel_fn! {
fn continue_unplanned_v(n: Node, q: Question, deposit: u8) -> (result: Node) [
    requires q.cost <= 1,
        n.generation < u64::MAX,
        n.credit.start as int + n.credit.earned as int + n.credit.added as int
            >= n.credit.spent as int,
        n.credit.withheld ==> q.cost == 0,
        n.credit.added as int + 255 <= u64::MAX as int,
        n.credit.spent < u64::MAX,
    ensures result == apply_answer_values::continue_unplanned(n, q, deposit),
] {
    let mut m = n;
    m.credit = fund_and_pay_v(m.credit, deposit, q.cost);
    m.question = None;
    m = set_phase_v(m, Phase::Unplanned);
    m
}
}

kernel_fn! {
fn continue_planning_v(n: Node, q: Question, deposit: u8, must_split: bool) -> (result: Node) [
    requires q.cost <= 1,
        n.generation < u64::MAX,
        n.credit.start as int + n.credit.earned as int + n.credit.added as int
            >= n.credit.spent as int,
        n.credit.withheld ==> q.cost == 0,
        n.credit.added as int + 255 <= u64::MAX as int,
        n.credit.spent < u64::MAX,
    ensures result == apply_answer_values::continue_planning(n, q, deposit, must_split),
] {
    let mut m = n;
    m.credit = fund_and_pay_v(m.credit, deposit, q.cost);
    m.question = None;
    m.plan_retries = 0;
    m = set_phase_v(m, Phase::Planning { must_split });
    m
}
}

kernel_fn! {
fn continue_ready_v(n: Node, q: Question, deposit: u8, r: Resume) -> (result: Node) [
    requires q.cost <= 1,
        n.generation < u64::MAX,
        n.credit.start as int + n.credit.earned as int + n.credit.added as int
            >= n.credit.spent as int,
        n.credit.withheld ==> q.cost == 0,
        n.credit.added as int + 255 <= u64::MAX as int,
        n.credit.spent < u64::MAX,
    ensures result == apply_answer_values::continue_ready(n, q, deposit, r),
] {
    let mut m = n;
    m.credit = fund_and_pay_v(m.credit, deposit, q.cost);
    m.question = None;
    m = set_phase_v(m, Phase::Ready(r));
    m
}
}

kernel_fn! {
fn continue_stop_v(n: Node, q: Question, deposit: u8, o: Outcome) -> (result: Node) [
    requires q.cost <= 1,
        n.generation < u64::MAX,
        n.credit.start as int + n.credit.earned as int + n.credit.added as int
            >= n.credit.spent as int,
        n.credit.withheld ==> q.cost == 0,
        n.credit.added as int + 255 <= u64::MAX as int,
        n.credit.spent < u64::MAX,
    ensures result == apply_answer_values::continue_stop(n, q, deposit, o),
] {
    let mut m = n;
    m.credit = fund_and_pay_v(m.credit, deposit, q.cost);
    m.question = None;
    m = stopped_v(m, o);
    m
}
}

kernel_fn! {
fn continue_nowhere_v(n: Node, q: Question, deposit: u8) -> (result: Node) [
    requires q.cost <= 1,
        n.generation < u64::MAX,
        n.credit.start as int + n.credit.earned as int + n.credit.added as int
            >= n.credit.spent as int,
        n.credit.withheld ==> q.cost == 0,
        n.credit.added as int + 255 <= u64::MAX as int,
        n.credit.spent < u64::MAX,
    ensures result == apply_answer_values::continue_nowhere(n, q, deposit),
] {
    let mut m = n;
    m.credit = fund_and_pay_v(m.credit, deposit, q.cost);
    m.question = None;
    m = stopped_v(m, Outcome::Stopped);
    m
}
}
