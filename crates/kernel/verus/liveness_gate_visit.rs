//! K5 G22: one scheduler::advance visit as a step of the gate-leaf trace, and the
//! harness's first younger visits (before the last conflicting holder is torn down).
use super::*;

verus! {
/// One advance keeps acceptance and the storage facts (E5.1 step_rows through the
/// advance_family_frames lift), and changes only node i, keeping its repository and files.
pub(super) proof fn visit_frames(b: World, a: World, i: int, r: bool)
    requires liveness_gate_domain::healthy(b), b.gates@.len() <= u64::MAX,
        b.nodes@.len() <= u64::MAX,
        liveness_gate_steps::advanced(b, a, i, r),
    ensures liveness_gate_domain::healthy(a),
        0 <= i < b.nodes@.len(),
        a.nodes@ == b.nodes@.update(i, a.nodes@[i]),
        a.gates@ == b.gates@,
        a.file_sets@ == b.file_sets@,
        a.dependencies@ == b.dependencies@,
        a.pool_capacity == b.pool_capacity,
        a.next_question == b.next_question,
        b.next_ticket <= a.next_ticket,
        a.nodes@[i].repo == b.nodes@[i].repo,
        a.nodes@[i].footprint == b.nodes@[i].footprint,
        a.nodes@[i].ticket == b.nodes@[i].ticket || a.nodes@[i].ticket.is_none()
            || (b.nodes@[i].ticket.is_none() && a.nodes@[i].ticket == Some(b.next_ticket)),
{
    reveal(liveness_gate_steps::advanced);
    advance_family_frames::step_premises_lift(b, a, i);
    internal_safety::step_preserves_accepted_full(b, a, i);
}

/// The blocker's facts carry across a visit of another node.
pub(super) proof fn blocker_frame(b: World, a: World)
    requires liveness_gate_domain::blocker_ok(b), b.nodes@.len() == 4,
        a.nodes@.len() == 4, a.nodes@[0] == b.nodes@[0], a.nodes@[1] == b.nodes@[1],
        a.next_question == b.next_question, a.dependencies@ == b.dependencies@,
    ensures liveness_gate_domain::blocker_ok(a),
{
    dependencies::prerequisite_definition(b, NodeId(1));
    dependencies::prerequisite_definition(a, NodeId(1));
}

/// Harness loop 1 (`for i in 2..4 { advance(&mut w, i, ..) }`), one visit: a younger leaf
/// granted a slot is disjoint from node 0, and node 0 keeps its ticket.
pub(super) proof fn before_visit(b: World, a: World, i: int, r: bool, t: u64)
    requires liveness_gate_domain::pre_inv(b, t), i == 2 || i == 3,
        liveness_gate_domain::younger_start(b, i, t),
        liveness_gate_steps::advanced(b, a, i, r),
        liveness_gate_steps::advanced_grants(b, a, i, r),
        liveness_gate_steps::advanced_requested(b, a, i, r),
    ensures liveness_gate_domain::pre_inv(a, t),
        a.nodes@[0] == b.nodes@[0],
        a.nodes@[0].ticket == Some(t),
        !b.nodes@[i].holds && a.nodes@[i].holds
            ==> footprint::disjoint(a, a.nodes@[0].footprint, a.nodes@[i].footprint),
        liveness_gate_domain::younger_start(b, 5 - i, t)
            ==> liveness_gate_domain::younger_start(a, 5 - i, t),
{
    reveal(liveness_gate_domain::pre_inv);
    reveal(liveness_gate_steps::advanced_grants);
    reveal(liveness_gate_steps::advanced_requested);
    reveal(advance_post);
    visit_frames(b, a, i, r);
    let f0 = b.nodes@[0].footprint;
    let j = 5 - i;
    footprint::storage_frame(b, a, f0, b.nodes@[i].footprint);
    footprint::storage_frame(b, a, f0, b.nodes@[j].footprint);
    assert(a.nodes@[0] == b.nodes@[0]);
    assert(a.nodes@[1] == b.nodes@[1]);
    assert(a.nodes@[j] == b.nodes@[j]);
    if liveness_gate_domain::younger_waiter(b, i, t) {
        if r {
            assert(b.nodes@[0].ticket.unwrap() < b.nodes@[i].ticket.unwrap());
            assert(footprint::disjoint(b, f0, b.nodes@[i].footprint));
            assert(liveness_gate_domain::granted(a, i, t));
        } else {
            assert(a.nodes@[i] == b.nodes@[i]);
            assert(liveness_gate_domain::younger_waiter(a, i, t));
        }
    } else {
        assert(liveness_gate_domain::parked(b, i, t));
        assert(!r);
        assert(a.nodes@[i] == b.nodes@[i]);
        assert(liveness_gate_domain::parked(a, i, t));
    }
    assert(liveness_gate_domain::younger_live(a, i, t));
    assert(liveness_gate_domain::younger_live(a, j, t));
    blocker_frame(b, a);
}
}
