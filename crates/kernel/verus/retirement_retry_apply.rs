//! Q50/Q54 / #806: independently gated retry obligations over the same real apply.
use super::retirement_retry::*;
use super::*;

verus! {
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
}

verus! {
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
    result_rules(after, before.nodes@[0], fact, before.fail_fast, before.next_question);
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

verus! {
/// Q50/Q54 / #806: the local result is consistent before global gate frames are expanded.
pub(super) proof fn result_rules(w: World, n: Node, fact: Fact, fast: bool, id: u64)
    requires retirement::node_rules(w, n), node_headroom(n), selected(fact),
        fact_result(n, fact, fast, id).1 ==> id < w.next_question,
    ensures retirement::node_rules(w, fact_result(n, fact, fast, id).0),
{
    reveal(fact_result);
    match fact {
        Fact::CiFailed { infra } => {
            if n.phase == Phase::Queued || (n.phase == Phase::Publishing && current_item(n) == Some(Item::Do(EffectKind::Enqueue))) {
                if infra {
                    retirement::fact_preserves_node(w, n, fact);
                } else {
                    let m = if n.phase == Phase::Publishing { Node { effect: match n.effect {
                        Some(e) => Some(InFlight { cancelling: true, ..e }), None => None,
                    }, ..n } } else { n };
                    assert(retirement::node_rules(w, m)) by { reveal(retirement::node_rules); }
                    failed_rules(w, m, fast, id);
                }
            }
        },
        Fact::PushByOther { head } => outside_rules(w, n, head, id),
        _ => {},
    }
}
}
