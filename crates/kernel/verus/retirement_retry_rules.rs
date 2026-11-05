//! Q50/Q54 / #806: independently gated retry obligations over the same real apply.
use super::retirement_retry::*;
use super::*;

verus! {
/// State-table consistency: ticket and question allocation only enlarge old ID bounds.
pub(super) proof fn question_frame(before: World, after: World, n: Node)
    requires before.gates@ == after.gates@, before.file_sets@ == after.file_sets@,
        before.next_ticket <= after.next_ticket, before.next_question <= after.next_question,
        retirement::node_rules(before, n),
    ensures retirement::node_rules(after, n),
{
    reveal(retirement::node_rules);
    footprint::storage_frame(before, after, n.footprint, n.footprint);
}
}

verus! {
/// Q54 / #806: the real failed-fix ladder preserves every production node rule.
/// One branch, no unwind. The old phase can be either CI-accepting phase.
pub(super) proof fn rung_rules(w: World, n: Node, fast: bool, id: u64)
    requires retirement::node_rules(w, n), node_headroom(n),
        matches!(n.phase, Phase::Publishing | Phase::Queued),
        rung_asks(n, fast) ==> id < w.next_question,
    ensures retirement::node_rules(w, rung(n, fast, id, ReviewStop::CiFailed)),
{
    reveal(retirement::node_rules); reveal(rung); reveal(retirement::stopped);
}
}

verus! {
/// Q54 / #806: a free fix preserves readiness without consuming credit.
pub(super) proof fn failed_rules(w: World, n: Node, fast: bool, id: u64)
    requires retirement::node_rules(w, n), node_headroom(n),
        matches!(n.phase, Phase::Publishing | Phase::Queued),
        n.round != 0 && rung_asks(n, fast) ==> id < w.next_question,
    ensures retirement::node_rules(w, failed(n, fast, id, ReviewStop::CiFailed)),
{
    reveal(failed);
    if n.round == 0 { reveal(retirement::node_rules); }
    else { rung_rules(w, n, fast, id); }
}
}

verus! {
/// Q50 / #806: a changed observed head preserves full node consistency after recheck.
pub(super) proof fn outside_rules(w: World, n: Node, head: HeadId, id: u64)
    requires retirement::node_rules(w, n), node_headroom(n),
        n.had_pr && matches!(n.phase, Phase::Publishing | Phase::Queued | Phase::Ready(Resume::Publishing))
            && n.rechecks >= FREE_RECHECKS && !can_pay(n) ==> id < w.next_question,
    ensures retirement::node_rules(w, fact_result(n, Fact::PushByOther { head }, w.fail_fast, id).0),
{
    reveal(retirement::node_rules); reveal(fact_result); reveal(rechecked); reveal(recheck_core); reveal(continued);
}
}
