//! Q50/Q54 / #806: exact retry decisions shared by real CI and outside-push paths.
pub(super) use super::retirement_retry_apply::*;
pub(super) use super::retirement_retry_rules::*;
use super::*;

verus! {
/// Q18: one spendable ledger unit, including the withholding rule.
pub(super) open spec fn can_pay(n: Node) -> bool {
    !n.credit.withheld && n.credit.start as int + n.credit.earned as int
        + n.credit.added as int >= n.credit.spent as int + 1
}

/// Q18: the real one-credit debit changes no other ledger field.
pub(super) open spec fn paid(n: Node) -> Node {
    Node { credit: Credit { spent: (n.credit.spent + 1) as u64, ..n.credit }, ..n }
}

/// Q18/Q54: exact ask_rob update, using the real world's next question ID.
pub(super) open spec fn asked(n: Node, id: u64, kind: QKind, cont: Cont) -> Node {
    retirement::phased(Node { holds: false,
        question: Some(Question { id, kind, cont, cost: 1, clarified: false }), ..n }, Phase::AwaitingRob)
}

/// Q54: whether the existing failed-fix ladder allocates a question.
pub(super) open spec fn rung_asks(n: Node, fast: bool) -> bool {
    !fast && ((!can_pay(n)) || (n.attempt >= 3 && n.replanned))
}

/// Q54: exact failed-fix ladder for every attempt and replan state.
#[verifier::opaque]
pub(super) open spec fn rung(n: Node, fast: bool, id: u64, step: ReviewStop) -> Node {
    if fast { retirement::stopped(Node { review_stop: Some(step), ..n }, Outcome::Stopped) }
    else if n.attempt < 3 {
        let m = Node { attempt: (n.attempt + 1) as u8, review_restarted: false, round: 0, ..n };
        if can_pay(n) { retirement::phased(paid(m), Phase::Making) }
        else { asked(m, id, QKind::OutOfCredit, Cont::Ready(Resume::Making)) }
    } else if !n.replanned {
        let m = Node { replanned: true, holds: false, ..n };
        if can_pay(n) { retirement::phased(Node { plan_retries: 0, ..paid(m) }, Phase::Planning { must_split: false }) }
        else { asked(m, id, QKind::OutOfCredit, Cont::Planning { must_split: false }) }
    } else { asked(n, id, QKind::CantMeet, Cont::Ready(Resume::Making)) }
}

/// Q54: free fix round precedes the existing paid ladder.
#[verifier::opaque]
pub(super) open spec fn failed(n: Node, fast: bool, id: u64, step: ReviewStop) -> Node {
    if n.round == 0 { retirement::phased(Node { round: 1, ..n }, Phase::Making) }
    else { rung(n, fast, id, step) }
}

/// Q50: exact free/paid continuation, opaque at the surrounding dispatchers.
#[verifier::opaque]
pub(super) open spec fn continued(n: Node, target: Phase, free: bool) -> Node {
    retirement::phased(Node {
        credit: if free { n.credit } else { Credit { spent: (n.credit.spent + 1) as u64, ..n.credit } },
        holds: if holding(target) { n.holds } else { false }, ..n }, target)
}

/// Q50: recheck branch after the real saturated increment has executed.
#[verifier::opaque]
pub(super) open spec fn recheck_core(n: Node, target: Phase, free: bool, id: u64) -> Node {
    if free || can_pay(n) { continued(n, target, free) }
    else { asked(n, id, QKind::OutOfCredit, Cont::Ready(if target == Phase::Reviewing { Resume::Reviewing } else { Resume::Checking })) }
}

/// Q50: a head change consumes free recheck allowance before credit.
#[verifier::opaque]
pub(super) open spec fn rechecked(n: Node, target: Phase, id: u64) -> Node {
    let m = Node { rechecks: if n.rechecks < 255 { (n.rechecks + 1) as u8 } else { 255u8 }, ..n };
    recheck_core(m, target, m.rechecks <= FREE_RECHECKS, id)
}

/// Q50/Q54: exact result and question allocation of a selected real fact.
#[verifier::opaque]
pub(super) open spec fn fact_result(n: Node, fact: Fact, fast: bool, id: u64) -> (Node, bool) {
    match fact {
        Fact::CiFailed { infra } => {
            if n.phase == Phase::Queued || (n.phase == Phase::Publishing && current_item(n) == Some(Item::Do(EffectKind::Enqueue))) {
                if infra { (retirement::fact_node(n, fact), false) }
                else {
                    let m = if n.phase == Phase::Publishing { Node { effect: match n.effect {
                        Some(e) => Some(InFlight { cancelling: true, ..e }), None => None,
                    }, ..n } } else { n };
                    (failed(m, fast, id, ReviewStop::CiFailed), n.round != 0 && rung_asks(n, fast))
                }
            } else { (n, false) }
        },
        Fact::PushByOther { head } => {
            if n.had_pr && matches!(n.phase, Phase::Publishing | Phase::Queued | Phase::Ready(Resume::Publishing)) {
                let m = Node { head: Some(head), checked: None,
                    review_restarted: n.head == Some(head) && n.review_restarted, ..n };
                (rechecked(m, Phase::Ready(Resume::Checking), id), n.rechecks >= FREE_RECHECKS && !can_pay(n))
            } else { (n, false) }
        },
        _ => (n, false),
    }
}

/// Q50: the exact source phases that accept a published-head recheck.
pub(super) open spec fn published_source(n: Node) -> bool {
    n.had_pr && matches!(n.phase, Phase::Publishing | Phase::Queued | Phase::Ready(Resume::Publishing))
}

/// Q50/Q54: only the two retry fact variants use this stronger contract.
pub(super) open spec fn selected(fact: Fact) -> bool {
    matches!(fact, Fact::CiFailed { .. } | Fact::PushByOther { .. })
}
}

verus! {
/// #805 / Q50: changing the recheck counter preserves the old progress obligation.
pub(super) proof fn recheck_distance(n: Node, target: Phase, free: bool, id: u64)
    requires ordinary_await(n), n.phase == Phase::Rebasing, target == Phase::Checking,
        node_headroom(n),
    ensures distance(recheck_core(n, target, free, id)) < distance(n),
{
    reveal(recheck_core); reveal(continued); reveal(distance);
    reveal_with_fuel(phase_rank, 3); reveal(continuation_rank);
}








}

verus! {
/// #806: every shared check_nodes and generic Q54 postcondition for these facts.
pub(super) open spec fn node_post(b: Node, n: Node, fast: bool) -> bool {
    n.proposed == b.proposed && (b.adopted ==> n.adopted) && n.next_seq >= b.next_seq
        && (b.phase is Closed ==> n.phase == b.phase)
        && n.credit.earned <= b.credit.earned + 1
        && (n.credit.earned > b.credit.earned ==> !b.credit.merge_credited && n.credit.merge_credited)
        && (!fast ==> n.review_stop == b.review_stop)
        && (fast ==> n.attempt <= if b.attempt > 1 { b.attempt } else { 1u8 })
        && (fast ==> b.replanned || !n.replanned)
        && (fast && n.phase is Planning ==> b.phase is Planning || b.phase == Phase::AwaitingRob)
        && (fast ==> match n.question { Some(q) => q.kind == QKind::CantMeet ==> b.question == n.question, None => true })
        && (n.review_stop != b.review_stop ==> b.review_stop.is_none()
            && n.phase == Phase::Stopping(Outcome::Stopped) && n.credit == b.credit && n.question.is_none())
}




}
