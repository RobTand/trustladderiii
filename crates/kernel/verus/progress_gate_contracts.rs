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

/// Maker completion row / #806: rejected and unrelated replies make no MakerDone claim.
pub(super) proof fn maker_reply_inactive(before: World, after: World, i: int,
    generation: u64, reply: Reply, before_out: Outbox, after_out: Outbox)
    requires 0 <= i < before.nodes@.len(),
        !(reply is MakerDone) || before.nodes@[i].effect.is_some()
            || before.nodes@[i].phase != Phase::Making
            || current_item(before.nodes@[i]) != Some(Item::Await)
            || before.nodes@[i].run != Some(generation),
    ensures maker_reply(before, after, i, generation, reply, before_out, after_out),
{
    reveal(maker_reply);
}

/// Maker completion row / #806: compose the real end_operation and reply_transition contracts.
pub(super) proof fn maker_reply_composed(before: World, ended: World, after: World, i: int,
    generation: u64, reply: Reply, before_out: Outbox, after_out: Outbox)
    requires 0 <= i < before.nodes@.len(), 0 <= i < ended.nodes@.len(),
        ended.nodes@[i] == (Node { run: None, job: None, pool: false,
            run_failures: 0, run_incidents: 0,
            effect: match before.nodes@[i].effect {
                Some(e) => Some(InFlight { cancelling: e.cancelling
                    || matches!(e.kind, EffectKind::StartRun(_)
                        | EffectKind::SubmitChecks | EffectKind::StartRebase), ..e }),
                None => None,
            }, ..before.nodes@[i] }),
        ended.next_question == before.next_question, ended.file_sets@ == before.file_sets@,
        maker_transition(ended, after, i, reply, before_out, after_out),
    ensures maker_reply(before, after, i, generation, reply, before_out, after_out),
{
    reveal(maker_reply);
    reveal(maker_transition);
}

/// Reply rows / #806: unchanged file storage preserves the opaque ordering predicate.
pub(super) proof fn table_order_frame(before: World, after: World)
    requires footprint::ordered_table(before), after.file_sets@ == before.file_sets@,
    ensures footprint::ordered_table(after),
{
    reveal(footprint::ordered_table);
}

/// Reply rows / #806: finishing an unobstructed operation preserves its input and rank.
pub(super) proof fn end_reply_progress(before: Node, after: Node, reply: Reply)
    requires successful_reply_input(before, reply),
        after == (Node { run: None, job: None, pool: false,
            run_failures: 0, run_incidents: 0, ..before }),
    ensures successful_reply_input(after, reply), distance(after) == distance(before),
{
    reveal(distance);
}
}
