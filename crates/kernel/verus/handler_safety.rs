//! K4b #910: node rows every command keeps, as one opaque post proved per
//! arm shape. Handlers ensure only `command_step`; these small lemmas carry
//! the case analysis so no handler reveals the state-table updates.
use super::*;

verus! {

/// check_nodes identity rows and the Q54 check_fail_fast node rows that hold
/// for every command and answer.
#[verifier::opaque]
pub(super) open spec fn command_step(b: Node, n: Node) -> bool {
    n.proposed == b.proposed && n.adopted == b.adopted
        && (b.phase is Closed ==> n.phase == b.phase)
        && n.review_stop == b.review_stop
        && n.attempt <= (if b.attempt > 1 { b.attempt } else { 1u8 })
        && (b.replanned || !n.replanned)
        && (n.phase is Planning ==> b.phase is Planning || b.phase == Phase::AwaitingRob)
        && !transition_safety::asks_cant_meet(b, n)
}

/// Stop, ChangeRequest and stopping continuations: the state-table stop update.
pub(super) proof fn stop_step(b: Node, o: Outcome)
    ensures command_step(b, retirement::stopped(b, o)),
{
    reveal(command_step); reveal(retirement::stopped);
}

/// Pause, Resume, Retry and MarkHandled: no tracked row changes and any
/// question is kept or cleared.
pub(super) proof fn field_step(b: Node, n: Node)
    requires n.proposed == b.proposed, n.adopted == b.adopted, n.phase == b.phase,
        n.review_stop == b.review_stop, n.attempt == b.attempt, n.replanned == b.replanned,
        n.question.is_none() || n.question == b.question,
    ensures command_step(b, n),
{
    reveal(command_step);
}

/// Rob's unclear answer re-asks his open question once, under a new id.
pub(super) proof fn clarify_step(b: Node, q: Question, id: u64)
    requires b.phase == Phase::AwaitingRob, b.question == Some(q), !q.clarified,
    ensures command_step(b, Node { question: Some(Question { id, clarified: true, ..q }), ..b }),
{
    reveal(command_step);
}

/// Continue, TryAgain and AddCredit: a continuation from AwaitingRob closes the
/// question and may restart the first attempt, but never re-plans.
pub(super) proof fn continue_step(b: Node, n: Node)
    requires b.phase == Phase::AwaitingRob,
        n.proposed == b.proposed, n.adopted == b.adopted, n.review_stop == b.review_stop,
        n.attempt <= (if b.attempt > 1 { b.attempt } else { 1u8 }),
        b.replanned || !n.replanned, n.question.is_none(),
    ensures command_step(b, n),
{
    reveal(command_step);
}

/// A command or answer that changes nothing keeps every row.
pub(super) proof fn same_step(b: Node)
    ensures command_step(b, b),
{
    reveal(command_step);
}

/// The three check_nodes identity rows of command_step, for callers that
/// state them directly.
pub(super) proof fn step_identity(b: Node, n: Node)
    requires command_step(b, n),
    ensures n.proposed == b.proposed, n.adopted == b.adopted,
        b.phase is Closed ==> n.phase == b.phase,
{
    reveal(command_step);
}

/// on_command records the command id before dispatching; the rows ignore it.
pub(super) proof fn last_cmd_step(b: Node, m: Node, n: Node)
    requires m == (Node { last_cmd: m.last_cmd, ..b }), command_step(m, n),
    ensures command_step(b, n),
{
    reveal(command_step);
}

}
