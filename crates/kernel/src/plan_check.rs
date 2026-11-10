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
    ensures reply_family_safety::entry_rows(*old(w), *final(w), i as int, reply),
        (successful_reply_input(old(w).nodes@[i as int], reply)
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
    let c = ReplyCtx { node: i, generation: w.nodes[i].generation,
        reply: reply, fast: w.fail_fast, next_question: w.next_question };
    let change = reply_plan_check_node(w.nodes[i], c);
    resolve_reply_change(w, i, change, _out);
    kernel_proof! {
        reveal(retirement::stopped); reveal(handler_safety::ladder);
        reveal(port_t860_contracts::transition_post); reveal(progress_gate_contracts::maker_transition);
    }
}
}

kernel_fn! {
/// Reply node work, with completed-stage rows separate from world mutation.
pub(super) fn reply_plan_check_node(n: Node, c: ReplyCtx) -> (result: ReplyChange) [
    requires node_headroom(n), arithmetic_consistent(n), c.next_question < u64::MAX,
        matches!(c.reply, Reply::PlanCheckRejected | Reply::PlanCheckNeedsSplit),
    ensures result == reply_family_values::plan_check(n),
        reply_family_values::reply_bound(n, c.next_question, result),
        reply_family_safety::rows(n, c, result),
        (successful_reply_input(n, c.reply) ==> distance(reply_family_values::finish(c.next_question, result)) < distance(n)),
        reply_family_values::finish(c.next_question, result).footprint == n.footprint,
] proof [reveal(distance); reveal_with_fuel(phase_rank, 3); reveal(continuation_rank); reveal(retirement::stopped);] {
    let mut node = n;
    let mut note: Option<Notice> = None;
    let i = c.node;
    let reply = c.reply;
match reply {
        Reply::PlanCheckRejected => {
            stop_for_review(&mut node, ReviewStop::PlanCantMeet);
        }
        Reply::PlanCheckNeedsSplit => {
            stop_for_review(&mut node, ReviewStop::PlanCantMeet);
        }
        _ => {}
    }
    { let value = ReplyChange::Done { node, note }; kernel_proof! { reply_family_safety::check_rows(n, c, value); } return value; }
}
}
