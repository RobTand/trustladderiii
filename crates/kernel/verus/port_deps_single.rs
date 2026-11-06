//! #806 W7: complete checker equivalence for the original external prerequisite worlds.
use super::*;

verus! {
/// #806 original dependency harnesses: one arbitrary node, external rows and fixed capacities.
pub(super) open spec fn domain(w: World) -> bool {
    w.nodes@.len() == 1 && w.gates@.len() == 1 && w.gates@[0].w > 0 && w.pool_capacity == 1
        && w.file_sets@.len() == 0
        && forall|k: int| 0 <= k < w.dependencies@.len() ==> w.dependencies@[k].node == NodeId(0)
            && w.dependencies@[k].target.request.is_none() && !w.dependencies@[k].rejected
}

/// #806: every production prerequisite-check condition for arbitrary external rows.
#[verifier::opaque]
pub(super) open spec fn dependencies_ok(w: World) -> bool {
    dependencies::declarations_unique(w)
        && (w.nodes@[0].credit.withheld ==> w.dependencies@.len() > 0)
        && forall|k: int| 0 <= k < w.dependencies@.len() ==> row_ok(w, w.dependencies@[k])
}

/// #806 external dependency rows: exact non-graph production checks for one owner.
pub(super) open spec fn row_ok(w: World, d: Dependency) -> bool {
    let n = w.nodes@[0];
            d.target.number > 0
                && (d.status != DependencyStatus::Delivered ==> !n.planning_started
                    && matches!(n.phase, Phase::Ingesting | Phase::Auditing | Phase::Unplanned
                        | Phase::AwaitingRob | Phase::Stopping(_) | Phase::Closed(_))
                    && (match n.question { None => true, Some(q) => matches!(q.cont,
                        Cont::Ingesting | Cont::Auditing | Cont::Unplanned | Cont::Stop(_) | Cont::Nowhere) }))
                && (!dependencies::prerequisites_delivered(w, NodeId(0))
                    && matches!(n.phase, Phase::Ingesting | Phase::Auditing | Phase::Unplanned)
                    ==> dependencies::zero_credit(n.credit))
                && (!dependencies::prerequisites_delivered(w, NodeId(0)) && n.phase == Phase::Unplanned
                    ==> !n.holds && !n.pool && n.ticket.is_none() && n.run.is_none()
                        && dependencies::zero_credit(n.credit))
}

/// #806: complete real checker acceptance, without headroom or small-counter assumptions.
#[verifier::opaque]
pub(super) open spec fn accepted(w: World) -> bool {
    domain(w) && retirement::node_rules(w, w.nodes@[0]) && dependencies_ok(w)
}
}
