//! E2.N4c (CONTRACTS.md v2.41/v2.42, owner apply), design 3: a waiting owner's rows over the answered node,
//! behind one opaque atom (form rule 2), proven arm by arm over the exact answer values (rule 6) and composed
//! in answer_command by one rule-7 lemma. Own module (module context).
use super::*;

verus! {

/// A waiting owner's rows over an answered node `r` (input node `b`): N4c before the dependency withhold.
#[verifier::opaque]
pub(super) open spec fn owner_rows(r: Node, b: Node, id: u64, answer: Answer) -> bool {
    !r.planning_started && dependencies::waits(r.question) && dependencies::blocked_phase(r.phase)
        && (r.phase == Phase::Unplanned ==> !r.holds && !r.pool && r.ticket.is_none() && r.run.is_none())
        && (dependencies::early(r.phase) ==> dependencies::zero_credit(r.credit)
            || (handler_safety::answer_withholds(b, id, answer) && r.credit.ready))
}

/// Unchanged node.
pub(super) proof fn owner_same(n: Node, id: u64, answer: Answer)
    ensures dependencies::owner_waiting_ok(n) ==> owner_rows(n, n, id, answer),
{
    reveal(owner_rows);
}

/// Unclear: a clarifying question keeps the phase and the continuation.
pub(super) proof fn owner_unclear(n: Node, nq: u64, id: u64, q: Question)
    requires n.phase == Phase::AwaitingRob, n.question == Some(q),
    ensures dependencies::owner_waiting_ok(n)
        ==> owner_rows(apply_answer_values::unclear_answer(n, nq, q).0, n, id, Answer::Unclear),
{
    reveal(owner_rows);
}

/// Continue: node rules fit the question (question_fits), the owner waits (waits), so the continuation is
/// early or a stop; an unready ledger is zero (cost 0), a ready one is withheld by this answer.
pub(super) proof fn owner_continue(n: Node, nq: u64, id: u64, q: Question)
    requires n.phase == Phase::AwaitingRob, n.question == Some(q), q.id == id,
        apply_answer_values::permits(q.kind, Answer::Continue),
    ensures (handler_safety::node_rules_v(n, nq) && dependencies::owner_waiting_ok(n))
        ==> owner_rows(apply_answer_values::continued_answer(n, q, 0).0, n, id, Answer::Continue),
{
    reveal(owner_rows);
    reveal(apply_answer_values::continued_answer);
    reveal(handler_safety::continued_node);
    reveal(retirement::stopped);
    reveal(dependencies::zero_credit);
    if handler_safety::node_rules_v(n, nq) && dependencies::owner_waiting_ok(n) {
        reveal(handler_safety::node_rules_v);
        assert(question_fits(q));
    }
}

/// TryAgain is permitted only on CantMeet, whose continuations an owner that waits cannot have.
pub(super) proof fn owner_no_retry(n: Node, nq: u64, q: Question)
    requires n.phase == Phase::AwaitingRob, n.question == Some(q),
        apply_answer_values::permits(q.kind, Answer::TryAgain),
    ensures !(handler_safety::node_rules_v(n, nq) && dependencies::owner_waiting_ok(n)),
{
    if handler_safety::node_rules_v(n, nq) && dependencies::owner_waiting_ok(n) {
        reveal(handler_safety::node_rules_v);
        assert(question_fits(q));
    }
}

/// AddCredit is permitted only on OutOfCredit, whose continuations an owner that waits cannot have.
pub(super) proof fn owner_no_credit(n: Node, nq: u64, q: Question, k: u8)
    requires n.phase == Phase::AwaitingRob, n.question == Some(q),
        apply_answer_values::permits(q.kind, Answer::AddCredit(k)),
    ensures !(handler_safety::node_rules_v(n, nq) && dependencies::owner_waiting_ok(n)),
{
    if handler_safety::node_rules_v(n, nq) && dependencies::owner_waiting_ok(n) {
        reveal(handler_safety::node_rules_v);
        assert(question_fits(q));
    }
}

/// ChangeRequest / Stop: the node stops (no question, Stopping phase).
pub(super) proof fn owner_stopped(n: Node, id: u64, answer: Answer, o: Outcome)
    requires n.phase == Phase::AwaitingRob,
    ensures dependencies::owner_waiting_ok(n) ==> owner_rows(retirement::stopped(n, o), n, id, answer),
{
    reveal(owner_rows);
    reveal(retirement::stopped);
}

/// Cleanup answers (Stopping): Retry drops the question, MarkHandled also advances the teardown, others keep
/// the node; the phase stays Stopping.
pub(super) proof fn owner_cleanup(n: Node, id: u64, answer: Answer)
    requires n.phase is Stopping,
    ensures dependencies::owner_waiting_ok(n) ==> owner_rows(Node { question: None, ..n }, n, id, answer)
            && owner_rows(handler_safety::handled_node(Node { question: None, ..n }), n, id, answer)
            && owner_rows(n, n, id, answer),
{
    reveal(owner_rows);
}

/// Aggregate of the AwaitingRob arms over awaiting_arms (rule 6).
pub(super) proof fn awaiting_owner(n: Node, nq: u64, id: u64, answer: Answer, q: Question)
    requires n.phase == Phase::AwaitingRob, n.question == Some(q), q.id == id,
    ensures (handler_safety::node_rules_v(n, nq) && dependencies::owner_waiting_ok(n))
        ==> owner_rows(apply_answer_values::awaiting_arms(n, nq, answer, q).0, n, id, answer),
{
    if answer == Answer::Unclear {
        owner_unclear(n, nq, id, q);
    } else if !apply_answer_values::permits(q.kind, answer) {
        owner_same(n, id, answer);
    } else {
        match answer {
            Answer::Continue => owner_continue(n, nq, id, q),
            Answer::TryAgain => owner_no_retry(n, nq, q),
            Answer::AddCredit(k) => owner_no_credit(n, nq, q, k),
            Answer::ChangeRequest => owner_stopped(n, id, answer, Outcome::Replaced),
            Answer::Stop => owner_stopped(n, id, answer, Outcome::Stopped),
            _ => owner_same(n, id, answer),
        }
    }
}

/// on_answer's withhold step (rule 7): over on_answer's own fact n == unwithheld(continued, n.credit.withheld);
/// withholding only grows zero_credit.
pub(super) proof fn owner_rows_unwithheld(a: Node, x: bool, b: Node, id: u64, answer: Answer)
    ensures (owner_rows(a, b, id, answer) && (a.credit.withheld ==> x))
        ==> owner_rows(handler_safety::unwithheld(a, x), b, id, answer),
{
    reveal(owner_rows);
    reveal(dependencies::zero_credit);
}

/// E2.N4c in answer_command (ONE rule-7 lemma). Premises: on_answer's atom, answer pair and dependency
/// frame, and record_last_cmd's per-field posts, verbatim.
pub(super) proof fn n4c_after_last_cmd(before: World, b: Node, a: Node, n: Node, question: u64, answer: Answer,
    table: Seq<Dependency>, node: NodeId)
    requires
        (retirement::node_rules(before, b) && dependencies::owner_waiting_ok(b))
            ==> owner_rows(a, b, question, answer),
        handler_safety::answer_pair(b, a, true, question, answer, table, node),
        table == before.dependencies@,
        n.phase == a.phase,
        n.question == a.question,
        n.credit == a.credit,
        n.planning_started == a.planning_started,
        n.holds == a.holds,
        n.pool == a.pool,
        n.ticket == a.ticket,
        n.run == a.run,
    ensures (retirement::node_rules(before, b) && dependencies::owner_waiting_ok(b))
        ==> dependencies::prerequisites_delivered(before, node) || dependencies::owner_waiting_ok(n),
{
    if retirement::node_rules(before, b) && dependencies::owner_waiting_ok(b) {
        reveal(owner_rows);
        reveal(dependencies::zero_credit);
        handler_safety::answer_pair_withheld(b, a, true, question, answer, table, node);
        dependencies::prerequisite_definition(before, node);
    }
}

}
