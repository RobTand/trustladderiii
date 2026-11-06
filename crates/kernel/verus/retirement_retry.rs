//! Q50/Q54 / #806: exact retry decisions shared by real CI and outside-push paths.
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

/// State-table consistency: ticket and question allocation only enlarge old ID bounds.
pub(super) proof fn question_frame(before: World, after: World, n: Node)
    requires before.gates@ == after.gates@, before.file_sets@ == after.file_sets@,
        before.next_ticket <= after.next_ticket, before.next_question <= after.next_question,
        retirement::node_rules(before, n),
    ensures retirement::node_rules(after, n),
{
    reveal(retirement::node_rules);
    footprint::storage_frame(before, after, n.footprint, n.footprint);
}

/// Q54 / #806: the real failed-fix ladder preserves every production node rule.
/// One branch, no unwind. The old phase can be either CI-accepting phase.
pub(super) proof fn rung_rules(w: World, n: Node, fast: bool, id: u64)
    requires retirement::node_rules(w, n), node_headroom(n),
        matches!(n.phase, Phase::Publishing | Phase::Queued),
        rung_asks(n, fast) ==> id < w.next_question,
    ensures retirement::node_rules(w, rung(n, fast, id, ReviewStop::CiFailed)),
{
    reveal(retirement::node_rules); reveal(rung); reveal(retirement::stopped);
}

/// Q54 / #806: a free fix preserves readiness without consuming credit.
pub(super) proof fn failed_rules(w: World, n: Node, fast: bool, id: u64)
    requires retirement::node_rules(w, n), node_headroom(n),
        matches!(n.phase, Phase::Publishing | Phase::Queued),
        n.round != 0 && rung_asks(n, fast) ==> id < w.next_question,
    ensures retirement::node_rules(w, failed(n, fast, id, ReviewStop::CiFailed)),
{
    reveal(failed);
    if n.round == 0 { reveal(retirement::node_rules); }
    else { rung_rules(w, n, fast, id); }
}

/// Q50 / #806: a changed observed head preserves full node consistency after recheck.
pub(super) proof fn outside_rules(w: World, n: Node, head: HeadId, id: u64)
    requires retirement::node_rules(w, n), node_headroom(n),
        n.had_pr && matches!(n.phase, Phase::Publishing | Phase::Queued | Phase::Ready(Resume::Publishing))
            && n.rechecks >= FREE_RECHECKS && !can_pay(n) ==> id < w.next_question,
    ensures retirement::node_rules(w, fact_result(n, Fact::PushByOther { head }, w.fail_fast, id).0),
{
    reveal(retirement::node_rules); reveal(fact_result); reveal(rechecked); reveal(recheck_core); reveal(continued);
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

/// Q50/Q54 / #806: exact retry updates imply the original shared transition rules.
pub(super) proof fn fact_transition_rules(w: World, n: Node, fact: Fact)
    requires retirement::node_rules(w, n), node_headroom(n), selected(fact),
    ensures node_post(n, fact_result(n, fact, w.fail_fast, w.next_question).0, w.fail_fast),
        fact_result(n, fact, w.fail_fast, w.next_question).0.repo == n.repo,
        fact_result(n, fact, w.fail_fast, w.next_question).0.footprint == n.footprint,
        fact_result(n, fact, w.fail_fast, w.next_question).0.credit.withheld == n.credit.withheld,
        !fact_result(n, fact, w.fail_fast, w.next_question).0.holds || n.holds,
        !fact_result(n, fact, w.fail_fast, w.next_question).0.pool || n.pool,
        fact_result(n, fact, w.fail_fast, w.next_question).0.ticket.is_none()
            || fact_result(n, fact, w.fail_fast, w.next_question).0.ticket == n.ticket,
{
    reveal(retirement::node_rules); reveal(fact_result); reveal(failed); reveal(rung);
    reveal(rechecked); reveal(recheck_core); reveal(continued); reveal(retirement::fact_node); reveal(retirement::stopped);
}

/// Q50 / #806: CI and outside push preserve the complete checker acceptance.
pub(super) proof fn fact_preserves_world(before: World, after: World, fact: Fact)
    requires retirement::domain(before), retirement::accepted(before), selected(fact),
        node_headroom(before.nodes@[0]),
        after.nodes@ == before.nodes@.update(0, fact_result(before.nodes@[0], fact, before.fail_fast, before.next_question).0),
        after.next_question as int == before.next_question as int
            + if fact_result(before.nodes@[0], fact, before.fail_fast, before.next_question).1 { 1int } else { 0int },
        after.gates@ == before.gates@, after.file_sets@ == before.file_sets@,
        after.next_ticket == before.next_ticket, after.pool_capacity == before.pool_capacity,
        after.dependencies@ == before.dependencies@,
    ensures retirement::domain(after), retirement::accepted(after),
        node_post(before.nodes@[0], after.nodes@[0], before.fail_fast),
{
    reveal(retirement::accepted);
    fact_transition_rules(before, before.nodes@[0], fact);
    question_frame(before, after, before.nodes@[0]);
    question_frame(before, after, before.nodes@[1]);
    reveal(fact_result);
    match fact {
        Fact::CiFailed { infra } => {
            let n = before.nodes@[0];
            if n.phase == Phase::Queued || (n.phase == Phase::Publishing && current_item(n) == Some(Item::Do(EffectKind::Enqueue))) {
                if infra {
                    retirement::fact_preserves_node(after, n, fact);
                } else {
                    let m = if n.phase == Phase::Publishing { Node { effect: match n.effect {
                        Some(e) => Some(InFlight { cancelling: true, ..e }), None => None,
                    }, ..n } } else { n };
                    assert(retirement::node_rules(after, m)) by { reveal(retirement::node_rules); }
                    failed_rules(after, m, before.fail_fast, before.next_question);
                }
            }
        },
        Fact::PushByOther { head } => outside_rules(after, before.nodes@[0], head, before.next_question),
        _ => {},
    }
    assert forall|r: int| 0 <= r < 2 implies
        retirement::holders(after, RepoId(r as u64), 2) <= after.gates@[r].w by {
        assert(retirement::holders(after, RepoId(r as u64), 2) <= retirement::holders(before, RepoId(r as u64), 2));
    }
    footprint::storage_frame(before, after, before.nodes@[0].footprint, before.nodes@[1].footprint);
}
}

