//! Q54 / #440 / #806: lift the parametric retry comparison to the original worlds.
use super::*;

verus! {
/// Q54 reply/fact diversion: only the actor and next_question can change outside
/// the fail_fast setting; file storage is unchanged on the retry path.
pub(super) open spec fn frame(before: World, after: World) -> bool {
    after.nodes@.len() == 2 && after.nodes@[1] == before.nodes@[1]
        && after.gates@ == before.gates@ && after.file_sets@ == before.file_sets@
        && after.dependencies@ == before.dependencies@ && after.pool_capacity == before.pool_capacity
        && after.next_ticket == before.next_ticket
}

/// Q54 / #440: one world-frame comparison covers all shared retry shapes.
/// The caller obtains source/results/frames from the real handler contracts.
pub(super) proof fn compare(before: World, off: World, on: World, source: Node,
    kind: u8, reason: ReviewStop, on_out: Outbox)
    requires before.nodes@.len() == 2, frame(before, off), frame(before, on),
        kind < 4, source.generation < u64::MAX, before.next_question < u64::MAX,
        !(source.phase is Stopping || source.phase is Closed),
        source.attempt == before.nodes@[0].attempt, source.credit == before.nodes@[0].credit,
        kind == 2 ==> source.phase is Planning && reason == ReviewStop::PlanCantMeet,
        port_failfast_pair::retry_post(source, (off.nodes@[0], off.next_question),
            false, before.next_question, kind, reason),
        port_failfast_pair::retry_post(source, (on.nodes@[0], on.next_question),
            true, before.next_question, kind, reason),
        kind != 3 || source.round != 0 ==> on_out.len == 0,
    ensures !port_failfast_admission::same_world(off, on) ==> {
        on.nodes@[1] == off.nodes@[1]
            && on.nodes@[0].credit == before.nodes@[0].credit
            && on.nodes@[0].phase == Phase::Stopping(Outcome::Stopped)
            && on.nodes@[0].review_stop == Some(reason) && on_out.len == 0
            && on.nodes@[0].question.is_none()
            && port_failfast_pair::retried(before.nodes@[0], off.nodes@[0])
    },
{
    if !port_failfast_admission::same_world(off, on) {
        if (off.nodes@[0], off.next_question) == (on.nodes@[0], on.next_question) {
            assert(off.nodes@ =~= on.nodes@);
        }
        port_failfast_pair::diversion(source, (off.nodes@[0], off.next_question),
            (on.nodes@[0], on.next_question), before.next_question, kind, reason);
        reveal(port_review_retry::round);
    }
}
}
