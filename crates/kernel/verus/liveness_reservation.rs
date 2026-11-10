//! Reservation and the two visits to each younger leaf.
use super::*;
use super::liveness_bounds::context;

verus! {
#[verifier::opaque]
pub(super) open spec fn holder(n: Node) -> bool {
    n.holds && (current_item(n) == Some(Item::Await)
        || (!n.paused && current_item(n) == Some(Item::Do(EffectKind::PrepareWorktree))))
}

#[verifier::opaque]
pub(super) open spec fn reserved(w: World, i: int, t: u64) -> bool {
    let n = w.nodes@[i];
    n.repo == w.nodes@[0].repo && if n.holds {
        holder(n) && footprint::disjoint(w, n.footprint, w.nodes@[0].footprint)
    } else {
        advance_family_post::ready_unobstructed(n) && n.phase == Phase::Ready(Resume::Making)
            && n.ticket.unwrap() > t
    }
}

pub(super) proof fn await_stutters(b: Node, n: Node, t0: u64, t1: u64, moved: bool)
    requires current_item(b) == Some(Item::Await), !b.waiting_incident,
        advance_post(b, n, t0, t1, moved),
    ensures !moved, n == b,
{
    reveal(advance_post);
}

pub(super) fn first_visit(w: &mut World, i: usize, Ghost(k): Ghost<int>, Ghost(t): Ghost<u64>)
    requires context(*old(w), k), 0 <= k <= 500, 2 <= i < 4,
        advance_family_post::ready_unobstructed(old(w).nodes@[0]),
        old(w).nodes@[0].ticket == Some(t),
        old(w).nodes@[i as int].repo == old(w).nodes@[0].repo,
        (advance_family_post::ready_unobstructed(old(w).nodes@[i as int])
            && old(w).nodes@[i as int].phase == Phase::Ready(Resume::Making)
            )
        || (holder(old(w).nodes@[i as int]) && current_item(old(w).nodes@[i as int]) == Some(Item::Await)
            && !old(w).nodes@[i as int].waiting_incident
            && footprint::disjoint(*old(w), old(w).nodes@[i as int].footprint, old(w).nodes@[0].footprint)),
    ensures context(*final(w), k + 2), reserved(*final(w), i as int, t),
        final(w).nodes@ == old(w).nodes@.update(i as int, final(w).nodes@[i as int]),
        final(w).gates@ == old(w).gates@, final(w).file_sets@ == old(w).file_sets@,
        !old(w).nodes@[i as int].holds && final(w).nodes@[i as int].holds
            ==> footprint::disjoint(*final(w), final(w).nodes@[0].footprint, final(w).nodes@[i as int].footprint),
{
    let ghost before = *w;
    proof {
        reveal(context); reveal(holder); liveness_bounds::reserves(*w, k, i as int);
        checking::accepted_node(*w, i as int);
        checking::accepted_node(*w, 0);
    }
    let mut out = Outbox::new();
    let mut report = Report::new();
    let moved = scheduler::advance(w, i, &mut out, &mut report);
    proof {
        liveness_bounds::advance_accepts(before, *w, i as int);
        liveness_bounds::advance_bounds(before.nodes@[i as int], w.nodes@[i as int],
            before.next_ticket, w.next_ticket, moved, k);
        liveness_bounds::context_frame(before, *w, i as int, k, 2);
        reveal(reserved); reveal(holder); reveal(advance_post);
        if current_item(before.nodes@[i as int]) == Some(Item::Await) {
            await_stutters(before.nodes@[i as int], w.nodes@[i as int],
                before.next_ticket, w.next_ticket, moved);
        } else if moved {
            checking::accepted_node(*w, i as int);
            checking::accepted_node(*w, 0);
            assert(footprint::disjoint(before, before.nodes@[0].footprint, before.nodes@[i as int].footprint));
            footprint::storage_frame(before, *w, before.nodes@[0].footprint, before.nodes@[i as int].footprint);
            footprint::disjoint_symmetric(*w, w.nodes@[0].footprint, w.nodes@[i as int].footprint);
        }
        footprint::storage_frame(before, *w, before.nodes@[i as int].footprint, before.nodes@[0].footprint);
    }
}
}
