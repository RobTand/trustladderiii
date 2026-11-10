//! The release opens the oldest waiter's slot; later younger visits preserve it.
use super::*;
use super::liveness_bounds::context;
use super::liveness_reservation::{holder, reserved};

verus! {
pub(super) proof fn free_after_release(b: World, n: World, k: int, t: u64)
    requires context(b, k), context(n, k + 24),
        advance_family_post::ready_unobstructed(b.nodes@[0]), b.nodes@[0].ticket == Some(t),
        b.nodes@[1].holds, n.nodes@[1].phase is Closed,
        n.nodes@ == b.nodes@.update(1, n.nodes@[1]),
        n.gates@ == b.gates@, n.file_sets@ == b.file_sets@,
        reserved(b, 2, t), reserved(b, 3, t),
    ensures slot_spec(n, 0), oldest_waiter(n, 0), !overlapped_by_older(n, 0, t),
        reserved(n, 2, t), reserved(n, 3, t),
{
    reveal(context); reveal(reserved); reveal(holder);
    checking::accepted_definition(b);
    checking::accepted_node(b, 0); checking::accepted_node(b, 1);
    checking::accepted_node(b, 2); checking::accepted_node(b, 3);
    checking::accepted_node(n, 1);
    assert(progress_gate_apply::holders(b, b.nodes@[0].repo, 4)
        <= b.gates@[b.nodes@[0].repo.0 as int].w);
    reveal_with_fuel(potential_prefix, 5);
    reveal_with_fuel(other_holders, 5);
    footprint::storage_frame(b, n, b.nodes@[2].footprint, b.nodes@[0].footprint);
    footprint::storage_frame(b, n, b.nodes@[3].footprint, b.nodes@[0].footprint);
    assert forall|j: int| 0 <= j < 4 && j != 0 && n.nodes@[j].holds
        && n.nodes@[j].repo == n.nodes@[0].repo
        implies footprint::disjoint(n, n.nodes@[j].footprint, n.nodes@[0].footprint) by {}
    assert forall|j: int| 0 <= j < 4 implies
        !(n.nodes@[j].phase is Ready) || n.nodes@[j].repo != n.nodes@[0].repo
        || match n.nodes@[j].ticket { None => true, Some(q) => q >= t } by {}
}

pub(super) proof fn slot_frame(b: World, n: World, i: int)
    requires b.nodes@.len() == 4, 2 <= i < 4,
        n.nodes@ == b.nodes@.update(i, n.nodes@[i]),
        n.nodes@[i].holds == b.nodes@[i].holds,
        n.nodes@[i].repo == b.nodes@[i].repo,
        n.nodes@[i].footprint == b.nodes@[i].footprint,
        n.gates@ == b.gates@, n.file_sets@ == b.file_sets@,
    ensures slot_spec(n, 0) == slot_spec(b, 0),
{
    reveal_with_fuel(other_holders, 5);
    assert forall|j: int| 0 <= j < 4 implies
        footprint::disjoint(n, n.nodes@[j].footprint, n.nodes@[0].footprint)
            == footprint::disjoint(b, b.nodes@[j].footprint, b.nodes@[0].footprint) by {
        footprint::storage_frame(b, n, b.nodes@[j].footprint, b.nodes@[0].footprint);
    }
}

pub(super) fn second_visit(w: &mut World, i: usize, Ghost(k): Ghost<int>, Ghost(t): Ghost<u64>)
    requires context(*old(w), k), 0 <= k <= 600, 2 <= i < 4,
        advance_family_post::ready_unobstructed(old(w).nodes@[0]), old(w).nodes@[0].ticket == Some(t),
        !overlapped_by_older(*old(w), 0, t),
        reserved(*old(w), i as int, t),
    ensures context(*final(w), k + 2),
        final(w).nodes@[i as int].holds == old(w).nodes@[i as int].holds,
        final(w).nodes@ == old(w).nodes@.update(i as int, final(w).nodes@[i as int]),
        final(w).gates@ == old(w).gates@, final(w).file_sets@ == old(w).file_sets@,
        slot_spec(*final(w), 0), oldest_waiter(*final(w), 0), !overlapped_by_older(*final(w), 0, t),
{
    let ghost before = *w;
    proof { reveal(context); reveal(reserved); reveal(holder); liveness_bounds::reserves(*w, k, i as int); }
    let mut out = Outbox::new();
    let mut report = Report::new();
    let moved = scheduler::advance(w, i, &mut out, &mut report);
    proof {
        if before.nodes@[i as int].holds {
            advance_family_rules::holds_from_frame(before.nodes@[i as int], w.nodes@[i as int]);
        } else {
            assert(!moved);
        }
        liveness_bounds::advance_accepts(before, *w, i as int);
        liveness_bounds::advance_bounds(before.nodes@[i as int], w.nodes@[i as int],
            before.next_ticket, w.next_ticket, moved, k);
        liveness_bounds::context_frame(before, *w, i as int, k, 2);
        reveal(advance_post);
        slot_frame(before, *w, i as int);
        checking::accepted_node(*w, i as int);
        assert forall|j: int| 0 <= j < 4 implies
            !(w.nodes@[j].phase is Ready) || w.nodes@[j].repo != w.nodes@[0].repo
                || match w.nodes@[j].ticket { None => true, Some(q) => q >= t } by {}
    }
}

pub(super) fn grant(w: &mut World, Ghost(k): Ghost<int>)
    requires context(*old(w), k), 0 <= k <= 700,
        advance_family_post::ready_unobstructed(old(w).nodes@[0]),
        slot_spec(*old(w), 0), oldest_waiter(*old(w), 0),
    ensures context(*final(w), k + 2), final(w).nodes@[0].holds, final(w).nodes@[0].ticket.is_none(),
        final(w).nodes@ == old(w).nodes@.update(0, final(w).nodes@[0]),
{
    let ghost before = *w;
    proof { reveal(context); liveness_bounds::reserves(*w, k, 0); }
    let mut out = Outbox::new();
    let mut report = Report::new();
    let moved = scheduler::advance(w, 0, &mut out, &mut report);
    proof {
        liveness_bounds::advance_accepts(before, *w, 0);
        liveness_bounds::advance_bounds(before.nodes@[0], w.nodes@[0],
            before.next_ticket, w.next_ticket, moved, k);
        liveness_bounds::context_frame(before, *w, 0, k, 2);
    }
}
}
