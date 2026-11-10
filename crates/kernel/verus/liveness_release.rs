//! Stop followed by the original five acknowledged cleanup rounds and final advance.
use super::*;
use super::liveness_bounds::{context, small};
use super::liveness_teardown::clean;

verus! {
pub(super) fn stop(w: &mut World, Ghost(k): Ghost<int>)
    requires context(*old(w), k), 0 <= k <= 800,
        !(old(w).nodes@[1].phase is Closed || old(w).nodes@[1].phase is Stopping),
        old(w).nodes@[1].last_cmd == 0, old(w).nodes@[1].effect.is_none(),
        !old(w).nodes@[1].credit.withheld,
    ensures context(*final(w), k + 2), clean(final(w).nodes@[1], 0),
        final(w).nodes@ == old(w).nodes@.update(1, final(w).nodes@[1]),
        final(w).gates@ == old(w).gates@, final(w).file_sets@ == old(w).file_sets@,
{
    let ghost before = *w;
    proof {
        reveal(context); liveness_bounds::reserves(*w, k, 1);
        checking::accepted_node(*w, 1);
    }
    let mut out = Outbox::new();
    command_dispatch::on_command(w, 1, 1, Command::Stop, &mut out);
    proof {
        // Integration (CONTRACTS.md v2.35): the v2.7 Stop rows are behind the opaque command_frame_rows.
        command_frames_reveal::command_frame_rows_reveal(before.nodes@[1], w.nodes@[1], 1, Command::Stop);
        liveness_bounds::event_accepts(before, *w, 1);
        reveal(small);
        liveness_bounds::context_frame(before, *w, 1, k, 2);
        reveal(clean);
    }
}

pub(super) fn finish(w: &mut World, Ghost(k): Ghost<int>)
    requires context(*old(w), k), 0 <= k <= 900, clean(old(w).nodes@[1], 5),
    ensures context(*final(w), k + 2), final(w).nodes@[1].phase == Phase::Closed(Outcome::Stopped),
        !final(w).nodes@[1].holds,
        final(w).nodes@ == old(w).nodes@.update(1, final(w).nodes@[1]),
        final(w).gates@ == old(w).gates@, final(w).file_sets@ == old(w).file_sets@,
{
    let ghost before = *w;
    proof { reveal(context); reveal(clean); liveness_bounds::reserves(*w, k, 1); }
    let mut out = Outbox::new();
    let mut report = Report::new();
    let moved = scheduler::advance(w, 1, &mut out, &mut report);
    proof {
        liveness_bounds::advance_accepts(before, *w, 1);
        liveness_bounds::advance_bounds(before.nodes@[1], w.nodes@[1],
            before.next_ticket, w.next_ticket, moved, k);
        liveness_bounds::context_frame(before, *w, 1, k, 2);
        checking::accepted_node(*w, 1);
    }
}

pub(super) fn release(w: &mut World, Ghost(k): Ghost<int>)
    requires context(*old(w), k), 0 <= k <= 700,
        old(w).nodes@[1].phase == Phase::Rebasing, old(w).nodes@[1].last_cmd == 0,
        old(w).nodes@[1].effect.is_none(), !old(w).nodes@[1].credit.withheld,
    ensures context(*final(w), k + 24), final(w).nodes@[1].phase == Phase::Closed(Outcome::Stopped),
        !final(w).nodes@[1].holds,
        final(w).nodes@ == old(w).nodes@.update(1, final(w).nodes@[1]),
        final(w).gates@ == old(w).gates@, final(w).file_sets@ == old(w).file_sets@,
{
    proof { reveal(clean); }
    stop(w, Ghost(k));
    liveness_teardown::round(w, Ghost(k + 2));
    liveness_teardown::round(w, Ghost(k + 6));
    liveness_teardown::round(w, Ghost(k + 10));
    liveness_teardown::round(w, Ghost(k + 14));
    liveness_teardown::round(w, Ghost(k + 18));
    finish(w, Ghost(k + 22));
}
}
