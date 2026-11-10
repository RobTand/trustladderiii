//! E11 composition of one cleanup agenda entry and its matching acknowledgement.
use super::*;
use super::liveness_bounds::{context, small};

verus! {
#[verifier::opaque]
pub(super) open spec fn clean(n: Node, p: int) -> bool {
    n.phase == Phase::Stopping(Outcome::Stopped) && n.pos == p
        && !n.waiting_incident && n.effect.is_none() && n.question.is_none()
        && !n.credit.withheld
}

pub(super) proof fn advance_guard(b: Node, n: Node)
    requires clean(b, b.pos as int), b.pos < 5,
        n.phase == b.phase, n.question.is_none(), distance(n) <= distance(b),
        (n.pos == b.pos + 1 && n.effect.is_none())
            || (n.pos == b.pos && n.effect.is_some() && !n.effect.unwrap().cancelling
                && current_item(b) == Some(Item::Teardown(n.effect.unwrap().kind))),
    ensures !n.waiting_incident,
{
    reveal(clean); reveal(distance);
}

pub(super) proof fn ack_guard(b: Node, n: Node)
    requires b.phase == Phase::Stopping(Outcome::Stopped), b.pos < 5,
        !b.waiting_incident, b.question.is_none(), b.effect.is_some(),
        !b.effect.unwrap().cancelling,
        current_item(b) == Some(Item::Teardown(b.effect.unwrap().kind)),
        n.phase == b.phase, n.pos == b.pos + 1, n.effect.is_none(), n.question.is_none(),
        true,
    ensures !n.waiting_incident,
{
    reveal(distance);
}

pub(super) fn round(w: &mut World, Ghost(k): Ghost<int>)
    requires context(*old(w), k), 0 <= k <= 900,
        clean(old(w).nodes@[1], old(w).nodes@[1].pos as int), old(w).nodes@[1].pos < 5,
    ensures context(*final(w), k + 4), clean(final(w).nodes@[1], old(w).nodes@[1].pos as int + 1),
        final(w).nodes@ == old(w).nodes@.update(1, final(w).nodes@[1]),
        final(w).gates@ == old(w).gates@, final(w).file_sets@ == old(w).file_sets@,
        final(w).nodes@[1].holds == (old(w).nodes@[1].holds && old(w).nodes@[1].pos != 3),
{
    let ghost before = *w;
    proof {
        reveal(context); reveal(clean);
        liveness_bounds::reserves(*w, k, 1);
    }
    let mut out = Outbox::new();
    let mut report = Report::new();
    let moved = scheduler::advance(w, 1, &mut out, &mut report);
    proof {
        liveness_bounds::advance_accepts(before, *w, 1);
        liveness_bounds::advance_bounds(before.nodes@[1], w.nodes@[1],
            before.next_ticket, w.next_ticket, moved, k);
        liveness_bounds::context_frame(before, *w, 1, k, 2);
        reveal(advance_post);
        advance_guard(before.nodes@[1], w.nodes@[1]);
    }
    if let Some(e) = w.nodes[1].effect {
        let ghost issued = *w;
        proof {
            liveness_bounds::reserves(*w, k + 2, 1);
            checking::accepted_node(*w, 1);
            assert(transition_safety::teardown_kind(e.kind));
        }
        let mut ack_out = Outbox::new();
        acknowledgements::on_ack(w, 1, e.seq, Ack::Confirmed, &mut ack_out);
        proof {
            assert(w.nodes@ == issued.nodes@.update(1, w.nodes@[1]));
            liveness_bounds::event_accepts(issued, *w, 1);
            reveal(small);
            liveness_bounds::context_frame(issued, *w, 1, k + 2, 2);
            ack_guard(issued.nodes@[1], w.nodes@[1]);
        }
    } else {
        proof {
            reveal(context); reveal(small);
            assert(w.nodes@ =~= w.nodes@.update(1, w.nodes@[1]));
            liveness_bounds::context_frame(*w, *w, 1, k + 2, 2);
        }
    }
    proof { reveal(clean); }
}
}
