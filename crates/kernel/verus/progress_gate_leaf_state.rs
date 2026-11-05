//! Q48 / #806: complete-checker preservation for the real leaf sequence.
use super::*;

verus! {
/// #806: original four leaves, with positive width generalized beyond W2/W3.
pub(super) open spec fn domain(w: World) -> bool {
    retirement_wide::domain(w) && w.nodes@.len() == 4 && w.gates@.len() == 1
        && w.file_sets@.len() == 0 && w.next_question == 0
}

/// #806: these generous reserves include every counter in the original constructor.
pub(super) open spec fn low(w: World) -> bool {
    w.next_ticket <= 1024 && forall|j: int| 0 <= j < w.nodes@.len() ==>
        w.nodes@[j].generation <= 1024 && w.nodes@[j].next_seq <= 1024
        && w.nodes@[j].credit.start <= 1024 && w.nodes@[j].credit.earned <= 1024
        && w.nodes@[j].credit.added <= 1024 && w.nodes@[j].credit.spent <= 1024
}

/// Q48: only Ready(Making), Queued, the freshly granted maker, and successful
/// Stopped teardown occur in the original leaf sequence.
pub(super) open spec fn tracked(n: Node) -> bool {
    !n.paused && !n.waiting_incident && n.effect.is_none() && n.question.is_none()
        && (n.phase == Phase::Ready(Resume::Making) && n.pos == 0
            || n.phase == Phase::Queued
            || n.phase == Phase::Making && n.pos == 0
            || progress_gate_leaf_steps::teardown_shape(n))
}

/// #806: exact result of one real tracked advance; all cases are source-connected.
pub(super) open spec fn updated(w: World, i: int) -> Node {
    let n = w.nodes@[i];
    if n.phase is Ready {
        retirement_gate::slot_node(n, w.next_ticket,
            retirement_gate::slot(w, i) && retirement_gate::line(w, i))
    } else if n.phase == Phase::Making {
        progress_gate_leaf_steps::emitted(n, EffectKind::PrepareWorktree)
    } else if n.phase is Stopping { progress_gate_leaf_steps::cleanup_advance(n) }
    else { n }
}

/// #806: the scheduler changes one node; the other world tables keep their values.
pub(super) open spec fn frame(before: World, after: World, i: int) -> bool {
    after.nodes@ == before.nodes@.update(i, after.nodes@[i])
        && after.gates@ == before.gates@ && after.file_sets@ == before.file_sets@
        && after.dependencies@ == before.dependencies@ && after.fail_fast == before.fail_fast
        && after.pool_capacity == before.pool_capacity && after.next_question == before.next_question
        && after.next_ticket == before.next_ticket
}

/// Q48: exact single-node update preserves every production checker condition.
pub(super) proof fn update_safe(before: World, after: World, i: int)
    requires domain(before), retirement_wide::accepted(before), low(before), 0 <= i < 4,
        tracked(before.nodes@[i]), before.nodes@[i].ticket.is_some() || !(before.nodes@[i].phase is Ready),
        frame(before, after, i), after.nodes@[i] == updated(before, i),
    ensures domain(after), retirement_wide::accepted(after),
{
    reveal(retirement_wide::accepted); reveal(retirement::node_rules);
    reveal(retirement_gate::slot_node);
    reveal_with_fuel(potential_prefix, 5); reveal_with_fuel(retirement_gate::slot_count, 5);
    assert forall|j: int| 0 <= j < 4 implies retirement::node_rules(after, after.nodes@[j]) by {
        retirement_frames::node_frame(before, after, before.nodes@[j]);
        footprint::storage_frame(before, after, before.nodes@[j].footprint, before.nodes@[j].footprint);
    }
    assert forall|a: int, b: int| 0 <= a < b < 4 implies retirement_wide::pair(after, a, b) by {
        footprint::storage_frame(before, after, before.nodes@[a].footprint, before.nodes@[b].footprint);
        assert(retirement_wide::pair(before, a, b));
        assert(after.nodes@[a].footprint == before.nodes@[a].footprint);
        assert(after.nodes@[b].footprint == before.nodes@[b].footprint);
        if after.nodes@[i].holds && !before.nodes@[i].holds {
            assert(retirement_gate::slot(before, i));
            if a == i && after.nodes@[b].holds {
                assert(footprint::disjoint(before, before.nodes@[b].footprint, before.nodes@[i].footprint));
                footprint::inline_disjoint_symmetric(before, before.nodes@[i].footprint, before.nodes@[b].footprint);
            }
            if b == i && after.nodes@[a].holds {
                assert(footprint::disjoint(before, before.nodes@[a].footprint, before.nodes@[i].footprint));
            }
        } else { assert(retirement_wide::pair(before, a, b)); }
    }
}
}

kernel_fn! {
/// Q48 / #806: advance the original leaf through the real scheduler and checker.
pub(super) fn advance_leaf(mut w: World, i: usize) -> (result: World) [
    requires domain(w), retirement_wide::accepted(w), low(w), storage_ordered(w),
        i < 4, tracked(w.nodes@[i as int]),
        w.nodes@[i as int].generation <= 1000, w.nodes@[i as int].next_seq <= 1000,
        w.nodes@[i as int].phase is Ready ==> w.nodes@[i as int].ticket.is_some(),
    ensures domain(result), retirement_wide::accepted(result), storage_ordered(result),
        frame(w, result, i as int), result.nodes@[i as int] == updated(w, i as int),
        low(result), result.nodes@[i as int].generation <= w.nodes@[i as int].generation + 1,
        result.nodes@[i as int].next_seq <= w.nodes@[i as int].next_seq + 1,
] proof [
    reveal(empty_outbox); reveal(advance_post); reveal(progress_gate_leaf_steps::advance_details); reveal(storage_ordered);
    reveal(retirement_wide::accepted); reveal(retirement::node_rules);
    reveal_with_fuel(open_prefix, 5); reveal_with_fuel(potential_prefix, 5);
    reveal(retirement_wide_domain); reveal(retirement_wide_accepted);
] {
    kernel_snapshot! { before = w }
    let mut out = Outbox::new();
    let changed = advance(&mut w, i, &mut out);
    kernel_proof! {
        if before@.nodes@[i as int].phase == Phase::Queued { assert(!changed); }
        reveal(retirement_gate::slot_node);
        assert(w.nodes@[i as int] == updated(before@, i as int));
        assert(frame(before@, w, i as int));
        update_safe(before@, w, i as int);
    }
    let checked = w.check();
    assert!(checked.is_ok());
    w
}
}
