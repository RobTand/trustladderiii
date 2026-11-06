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

/// #771 / #806 fact rows: project scalar frames from exact inline-arm results.
/// The dispatcher still checks each equation against its real mutable arm.
#[verifier::spinoff_prover]
pub(super) proof fn inline_frame(n: Node, fact: Fact)
    requires node_headroom(n), matches!(fact, Fact::Priority(_) | Fact::PrClosedByPerson
        | Fact::Conflict | Fact::UpstreamStalled),
    ensures (n.phase is Closed ==> retirement::fact_node(n, fact).phase is Closed),
        retirement::fact_node(n, fact).repo == n.repo,
        retirement::fact_node(n, fact).footprint == n.footprint,
        settle_shape(n) ==> settle_shape(retirement::fact_node(n, fact)),
        retirement::fact_node(n, fact).generation as int <= n.generation as int + 1,
        retirement::fact_node(n, fact).next_seq == n.next_seq,
        retirement::fact_node(n, fact).credit.start == n.credit.start,
        retirement::fact_node(n, fact).credit.earned == n.credit.earned,
        retirement::fact_node(n, fact).credit.added == n.credit.added,
        retirement::fact_node(n, fact).credit.spent as int <= n.credit.spent as int + 1,
        retirement::fact_node(n, fact).last_cmd == n.last_cmd,
{
    reveal(retirement::fact_node);
    reveal(retirement::stopped);
    reveal(retirement::incident);
    reveal(settle_shape);
}
}
