//! #806 / #860: bridge W2's admission relation to #860's identical schema table.
use super::*;

verus! {
/// Reply schema rows / #771: W2's accepted_reply and #860's admissible encode the
/// same executable accepts table, so either family's guarantee transfers to the other.
pub(super) proof fn accepted_admissible(w: World, n: Node, reply: Reply)
    ensures port_review_reply::accepted_reply(w, n, reply)
        == port_t860_contracts::admissible(w, n, reply),
{
    reveal(port_review_reply::accepted_reply);
    reveal(port_t860_contracts::admissible);
}

/// Failed-run rows / Q50: under run_failed's run_failures <= 1 precondition, W2's
/// exact failed_run equals #860's failed_node, so the exact result gives failed_post.
pub(super) proof fn failed_bridge(b: Node, a: Node, bo: Outbox, ao: Outbox)
    requires b.run_failures <= 1, a == port_review_retry::failed_run(b),
        retirement::notices_only(bo) ==> retirement::notices_only(ao),
    ensures port_t860_contracts::failed_post(b, a, bo, ao),
{
    reveal(port_t860_contracts::failed_post);
    reveal(port_t860_contracts::failed_node);
    reveal(port_review_retry::failed_run);
}

/// Q30 stopping rows: #860's opaque release equation is W2's stopping_reply node.
pub(super) proof fn stopping_bridge(b: Node, a: Node, generation: u64)
    requires port_t860_contracts::stopping_post(b, a, generation),
    ensures a == port_review_reply::stopping_reply(b, generation),
{
    reveal(port_t860_contracts::stopping_post);
}
}
