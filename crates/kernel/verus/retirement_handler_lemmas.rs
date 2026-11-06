//! #806 / Q50 / Q54: isolate exact handler projections from mutable World proofs.
use super::*;

verus! {
/// Q50: an ignored CI fact has the same node, question and output under either flag.
pub(super) proof fn ci_ignored(n: Node, infra: bool, fast: bool, id: u64, out: Outbox)
    requires n.phase != Phase::Queued && !(n.phase == Phase::Publishing
        && current_item(n) == Some(Item::Do(EffectKind::Enqueue))),
    ensures retirement_runtime::ci_post(n, n, infra, fast, id, id),
        retirement_runtime::ci_output(n, infra, fast, out, out),
        infra ==> n == retirement::fact_node(n, Fact::CiFailed { infra }),
{
    reveal(retirement_runtime::ci_post); reveal(retirement_runtime::ci_output);
    reveal(retirement::fact_node);
}

/// Q54: project W2's actual round tuple and output frame through cancellation.
pub(super) proof fn ci_retry(n: Node, prepared: Node, after: Node, fast: bool,
    id: u64, next: u64, out_before: Outbox, out_after: Outbox)
    requires n.phase == Phase::Queued || (n.phase == Phase::Publishing
            && current_item(n) == Some(Item::Do(EffectKind::Enqueue))),
        prepared == retirement_runtime::ci_prepared(n),
        (after, next) == port_review_retry::round(prepared, fast, id, ReviewStop::CiFailed),
        fast && prepared.round != 0 ==> out_after.len == out_before.len,
    ensures retirement_runtime::ci_post(n, after, false, fast, id, next),
        retirement_runtime::ci_output(n, false, fast, out_before, out_after),
{
    reveal(retirement_runtime::ci_prepared);
    reveal(retirement_runtime::ci_post); reveal(retirement_runtime::ci_output);
}

/// Q50: the infrastructure result is identical before and after enqueue cancellation.
pub(super) proof fn ci_infrastructure(n: Node, prepared: Node, after: Node,
    fast: bool, id: u64, out_before: Outbox, out_after: Outbox)
    requires n.phase == Phase::Queued || (n.phase == Phase::Publishing
            && current_item(n) == Some(Item::Do(EffectKind::Enqueue))),
        prepared == retirement_runtime::ci_prepared(n),
        after == retirement::fact_node(prepared, Fact::CiFailed { infra: true }),
    ensures after == retirement::fact_node(n, Fact::CiFailed { infra: true }),
        retirement_runtime::ci_post(n, after, true, fast, id, id),
        retirement_runtime::ci_output(n, true, fast, out_before, out_after),
{
    reveal(retirement_runtime::ci_prepared);
    facts_ci_failed_lemmas::cancellation_input(n, prepared);
    reveal(retirement_runtime::ci_post); reveal(retirement_runtime::ci_output);
}

/// #771 recheck rows: precompute the existing shape implication outside mutable state.
pub(super) proof fn recheck_shape(n: Node, target: Phase, id: u64)
    requires !(target is Closed),
    ensures settle_shape(n) ==>
        settle_shape(port_review_retry::rechecked(n, target, id).0),
{
    reveal(port_review_retry::recheck_post);
    port_review_retry_lemmas::recheck_shape(n,
        port_review_retry::rechecked(n, target, id).0, target, id,
        port_review_retry::rechecked(n, target, id).1);
}

/// #805 rebase row: precompute the existing distance implication independently.
pub(super) proof fn recheck_progress(n: Node, target: Phase, id: u64)
    ensures ordinary_await(n) && n.phase == Phase::Rebasing && target == Phase::Checking
        ==> distance(port_review_retry::rechecked(n, target, id).0) < distance(n),
{
    reveal(port_review_retry::recheck_post);
    port_review_retry_lemmas::recheck_progress(n,
        port_review_retry::rechecked(n, target, id).0, target, id,
        port_review_retry::rechecked(n, target, id).1);
}
/// #771: changing fields outside the settle-shape tuple preserves it exactly.
pub(super) proof fn shape_frame(before: Node, after: Node)
    requires before.phase == after.phase, before.pos == after.pos,
        before.pool == after.pool, before.holds == after.holds,
    ensures settle_shape(before) == settle_shape(after),
{ reveal(settle_shape); }

/// #771 conflict row: a nonterminal target has no teardown shape obligation.
pub(super) proof fn shape_nonterminal(n: Node)
    requires !(n.phase is Closed || n.phase is Stopping),
    ensures settle_shape(n),
{ reveal(settle_shape); }

}