kernel_fn! {
/// Q50/Q54 / #806: one real apply, original two-node domain, weaker counter bounds.
/// No settledness is required; notice-only output proves all check_outputs assertions.
pub(super) fn verify_retry_apply(mut w: World, fact: Fact) -> (result: (World, Outbox)) [
    requires retirement::domain(w), retirement::accepted(w), retirement::bounded(w), storage_ordered(w), selected(fact),
    ensures retirement::domain(result.0), retirement::accepted(result.0),
        result.0.nodes@ == w.nodes@.update(0, fact_result(w.nodes@[0], fact, w.fail_fast, w.next_question).0),
        result.0.next_question as int == w.next_question as int
            + if fact_result(w.nodes@[0], fact, w.fail_fast, w.next_question).1 { 1int } else { 0int },
        retirement::notices_only(result.1), node_post(w.nodes@[0], result.0.nodes@[0], w.fail_fast),
        result.0.nodes@[1] == w.nodes@[1],
] proof [
    reveal(world_headroom); reveal_with_fuel(open_prefix, 3); reveal(storage_ordered);
    reveal(empty_outbox); reveal(retirement::notices_only); reveal(retirement_domain); reveal(retirement_accepted);
    reveal(retirement::accepted); reveal(retirement::bounded);
] {
    kernel_snapshot! { before = w }
    kernel_proof! {
        open_prefix_monotone(w.nodes@, 0, w.nodes@.len());
        assert forall|j: int| 0 <= j < w.nodes@.len() implies node_headroom(w.nodes@[j]) by {}
        dependencies::indexable_definition(w);
    }
    let checked_before = w.check();
    assert!(checked_before.is_ok());
    kernel_proof! { world_checked_definition(w); }
    let mut out = Outbox::new();
    apply(&mut w, Event::Fact { node: NodeId(0), fact }, &mut out);
    kernel_proof! { fact_preserves_world(before@, w, fact); }
    let checked = w.check();
    assert!(checked.is_ok());
    (w, out)
}
}
