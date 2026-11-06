//! #806: keep successful-selector admission reasoning out of the executable dispatcher.
use super::*;

verus! {
/// Q30 / #805: every consistent selected successful reply passes actual admission.
pub(super) proof fn progress_admitted(w: World, n: Node, generation: u64, reply: Reply)
    requires footprint::valid_footprint(w, n.footprint), node_headroom(n), arithmetic_consistent(n),
    ensures n.generation as int + 2 <= u64::MAX as int,
        n.run_failures <= 1, n.run_incidents <= 2,
        (n.phase is Closed ==> !port_review_reply::active(n, generation)),
        (node_checked(w, n) && generation == n.generation && successful_reply_input(n, reply))
        ==> !(n.phase is Stopping) && port_review_reply::active(n, generation)
            && port_review_reply::accepted_reply(w, n, reply),
{
    reveal(port_review_reply::accepted_reply);
    if successful_reply_input(n, reply) {
        if let Reply::PlanLeaf { footprint, .. } = reply {
            if footprint::footprint_empty(n.footprint) { footprint::inline_is_valid(w, 1); }
        }
    }
}
}
