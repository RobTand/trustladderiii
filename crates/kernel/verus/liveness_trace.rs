//! G22: the original internal-call gate-leaf trace for widths two and three.
use super::*;
use super::liveness_bounds::context;
use super::liveness_reservation::{holder, reserved};

verus! {
#[verifier::opaque]
pub(super) open spec fn initial_young(w: World, i: int, t: u64) -> bool {
    let n = w.nodes@[i];
    n.repo == w.nodes@[0].repo && !n.paused && !n.waiting_incident && n.effect.is_none()
        && ((n.phase == Phase::Ready(Resume::Making)
                && advance_family_post::ready_unobstructed(n) && n.ticket.unwrap() > t)
            || (n.phase == Phase::Rebasing && current_item(n) == Some(Item::Await)
                && holder(n) && footprint::disjoint(w, n.footprint, w.nodes@[0].footprint)))
}

#[verifier::opaque]
pub(super) open spec fn initial(w: World, t: u64) -> bool {
    context(w, 8) && (w.gates@[0].w == 2 || w.gates@[0].w == 3)
        && w.nodes@[0].phase == Phase::Ready(Resume::Making)
        && advance_family_post::ready_unobstructed(w.nodes@[0]) && w.nodes@[0].ticket == Some(t)
        && w.nodes@[1].phase == Phase::Rebasing && w.nodes@[1].last_cmd == 0
        && w.nodes@[1].effect.is_none() && !w.nodes@[1].credit.withheld && w.nodes@[1].holds
        && !footprint::disjoint(w, w.nodes@[0].footprint, w.nodes@[1].footprint)
        && initial_young(w, 2, t) && initial_young(w, 3, t)
}

pub(super) proof fn young_frame(b: World, n: World, i: int, t: u64)
    requires b.nodes@[0] == n.nodes@[0], b.file_sets@ == n.file_sets@,
    ensures initial_young(b, i, t) == initial_young(n, i, t),
        reserved(b, i, t) == reserved(n, i, t),
{
    reveal(initial_young); reveal(reserved);
    footprint::storage_frame(b, n, b.nodes@[i].footprint, b.nodes@[0].footprint);
}

pub(super) fn gate_leaf_progress(w: &mut World, Ghost(t): Ghost<u64>)
    requires initial(*old(w), t),
    ensures checking::accepted(*final(w)),
        final(w).nodes@[1].phase == Phase::Closed(Outcome::Stopped),
        final(w).nodes@[0].holds && final(w).nodes@[0].ticket.is_none(),
{
    let ghost input = *w;
    proof { reveal(initial); reveal(initial_young); reveal(context); }
    liveness_reservation::first_visit(w, 2, Ghost(8), Ghost(t));
    proof { young_frame(input, *w, 3, t); }
    let ghost visited_two = *w;
    liveness_reservation::first_visit(w, 3, Ghost(10), Ghost(t));
    proof {
        young_frame(visited_two, *w, 2, t);
        assert(w.nodes@[0].ticket == Some(t));
    }
    let ghost reserved_world = *w;
    liveness_release::release(w, Ghost(12));
    proof {
        liveness_finish::free_after_release(reserved_world, *w, 12, t);
        liveness_bounds::reserves(*w, 36, 0);
    }
    let free = scheduler::slot_free(w, 0);
    proof { assert(free); }
    let ghost released = *w;
    liveness_finish::second_visit(w, 2, Ghost(36), Ghost(t));
    proof { young_frame(released, *w, 3, t); }
    liveness_finish::second_visit(w, 3, Ghost(38), Ghost(t));
    liveness_finish::grant(w, Ghost(40));
    proof { liveness_bounds::reserves(*w, 42, 0); reveal(context); }
    let checked = w.consistent();
    proof { assert(checked); }
}
}
