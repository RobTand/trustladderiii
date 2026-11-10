//! Derive the waiting-owner rows used by operator-command consistency proofs.
use super::*;

verus! {
pub(super) proof fn waiting_owner_ok(w: World, i: int)
    requires dependencies::rules_ok(w), 0 <= i < w.nodes@.len(),
        !dependencies::prerequisites_delivered(w, NodeId(i as u64)),
    ensures dependencies::owner_waiting_ok(w.nodes@[i]),
{
    vstd::std_specs::vec::axiom_spec_len(&w.nodes);
    assert(i <= u64::MAX);
    let node = NodeId(i as u64);
    dependencies::prerequisite_definition(w, node);
    dependencies::delivered_definition(w.dependencies@, node);
    assert(exists|j: int| 0 <= j < w.dependencies@.len()
        && w.dependencies@[j].node == node
        && (w.dependencies@[j].rejected
            || w.dependencies@[j].status != DependencyStatus::Delivered));
    let j = choose|j: int| 0 <= j < w.dependencies@.len()
        && w.dependencies@[j].node == node
        && (w.dependencies@[j].rejected
            || w.dependencies@[j].status != DependencyStatus::Delivered);
    dependencies::rules_definition(w);
    assert(dependencies::row_ok(w, w.dependencies@[j]));
    assert(w.dependencies@[j].node.0 as int == i);
}
}
