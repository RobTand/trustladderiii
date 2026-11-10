//! K5 G22: the Kani `gate_leaf_progress_w2/w3` trace, composed over the relations of its real
//! calls. w0 -advance(2)-> w1 -advance(3)-> w2 -on_command(1, Stop)-> s -five rounds of advance(1)
//! and Confirmed on_ack(1)-> a4 -advance(1)-> w3 -advance(2)-> w4 -advance(3)-> w5 -advance(0)-> w6.
//! Every harness assertion is an ensures clause, in harness order.
use super::*;

verus! {
pub(super) proof fn gate_leaf_progress(t: u64,
    w0: World, w1: World, w2: World,
    s: World, m0: World, a0: World, m1: World, a1: World, m2: World, a2: World,
    m3: World, a3: World, m4: World, a4: World,
    w3: World, w4: World, w5: World, w6: World,
    r1: bool, r2: bool, q0: bool, q1: bool, q2: bool, q3: bool, q4: bool, qc: bool,
    r4: bool, r5: bool, r6: bool)
    requires liveness_gate_domain::gate_start(w0, t),
        liveness_gate_steps::advanced(w0, w1, 2, r1),
        liveness_gate_steps::advanced_grants(w0, w1, 2, r1),
        liveness_gate_steps::advanced_slot(w0, w1, 2, r1),
        liveness_gate_steps::advanced(w1, w2, 3, r2),
        liveness_gate_steps::advanced_grants(w1, w2, 3, r2),
        liveness_gate_steps::advanced_slot(w1, w2, 3, r2),
        liveness_gate_steps::stopped(w2, s, 1, 1),
        liveness_gate_teardown::round(s, m0, a0, q0),
        liveness_gate_teardown::round(a0, m1, a1, q1),
        liveness_gate_teardown::round(a1, m2, a2, q2),
        liveness_gate_teardown::round(a2, m3, a3, q3),
        liveness_gate_teardown::round(a3, m4, a4, q4),
        liveness_gate_steps::advanced(a4, w3, 1, qc),
        liveness_gate_steps::advanced_teardown(a4, w3, 1, qc),
        liveness_gate_steps::advanced(w3, w4, 2, r4),
        liveness_gate_steps::advanced_grants(w3, w4, 2, r4),
        liveness_gate_steps::advanced(w4, w5, 3, r5),
        liveness_gate_steps::advanced_grants(w4, w5, 3, r5),
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
    liveness_gate_release::pre_blocker(w2, t);
    liveness_gate_teardown::blocker_torn_down(w2, s, m0, a0, m1, a1, m2, a2, m3, a3, m4, a4, w3,
        q0, q1, q2, q3, q4, qc);
    liveness_gate_release::released(w2, w3, t);
    liveness_gate_grant::after_visit(w3, w4, 2, r4, t);
    liveness_gate_grant::after_visit(w4, w5, 3, r5, t);
    liveness_gate_grant::waiter_granted(w5, w6, r6, t);
}
}
