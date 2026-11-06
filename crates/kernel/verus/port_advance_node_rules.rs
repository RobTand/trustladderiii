//! #806: local consistency follows from exact contracts, without executing steps again.
use super::port_advance_contracts::*;
use super::*;

verus! {
/// #806: the immutable checker environment and exact global ticket allocation.
pub(super) open spec fn environment(w: World, v: World, n: Node, c: AdvanceContext) -> bool {
    retirement::node_rules(w, n) && advance_pre(n, w.next_ticket, 0)
        && v.gates@ == w.gates@ && v.file_sets@ == w.file_sets@
        && v.next_question == w.next_question
        && v.next_ticket as int == w.next_ticket as int + if takes_ticket(n) { 1int } else { 0int }
}

/// #806: increasing the global ticket counter preserves an untouched node's rules.
pub(super) proof fn environment_frame(w: World, v: World, n: Node)
    requires retirement::node_rules(w, n), v.gates@ == w.gates@,
        v.file_sets@ == w.file_sets@, v.next_question == w.next_question,
        v.next_ticket >= w.next_ticket,
    ensures retirement::node_rules(v, n),
{
    reveal(retirement::node_rules); reveal(node_checked);
    footprint::storage_frame(w, v, n.footprint, n.footprint);
}

/// State-table blocked row / #806: preserve the complete per-node checker predicate.
pub(super) proof fn blocked_rules(w: World, v: World, n: Node, c: AdvanceContext)
    requires environment(w, v, n, c), n.phase is Closed || n.waiting_incident || n.effect.is_some(),
    ensures retirement::node_rules(v, node_after(n, w.next_ticket, c)),
{
    reveal(node_after); reveal(retirement::node_rules); reveal(node_checked); reveal(ready_checked);
    footprint::storage_frame(w, v, n.footprint, n.footprint);
}

/// State-table finished row / #806: preserve the complete per-node checker predicate.
pub(super) proof fn finished_rules(w: World, v: World, n: Node, c: AdvanceContext)
    requires environment(w, v, n, c), !(n.phase is Closed), !n.waiting_incident, n.effect.is_none(),
        current_item(n).is_none(),
    ensures retirement::node_rules(v, node_after(n, w.next_ticket, c)),
{
    reveal(node_after); reveal(retirement::node_rules); reveal(node_checked); reveal(ready_checked);
    footprint::storage_frame(w, v, n.footprint, n.footprint);
}

/// State-table do row / #806: preserve the complete per-node checker predicate.
pub(super) proof fn do_rules(w: World, v: World, n: Node, c: AdvanceContext)
    requires environment(w, v, n, c), !(n.phase is Closed), !n.waiting_incident, n.effect.is_none(),
        matches!(current_item(n), Some(Item::Do(_))),
    ensures retirement::node_rules(v, node_after(n, w.next_ticket, c)),
{
    reveal(node_after); reveal(retirement::node_rules); reveal(node_checked); reveal(ready_checked);
    footprint::storage_frame(w, v, n.footprint, n.footprint);
}

/// State-table start row / #806: preserve the complete per-node checker predicate.
pub(super) proof fn start_rules(w: World, v: World, n: Node, c: AdvanceContext)
    requires environment(w, v, n, c), !(n.phase is Closed), !n.waiting_incident, n.effect.is_none(),
        matches!(current_item(n), Some(Item::Start(_))),
    ensures retirement::node_rules(v, node_after(n, w.next_ticket, c)),
{
    reveal(node_after); reveal(retirement::node_rules); reveal(node_checked); reveal(ready_checked);
    footprint::storage_frame(w, v, n.footprint, n.footprint);
}

/// State-table admit row / #806: preserve the complete per-node checker predicate.
pub(super) proof fn admit_rules(w: World, v: World, n: Node, c: AdvanceContext)
    requires environment(w, v, n, c), !(n.phase is Closed), !n.waiting_incident, n.effect.is_none(),
        current_item(n) == Some(Item::Admit),
    ensures retirement::node_rules(v, node_after(n, w.next_ticket, c)),
{
    reveal(node_after); reveal(retirement::node_rules); reveal(node_checked); reveal(ready_checked);
    footprint::storage_frame(w, v, n.footprint, n.footprint);
}

/// State-table slot row / #806: preserve the complete per-node checker predicate.
pub(super) proof fn slot_rules(w: World, v: World, n: Node, c: AdvanceContext)
    requires environment(w, v, n, c), !(n.phase is Closed), !n.waiting_incident, n.effect.is_none(),
        current_item(n) == Some(Item::Slot),
    ensures retirement::node_rules(v, node_after(n, w.next_ticket, c)),
{
    reveal(node_after); reveal(retirement::node_rules); reveal(node_checked); reveal(ready_checked);
    footprint::storage_frame(w, v, n.footprint, n.footprint);
}

/// State-table checks row / #806: preserve the complete per-node checker predicate.
pub(super) proof fn checks_rules(w: World, v: World, n: Node, c: AdvanceContext)
    requires environment(w, v, n, c), !(n.phase is Closed), !n.waiting_incident, n.effect.is_none(),
        current_item(n) == Some(Item::StartChecks),
    ensures retirement::node_rules(v, node_after(n, w.next_ticket, c)),
{
    reveal(node_after); reveal(retirement::node_rules); reveal(node_checked); reveal(ready_checked);
    footprint::storage_frame(w, v, n.footprint, n.footprint);
}

/// State-table rebase row / #806: preserve the complete per-node checker predicate.
pub(super) proof fn rebase_rules(w: World, v: World, n: Node, c: AdvanceContext)
    requires environment(w, v, n, c), !(n.phase is Closed), !n.waiting_incident, n.effect.is_none(),
        current_item(n) == Some(Item::StartRebase),
    ensures retirement::node_rules(v, node_after(n, w.next_ticket, c)),
{
    reveal(node_after); reveal(retirement::node_rules); reveal(node_checked); reveal(ready_checked);
    footprint::storage_frame(w, v, n.footprint, n.footprint);
}

/// State-table release row / #806: preserve the complete per-node checker predicate.
pub(super) proof fn release_rules(w: World, v: World, n: Node, c: AdvanceContext)
    requires environment(w, v, n, c), !(n.phase is Closed), !n.waiting_incident, n.effect.is_none(),
        current_item(n) == Some(Item::Release),
    ensures retirement::node_rules(v, node_after(n, w.next_ticket, c)),
{
    reveal(node_after); reveal(retirement::node_rules); reveal(node_checked); reveal(ready_checked);
    footprint::storage_frame(w, v, n.footprint, n.footprint);
}

/// State-table teardown wait row / #806: preserve the complete per-node checker predicate.
pub(super) proof fn teardown_wait_rules(w: World, v: World, n: Node, c: AdvanceContext)
    requires environment(w, v, n, c), !(n.phase is Closed), !n.waiting_incident, n.effect.is_none(),
        matches!(current_item(n), Some(Item::Teardown(_))),
        n.question.is_some(),
    ensures retirement::node_rules(v, node_after(n, w.next_ticket, c)),
{
    reveal(node_after); reveal(retirement::node_rules); reveal(node_checked); reveal(ready_checked);
    footprint::storage_frame(w, v, n.footprint, n.footprint);
}

/// State-table teardown gone row / #806: preserve the complete per-node checker predicate.
pub(super) proof fn teardown_gone_rules(w: World, v: World, n: Node, c: AdvanceContext)
    requires environment(w, v, n, c), !(n.phase is Closed), !n.waiting_incident, n.effect.is_none(),
        matches!(current_item(n), Some(Item::Teardown(_))),
        n.question.is_none() && match current_item(n) { Some(Item::Teardown(k)) => gone(n, k), _ => false },
    ensures retirement::node_rules(v, node_after(n, w.next_ticket, c)),
{
    reveal(node_after); reveal(retirement::node_rules); reveal(node_checked); reveal(ready_checked);
    footprint::storage_frame(w, v, n.footprint, n.footprint);
}

/// State-table teardown emit row / #806: preserve the complete per-node checker predicate.
pub(super) proof fn teardown_emit_rules(w: World, v: World, n: Node, c: AdvanceContext)
    requires environment(w, v, n, c), !(n.phase is Closed), !n.waiting_incident, n.effect.is_none(),
        matches!(current_item(n), Some(Item::Teardown(_))),
        n.question.is_none() && match current_item(n) { Some(Item::Teardown(k)) => !gone(n, k), _ => false },
    ensures retirement::node_rules(v, node_after(n, w.next_ticket, c)),
{
    reveal(node_after); reveal(retirement::node_rules); reveal(node_checked); reveal(ready_checked);
    footprint::storage_frame(w, v, n.footprint, n.footprint);
}

/// State-table teardown row / #806: preserve the complete per-node checker predicate.
pub(super) proof fn teardown_rules(w: World, v: World, n: Node, c: AdvanceContext)
    requires environment(w, v, n, c), !(n.phase is Closed), !n.waiting_incident, n.effect.is_none(),
        matches!(current_item(n), Some(Item::Teardown(_))),
    ensures retirement::node_rules(v, node_after(n, w.next_ticket, c)),
{
    if n.question.is_some() { teardown_wait_rules(w, v, n, c); }
    else { match current_item(n) {
        Some(Item::Teardown(k)) => if gone(n, k) { teardown_gone_rules(w, v, n, c); }
            else { teardown_emit_rules(w, v, n, c); },
        _ => {},
    } }
}

/// State-table idle row / #806: preserve the complete per-node checker predicate.
pub(super) proof fn idle_rules(w: World, v: World, n: Node, c: AdvanceContext)
    requires environment(w, v, n, c), !(n.phase is Closed), !n.waiting_incident, n.effect.is_none(),
        matches!(current_item(n), Some(Item::Await | Item::Merge | Item::Answer)),
    ensures retirement::node_rules(v, node_after(n, w.next_ticket, c)),
{
    reveal(node_after); reveal(retirement::node_rules); reveal(node_checked); reveal(ready_checked);
    footprint::storage_frame(w, v, n.footprint, n.footprint);
}

/// #806: one parametric local consistency lemma composed from the exact step relation.
pub(super) proof fn node_rules(w: World, v: World, n: Node, c: AdvanceContext)
    requires environment(w, v, n, c),
    ensures retirement::node_rules(v, node_after(n, w.next_ticket, c)),
{
    if n.phase is Closed || n.waiting_incident || n.effect.is_some() { blocked_rules(w, v, n, c); }
    else { match current_item(n) {
        None => finished_rules(w, v, n, c),
        Some(Item::Do(_)) => do_rules(w, v, n, c),
        Some(Item::Start(_)) => start_rules(w, v, n, c),
        Some(Item::Admit) => admit_rules(w, v, n, c),
        Some(Item::Slot) => slot_rules(w, v, n, c),
        Some(Item::StartChecks) => checks_rules(w, v, n, c),
        Some(Item::StartRebase) => rebase_rules(w, v, n, c),
        Some(Item::Release) => release_rules(w, v, n, c),
        Some(Item::Teardown(_)) => teardown_rules(w, v, n, c),
        Some(Item::Await | Item::Merge | Item::Answer) => idle_rules(w, v, n, c),
    } }
}
}
