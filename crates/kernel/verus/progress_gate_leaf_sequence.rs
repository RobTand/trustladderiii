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
        && forall|j: int| #![trigger w.nodes@[j]] 2 <= j < 4 ==> if w.nodes@[j].holds {
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

/// Q48: expose only the initial local facts used by the composed sequence,
/// keeping the complete checker opaque in the executable sequence query.
pub(super) proof fn input_details(w: World)
    requires input(w),
    ensures w.nodes@[1].holds,
        forall|j: int| 0 <= j < 4 && w.nodes@[j].phase is Ready ==> w.nodes@[j].pos == 0 && !w.nodes@[j].holds,
{
    reveal(retirement_wide::accepted); reveal(retirement::node_rules);
}

/// Q48 / #806: expose only the local facts needed after a real Ready/Queued
/// visit. The sequence keeps the exact update opaque and uses this transition frame.
pub(super) proof fn visit_details(before: World, after: World, i: int)
    requires domain(before), 2 <= i < 4, tracked(before.nodes@[i]),
        before.nodes@[i].phase == Phase::Ready(Resume::Making) || before.nodes@[i].phase == Phase::Queued,
        before.nodes@[i].phase is Ready ==> before.nodes@[i].ticket.is_some(),
        frame(before, after, i), after.nodes@[i] == updated(before, i),
    ensures tracked(after.nodes@[i]), !(after.nodes@[i].phase is Stopping),
        after.nodes@[i].phase is Ready ==> after.nodes@[i].ticket.is_some(),
        before.nodes@[i].phase is Ready && retirement_gate::slot(before, i) && retirement_gate::line(before, i)
            ==> after.nodes@[i].holds,
{
    reveal(updated); reveal(retirement_gate::slot_node);
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
    reveal(updated); reveal(retirement_gate::slot_node); reveal(retirement_wide::accepted); reveal(retirement::node_rules);
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
    reveal(updated); reveal(retirement_gate::slot_node); reveal(retirement_wide::accepted); reveal(retirement::node_rules);
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
/// Q48: granting the freed slot preserves every younger holder's disjointness.
pub(super) proof fn grant_result(before: World, after: World)
    requires domain(before), ordered(before), retirement_gate::slot(before, 0),
        retirement_gate::line(before, 0), frame(before, after, 0),
        after.nodes@[0] == updated(before, 0),
    ensures after.nodes@[0].holds, after.nodes@[0].ticket.is_none(),
        forall|j: int| 2 <= j < 4 && after.nodes@[j].holds ==>
            footprint::disjoint(after, after.nodes@[0].footprint, after.nodes@[j].footprint),
{
    reveal(updated); reveal(retirement_gate::slot_node);
    assert forall|j: int| 2 <= j < 4 && after.nodes@[j].holds implies
        footprint::disjoint(after, after.nodes@[0].footprint, after.nodes@[j].footprint) by {
        assert(before.nodes@[j].holds);
        assert(footprint::disjoint(before, before.nodes@[0].footprint, before.nodes@[j].footprint));
        footprint::storage_frame(before, after, before.nodes@[0].footprint, before.nodes@[j].footprint);
    }
}

}

pub(super) use super::progress_gate_leaf_execute::leaf_progress;
