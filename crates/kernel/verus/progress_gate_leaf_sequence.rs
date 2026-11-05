//! Q48 / #806: the original four-leaf sequence, composed from real step lemmas.
use super::progress_gate_leaf_state::*;
use super::*;

verus! {
/// Q48: the oldest waiter retains its ticket; every younger holder is disjoint
/// and every younger non-holder still waits with a strictly later ticket.
pub(super) open spec fn ordered(w: World) -> bool {
    w.nodes@[0].phase == Phase::Ready(Resume::Making) && w.nodes@[0].pos == 0
        && !w.nodes@[0].paused && !w.nodes@[0].waiting_incident && w.nodes@[0].effect.is_none()
        && w.nodes@[0].ticket.is_some()
        && forall|j: int| 2 <= j < 4 ==> if w.nodes@[j].holds {
            footprint::disjoint(w, w.nodes@[0].footprint, w.nodes@[j].footprint)
        } else {
            w.nodes@[j].phase == Phase::Ready(Resume::Making)
                && match (w.nodes@[0].ticket, w.nodes@[j].ticket) {
                    (Some(t), Some(younger)) => t < younger, _ => false,
                }
        }
}

/// #806: same or weaker premises than gate_world_in_phase plus the original
/// leaf assumptions. Counter zeroes and effect absence come from Node::new.
pub(super) open spec fn input(w: World) -> bool {
    domain(w) && retirement_wide::accepted(w) && low(w) && storage_ordered(w)
        && ordered(w) && w.nodes@[1].phase == Phase::Queued
        && !footprint::disjoint(w, w.nodes@[0].footprint, w.nodes@[1].footprint)
        && (forall|j: int| 0 <= j < 4 ==> !w.nodes@[j].paused
            && !w.nodes@[j].waiting_incident && w.nodes@[j].effect.is_none()
            && w.nodes@[j].question.is_none() && w.nodes@[j].generation == 0
            && w.nodes@[j].next_seq == 0 && w.nodes@[j].last_cmd == 0)
        && (forall|j: int| 2 <= j < 4 ==>
            (w.nodes@[j].phase == Phase::Queued || w.nodes@[j].phase == Phase::Ready(Resume::Making)))
}

/// Q48: copying Vec storage without changing values leaves every gate query equal.
pub(super) proof fn queries_frame(before: World, after: World)
    requires before.nodes@.len() == 4, before.nodes@ == after.nodes@,
        before.gates@ == after.gates@, before.file_sets@ == after.file_sets@,
    ensures forall|i: int| 0 <= i < 4 ==> retirement_gate::slot(before, i) == retirement_gate::slot(after, i)
        && retirement_gate::line(before, i) == retirement_gate::line(after, i),
{
    reveal_with_fuel(retirement_gate::slot_count, 5);
    assert forall|i: int, j: int| 0 <= i < 4 && 0 <= j < 4 implies
        footprint::disjoint(before, before.nodes@[i].footprint, before.nodes@[j].footprint)
        == footprint::disjoint(after, after.nodes@[i].footprint, after.nodes@[j].footprint) by {
        footprint::storage_frame(before, after, before.nodes@[i].footprint, before.nodes@[j].footprint);
    }
    assert forall|i: int| 0 <= i < 4 implies retirement_gate::slot(before, i) == retirement_gate::slot(after, i) by {}
    assert forall|i: int, t: u64| 0 <= i < 4 implies
        retirement_gate::overlap(before, i, t) == retirement_gate::overlap(after, i, t) by {
        if retirement_gate::overlap(before, i, t) {
            let j = choose|j: int| retirement_gate::overlapping_before(before, i, j, t);
            assert(retirement_gate::overlapping_before(after, i, j, t));
        }
        if retirement_gate::overlap(after, i, t) {
            let j = choose|j: int| retirement_gate::overlapping_before(after, i, j, t);
            assert(retirement_gate::overlapping_before(before, i, j, t));
        }
    }
    assert forall|i: int, j: int| 0 <= i < 4 && 0 <= j < 4 implies
        retirement_gate::eligible(before, i, j) == retirement_gate::eligible(after, i, j) by {}
    assert forall|i: int| 0 <= i < 4 implies retirement_gate::line(before, i) == retirement_gate::line(after, i) by {
        if retirement_gate::line(before, i) {
            assert forall|j: int| 0 <= j < 4 && retirement_gate::older(after, i, j)
                implies retirement_gate::eligible(after, i, j) by {
                assert(retirement_gate::older(before, i, j));
                assert(retirement_gate::eligible(before, i, j));
            }
        }
        if retirement_gate::line(after, i) {
            assert forall|j: int| 0 <= j < 4 && retirement_gate::older(before, i, j)
                implies retirement_gate::eligible(before, i, j) by {
                assert(retirement_gate::older(after, i, j));
                assert(retirement_gate::eligible(after, i, j));
            }
        }
    }
}

/// Q48: each actual younger visit preserves the oldest ticket and bypass safety.
pub(super) proof fn preserve_order(before: World, after: World, i: int)
    requires domain(before), retirement_wide::accepted(before), ordered(before),
        2 <= i < 4, tracked(before.nodes@[i]), !(before.nodes@[i].phase is Stopping), frame(before, after, i),
        after.nodes@[i] == updated(before, i),
    ensures ordered(after), after.nodes@[0].ticket == before.nodes@[0].ticket,
{
    reveal(retirement_gate::slot_node); reveal(retirement_wide::accepted); reveal(retirement::node_rules);
    assert forall|j: int| 2 <= j < 4 implies if after.nodes@[j].holds {
        footprint::disjoint(after, after.nodes@[0].footprint, after.nodes@[j].footprint)
    } else {
        after.nodes@[j].phase == Phase::Ready(Resume::Making)
            && match (after.nodes@[0].ticket, after.nodes@[j].ticket) {
                (Some(t), Some(younger)) => t < younger, _ => false,
            }
    } by {
        footprint::storage_frame(before, after, before.nodes@[0].footprint, before.nodes@[j].footprint);
        if j == i && !before.nodes@[i].holds && after.nodes@[i].holds {
            assert(retirement_gate::older(before, i, 0));
            progress_gate_leaf::bypass_safety(before, after, i, 0);
        }
    }
}

/// Q48: releasing the last conflicting holder frees both its capacity and files.
pub(super) proof fn released(before: World, after: World)
    requires domain(before), retirement_wide::accepted(before), ordered(before),
        before.nodes@[1].holds, frame(before, after, 1), !after.nodes@[1].holds,
        after.nodes@[1].phase is Closed,
    ensures ordered(after), retirement_gate::slot(after, 0), retirement_gate::line(after, 0),
{
    reveal(retirement_wide::accepted); reveal(retirement::node_rules);
    reveal_with_fuel(potential_prefix, 5); reveal_with_fuel(retirement_gate::slot_count, 5);
    assert forall|j: int| 0 <= j < 4 && j != 0 && after.nodes@[j].holds implies
        footprint::disjoint(after, after.nodes@[j].footprint, after.nodes@[0].footprint) by {
        assert(j == 2 || j == 3);
        footprint::storage_frame(before, after, before.nodes@[0].footprint, before.nodes@[j].footprint);
        footprint::valid_under_table_growth(before, after, before.nodes@[j].footprint);
        footprint::valid_under_table_growth(before, after, before.nodes@[0].footprint);
        footprint::inline_disjoint_symmetric(after, after.nodes@[j].footprint, after.nodes@[0].footprint);
    }
    assert forall|j: int| 2 <= j < 4 implies if after.nodes@[j].holds {
        footprint::disjoint(after, after.nodes@[0].footprint, after.nodes@[j].footprint)
    } else { after.nodes@[j].phase == Phase::Ready(Resume::Making)
        && match (after.nodes@[0].ticket, after.nodes@[j].ticket) {
            (Some(t), Some(younger)) => t < younger, _ => false,
        } } by {
        footprint::storage_frame(before, after, before.nodes@[0].footprint, before.nodes@[j].footprint);
    }
    assert(ordered(after));
    assert forall|j: int| 0 <= j < 4 implies !retirement_gate::older(after, 0, j) by {
        if j == 2 || j == 3 { assert(ordered(after)); }
    }
    progress_gate_leaf::oldest_line(after, 0);
}

/// Q48: visiting a younger leaf after release cannot steal capacity or files.
pub(super) proof fn no_steal_step(before: World, after: World, i: int)
    requires domain(before), retirement_wide::accepted(before), ordered(before),
        2 <= i < 4, tracked(before.nodes@[i]), !(before.nodes@[i].phase is Stopping), retirement_gate::slot(before, 0),
        retirement_gate::line(before, 0), before.nodes@[1].phase is Closed,
        frame(before, after, i), after.nodes@[i] == updated(before, i),
    ensures after.nodes@[i].holds == before.nodes@[i].holds,
        ordered(after), retirement_gate::slot(after, 0), retirement_gate::line(after, 0),
{
    reveal(retirement_gate::slot_node); reveal(retirement_wide::accepted); reveal(retirement::node_rules);
    reveal_with_fuel(retirement_gate::slot_count, 5);
    if before.nodes@[i].phase is Ready {
        assert(retirement_gate::older(before, i, 0));
        progress_gate_leaf::no_steal(before, i, 0);
    }
    preserve_order(before, after, i);
    assert forall|j: int| 0 <= j < 4 && j != 0 && after.nodes@[j].holds implies
        footprint::disjoint(after, after.nodes@[j].footprint, after.nodes@[0].footprint) by {
        footprint::storage_frame(before, after, before.nodes@[j].footprint, before.nodes@[0].footprint);
    }
    assert forall|j: int| 0 <= j < 4 implies !retirement_gate::older(after, 0, j) by {}
    progress_gate_leaf::oldest_line(after, 0);
}
}

