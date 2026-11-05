//! #806: expose the complete four-leaf checker contract only to its gate consumers.
use super::*;

verus! {
/// #806: full checker equivalence, hidden from unrelated World::check callers.
#[verifier::opaque]
pub(super) open spec fn checked(w: World, ok: bool) -> bool {
    progress_gate::domain(w) ==> ok == progress_gate::accepted(w)
}

/// #806: expose the gate checker guarantee only at the real check call site.
pub(super) proof fn checked_result(w: World, ok: bool)
    requires progress_gate::domain(w), progress_gate::accepted(w), checked(w, ok),
    ensures ok,
{
    reveal(checked);
}

/// #806: exact MakerDone transition, opaque to unrelated reply callers.
#[verifier::opaque]
pub(super) open spec fn maker_transition(before: World, after: World, i: int,
    reply: Reply, before_out: Outbox, after_out: Outbox) -> bool {
    match reply {
        Reply::MakerDone { head } => after.nodes@[i] == retirement::phased(
            Node { head: Some(head), checked: None, ..before.nodes@[i] }, Phase::Checking)
            && after.next_question == before.next_question
            && after.file_sets@ == before.file_sets@ && after_out == before_out,
        _ => true,
    }
}

/// #806: admitted MakerDone reply, opaque until the gate maker proof consumes it.
#[verifier::opaque]
pub(super) open spec fn maker_reply(before: World, after: World, i: int,
    generation: u64, reply: Reply, before_out: Outbox, after_out: Outbox) -> bool {
    match reply {
        Reply::MakerDone { head } => before.nodes@[i].effect.is_none()
            && before.nodes@[i].phase == Phase::Making
            && current_item(before.nodes@[i]) == Some(Item::Await)
            && before.nodes@[i].run == Some(generation) ==>
            after.nodes@[i] == progress_gate::maker_completed(before.nodes@[i], head)
            && after.next_question == before.next_question
            && after.file_sets@ == before.file_sets@ && after_out == before_out,
        _ => true,
    }
}
}
