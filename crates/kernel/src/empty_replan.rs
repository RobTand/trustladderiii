//! Q27, Rob 2026-11-06: one empty-allowed re-plan per request version.
use super::*;

kernel_fn! {
/// The exact production transition proves the one-use bound and no question or
/// extra credit spend. The usual Planning agenda starts the tool-enabled run.
pub(super) fn reply_empty_replan(w: &mut World, i: usize, _out: &mut Outbox)
    -> (result: ()) [
    requires i < old(w).nodes@.len(), old(_out).len < OUTBOX,
        node_headroom(old(w).nodes@[i as int]),
        arithmetic_consistent(old(w).nodes@[i as int]),
        old(w).next_question < u64::MAX,
        old(w).nodes@[i as int].phase is Planning,
    ensures reply_bounds(*old(w), *final(w), i as int, old(_out).len, final(_out).len),
        final(w).nodes@[i as int].repo == old(w).nodes@[i as int].repo,
        final(w).nodes@[i as int].footprint == old(w).nodes@[i as int].footprint,
        final(w).nodes@[i as int].last_cmd == old(w).nodes@[i as int].last_cmd,
        final(w).nodes@[i as int].empty_allowed_replanned,
        (!old(w).nodes@[i as int].empty_allowed_replanned ==>
            final(w).nodes@[i as int] == retirement::phased(
                Node { empty_allowed_replanned: true, question: None, ..old(w).nodes@[i as int] },
                old(w).nodes@[i as int].phase)),
        (old(w).nodes@[i as int].empty_allowed_replanned ==>
            final(w).nodes@[i as int].phase == Phase::Stopping(Outcome::Stopped)
                && final(w).nodes@[i as int].review_stop == Some(ReviewStop::PlanCantMeet)),
        final(w).nodes@[i as int].credit == old(w).nodes@[i as int].credit,
        final(w).nodes@[i as int].question.is_none(),
        final(w).nodes@[i as int].plan_retries == old(w).nodes@[i as int].plan_retries,
        final(w).nodes@[i as int].replanned == old(w).nodes@[i as int].replanned,
        final(w).next_question == old(w).next_question,
        *final(_out) == *old(_out),
] proof [reveal(reply_bounds); reveal(retry_bounds); reveal(retirement::phased);] {
    let m = &mut w.nodes[i];
    m.question = None;
    if !m.empty_allowed_replanned {
        m.empty_allowed_replanned = true;
        let phase = m.phase;
        set_phase(m, phase);
    } else {
        stop_for_review(m, ReviewStop::PlanCantMeet);
    }
}
}