kernel_fn! {
/// Q48 / #806: original advance(2), advance(3), real Stop(1), five successful
/// cleanup rounds, advance(1), advance(2), advance(3), advance(0), complete check.
pub(super) fn leaf_progress(mut w: World) -> (result: (World, bool, bool)) [
    requires input(w),
    ensures domain(result.0), retirement_wide::accepted(result.0), result.0.nodes@[0].holds,
        result.0.nodes@[0].ticket.is_none(), result.0.nodes@[1].phase is Closed,
        result.1 == (!w.nodes@[2].holds && result.0.nodes@[2].holds),
        result.2 == (!w.nodes@[3].holds && result.0.nodes@[3].holds),
        w.nodes@[2].phase is Ready && retirement_gate::slot(w, 2) && retirement_gate::line(w, 2)
            ==> result.0.nodes@[2].holds,
        updated(w, 2) == w.nodes@[2] && w.nodes@[3].phase is Ready
            && retirement_gate::slot(w, 3) && retirement_gate::line(w, 3) ==> result.0.nodes@[3].holds,
        result.0.nodes@[2].holds ==> footprint::disjoint(result.0, result.0.nodes@[0].footprint, result.0.nodes@[2].footprint),
        result.0.nodes@[3].holds ==> footprint::disjoint(result.0, result.0.nodes@[0].footprint, result.0.nodes@[3].footprint),
] proof [
    reveal(storage_ordered);
    reveal_with_fuel(open_prefix, 5);
    reveal(retirement_gate::slot_node); reveal(retirement_wide::accepted); reveal(retirement::node_rules);
] {
    kernel_snapshot! { original = w }
    let held_before2 = w.nodes[2].holds;
    w = advance_leaf(w, 2);
    let passed2 = !held_before2 && w.nodes[2].holds;
    kernel_proof! {
        preserve_order(original@, w, 2);
        if updated(original@, 2) == original@.nodes@[2] {
            assert(w.nodes@ =~= original@.nodes@);
            queries_frame(original@, w);
        }
    }
    kernel_snapshot! { visited2 = w }
    let held_before3 = w.nodes[3].holds;
    w = advance_leaf(w, 3);
    let passed3 = !held_before3 && w.nodes[3].holds;
    kernel_proof! { preserve_order(visited2@, w, 3); }
    kernel_snapshot! { blocked = w }
    kernel_proof! { assert(w.nodes@[0].ticket == original@.nodes@[0].ticket); }
    w = progress_gate_leaf_teardown::teardown(w);
    kernel_proof! { released(blocked@, w); }
    assert!(slot_free(&w, 0));
    kernel_snapshot! { freed = w }
    let held2 = w.nodes[2].holds;
    w = advance_leaf(w, 2);
    kernel_proof! { no_steal_step(freed@, w, 2); }
    assert!(w.nodes[2].holds == held2);
    kernel_snapshot! { visited_again2 = w }
    let held3 = w.nodes[3].holds;
    w = advance_leaf(w, 3);
    kernel_proof! { no_steal_step(visited_again2@, w, 3); }
    assert!(w.nodes[3].holds == held3);
    kernel_snapshot! { grant_before = w }
    w = advance_leaf(w, 0);
    kernel_proof! {
        footprint::storage_frame(grant_before@, w, grant_before@.nodes@[0].footprint, grant_before@.nodes@[2].footprint);
        footprint::storage_frame(grant_before@, w, grant_before@.nodes@[0].footprint, grant_before@.nodes@[3].footprint);
    }
    assert!(w.nodes[0].holds && w.nodes[0].ticket.is_none());
    (w, passed2, passed3)
}
}
