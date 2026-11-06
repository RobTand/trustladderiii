//! Q50/Q54 / #806: Publishing-before-enqueue and Queued share the actual CI ladder.
use super::*;

verus! {
/// Q50: the Publishing harness's extra assertions, including exact queued parity.
pub(super) proof fn properties(w: World)
    requires retirement::domain(w), retirement::accepted(w), node_headroom(w.nodes@[0]),
        w.nodes@[0].phase == Phase::Publishing,
        current_item(w.nodes@[0]) == Some(Item::Do(EffectKind::Enqueue)),
        !w.nodes@[0].waiting_incident,
    ensures {
        let b = w.nodes@[0];
        let n = retirement_retry::fact_result(b, Fact::CiFailed { infra: false }, w.fail_fast, w.next_question).0;
        let queued = Node { phase: Phase::Queued, pos: 0, effect: None, ..b };
        let q = retirement_retry::fact_result(queued, Fact::CiFailed { infra: false }, w.fail_fast, w.next_question).0;
        n.phase != Phase::Publishing && n.credit == q.credit && n.round == q.round && n.attempt == q.attempt
            && (n.round != b.round || n.credit.spent > b.credit.spent || n.question.is_some() || n.review_stop.is_some())
            && match b.effect { None => true, Some(e) => match n.effect {
                Some(current) => current.seq == e.seq && current.cancelling, None => false,
            } }
    },
{
    reveal(retirement::accepted); reveal(retirement::node_rules);
    reveal(retirement_retry::fact_result); reveal(retirement_retry::failed);
    reveal(retirement_retry::rung); reveal(retirement::stopped);
}

/// Q50: the original same-world Queued comparison is itself fully consistent.
pub(super) proof fn queued_world(before: World, after: World)
    requires retirement::domain(before), retirement::accepted(before),
        before.nodes@[0].phase == Phase::Publishing,
        current_item(before.nodes@[0]) == Some(Item::Do(EffectKind::Enqueue)),
        after.nodes@ == before.nodes@.update(0, Node { phase: Phase::Queued, pos: 0, effect: None, ..before.nodes@[0] }),
        after.gates@ == before.gates@, after.file_sets@ == before.file_sets@,
        after.dependencies@ == before.dependencies@, after.next_ticket == before.next_ticket,
        after.next_question == before.next_question, after.pool_capacity == before.pool_capacity,
    ensures retirement::domain(after), retirement::accepted(after),
{
    reveal(retirement::accepted); reveal(retirement::node_rules);
    retirement::node_frame(before, after, before.nodes@[1]);
    footprint::storage_frame(before, after, before.nodes@[0].footprint, before.nodes@[1].footprint);
}
}

kernel_fn! {
/// Q50 / #806: execute both original real apply paths and compare their ledgers.
/// Bound: two independent one-event runs from the original bounded construction.
pub(super) fn verify_publishing_apply(w: World) -> (result: (World, Outbox)) [
    requires retirement::domain(w), retirement::accepted(w), retirement::bounded(w), storage_ordered(w),
        w.file_sets@.len() == 0,
        w.nodes@[0].phase == Phase::Publishing,
        current_item(w.nodes@[0]) == Some(Item::Do(EffectKind::Enqueue)), !w.nodes@[0].waiting_incident,
    ensures retirement::domain(result.0), retirement::accepted(result.0), retirement::notices_only(result.1),
        result.0.nodes@ == w.nodes@.update(0, retirement_retry::fact_result(w.nodes@[0], Fact::CiFailed { infra: false }, w.fail_fast, w.next_question).0),
        retirement_retry::node_post(w.nodes@[0], result.0.nodes@[0], w.fail_fast),
        result.0.nodes@[1] == w.nodes@[1],
        result.0.nodes@[0].phase != Phase::Publishing,
        result.0.nodes@[0].round != w.nodes@[0].round
            || result.0.nodes@[0].credit.spent > w.nodes@[0].credit.spent
            || result.0.nodes@[0].question.is_some() || result.0.nodes@[0].review_stop.is_some(),
        match w.nodes@[0].effect { None => true, Some(e) => match result.0.nodes@[0].effect {
            Some(current) => current.seq == e.seq && current.cancelling, None => false,
        } },
] {
    kernel_snapshot! { before = w }
    kernel_proof! { reveal(retirement::accepted); reveal(retirement::bounded); }
    let mut queued = snapshot_world(&w);
    queued.nodes[0].phase = Phase::Queued;
    queued.nodes[0].pos = 0;
    queued.nodes[0].effect = None;
    kernel_proof! {
        assert(queued.nodes@ =~= w.nodes@.update(0, Node { phase: Phase::Queued, pos: 0, effect: None, ..w.nodes@[0] }));
        assert(queued.file_sets@ =~= w.file_sets@);
        reveal(storage_ordered); reveal(footprint::ordered_table);
        queued_world(w, queued);
    }
    let (after, out) = retirement_retry::verify_retry_apply(w, Fact::CiFailed { infra: false });
    let (queued_after, _) = retirement_retry::verify_retry_apply(queued, Fact::CiFailed { infra: false });
    kernel_proof! { properties(before@); }
    assert!(after.nodes[0].credit == queued_after.nodes[0].credit);
    assert!(after.nodes[0].round == queued_after.nodes[0].round);
    assert!(after.nodes[0].attempt == queued_after.nodes[0].attempt);
    (after, out)
}
}
