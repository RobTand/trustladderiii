//! K5 G22: the Kani `gate_leaf_progress_w2/w3` trace, composed over its step relations.
//! w0 -advance(2)-> w1 -advance(3)-> w2 -teardown(1)-> w3 -advance(2)-> w4 -advance(3)-> w5
//! -advance(0)-> w6. Every harness assertion is an ensures clause, in harness order.
use super::*;

verus! {
pub(super) proof fn gate_leaf_progress(t: u64,
    w0: World, w1: World, w2: World, w3: World, w4: World, w5: World, w6: World,
    r1: bool, r2: bool, r4: bool, r5: bool, r6: bool)
    requires liveness_gate_domain::gate_start(w0, t),
        liveness_gate_steps::advanced(w0, w1, 2, r1),
        liveness_gate_steps::advanced_grants(w0, w1, 2, r1),
        liveness_gate_steps::advanced_requested(w0, w1, 2, r1),
        liveness_gate_steps::advanced(w1, w2, 3, r2),
        liveness_gate_steps::advanced_grants(w1, w2, 3, r2),
        liveness_gate_steps::advanced_requested(w1, w2, 3, r2),
        liveness_gate_steps::torn_down(w2, w3),
        liveness_gate_steps::advanced(w3, w4, 2, r4),
        liveness_gate_steps::advanced_grants(w3, w4, 2, r4),
        liveness_gate_steps::advanced_requested(w3, w4, 2, r4),
        liveness_gate_steps::advanced(w4, w5, 3, r5),
        liveness_gate_steps::advanced_grants(w4, w5, 3, r5),
        liveness_gate_steps::advanced_requested(w4, w5, 3, r5),
        liveness_gate_steps::advanced(w5, w6, 0, r6),
        liveness_gate_steps::advanced_grants(w5, w6, 0, r6),
    ensures
        !w0.nodes@[2].holds && w1.nodes@[2].holds
            ==> footprint::disjoint(w1, w1.nodes@[0].footprint, w1.nodes@[2].footprint),
        !w1.nodes@[3].holds && w2.nodes@[3].holds
            ==> footprint::disjoint(w2, w2.nodes@[0].footprint, w2.nodes@[3].footprint),
        w2.nodes@[0].ticket == Some(t),
        w3.nodes@[1].phase is Closed,
        scheduler::slot_spec(w3, 0),
        w4.nodes@[2].holds == w3.nodes@[2].holds,
        w5.nodes@[3].holds == w4.nodes@[3].holds,
        w6.nodes@[0].holds && w6.nodes@[0].ticket.is_none(),
        checking::accepted(w6),
{
    liveness_gate_release::start_inv(w0, t);
    liveness_gate_visit::before_visit(w0, w1, 2, r1, t);
    liveness_gate_visit::before_visit(w1, w2, 3, r2, t);
    liveness_gate_release::released(w2, w3, t);
    liveness_gate_grant::after_visit(w3, w4, 2, r4, t);
    liveness_gate_grant::after_visit(w4, w5, 3, r5, t);
    liveness_gate_grant::waiter_granted(w5, w6, r6, t);
}
}
