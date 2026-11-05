//! #832: production semantic plan-check stops, proved independently.
use super::*;

kernel_fn! {
/// Planning / #832: terminal review without credit, retries or a question.
pub(super) fn reply_plan_check(w: &mut World, i: usize, reply: Reply,
    _out: &mut Outbox) -> (result: ()) [
    requires i < old(w).nodes@.len(), old(_out).len < OUTBOX,
        node_headroom(old(w).nodes@[i as int]),
        arithmetic_consistent(old(w).nodes@[i as int]),
        old(w).next_question < u64::MAX,
        matches!(reply, Reply::PlanCheckRejected | Reply::PlanCheckNeedsSplit),
    ensures (successful_reply_input(old(w).nodes@[i as int], reply)
            ==> distance(final(w).nodes@[i as int]) < distance(old(w).nodes@[i as int])),
        final(w).nodes@[i as int].repo == old(w).nodes@[i as int].repo,
        final(w).nodes@[i as int].footprint == old(w).nodes@[i as int].footprint,
        reply_bounds(*old(w), *final(w), i as int, old(_out).len, final(_out).len),
        final(w).nodes@[i as int].last_cmd == old(w).nodes@[i as int].last_cmd,
        (matches!(reply, Reply::PlanCheckRejected | Reply::PlanCheckNeedsSplit)
            && old(w).nodes@[i as int].phase is Planning ==> {
                let n = final(w).nodes@[i as int];
                n.phase == Phase::Stopping(Outcome::Stopped) && n.question.is_none()
                    && n.review_stop == Some(ReviewStop::PlanCantMeet)
                    && n.credit == old(w).nodes@[i as int].credit
                    && n.plan_retries == old(w).nodes@[i as int].plan_retries
                    && final(w).next_question == old(w).next_question
                    && *final(_out) == *old(_out)
            }),
] proof [reveal(distance); reveal_with_fuel(phase_rank, 3); reveal(continuation_rank); reveal(reply_bounds); reveal(retry_bounds);] {
    match reply {
        Reply::PlanCheckRejected => {
            stop_for_review(&mut w.nodes[i], ReviewStop::PlanCantMeet);
        }
        Reply::PlanCheckNeedsSplit => {
            stop_for_review(&mut w.nodes[i], ReviewStop::PlanCantMeet);
        }
        _ => {}
    }
}
}
