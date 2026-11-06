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
}
