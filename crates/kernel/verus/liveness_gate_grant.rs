//! K5 G22: the harness's visits after the teardown. Younger leaves cannot take the freed
//! slot; node 0 then takes it and leaves the line.
use super::*;

verus! {
/// Harness loop 2 (`for i in 2..4 { .. assert_eq!(w.nodes[i].holds, held); }`), one visit.
pub(super) proof fn after_visit(b: World, a: World, i: int, r: bool, t: u64)
    requires liveness_gate_domain::post_inv(b, t), i == 2 || i == 3,
        liveness_gate_steps::advanced(b, a, i, r),
        liveness_gate_steps::advanced_grants(b, a, i, r),
        liveness_gate_steps::advanced_requested(b, a, i, r),
    ensures liveness_gate_domain::post_inv(a, t),
        a.nodes@[i].holds == b.nodes@[i].holds,
{
    liveness_gate_release::reserved(b, t);
    reveal(liveness_gate_domain::post_inv);
    reveal(liveness_gate_steps::advanced_grants);
    reveal(liveness_gate_steps::advanced_requested);
    liveness_gate_visit::visit_frames(b, a, i, r);
    let f0 = b.nodes@[0].footprint;
    let j = if i == 2 { 3int } else { 2int };
    footprint::storage_frame(b, a, f0, b.nodes@[i].footprint);
    footprint::storage_frame(b, a, f0, b.nodes@[j].footprint);
    assert(a.nodes@[0] == b.nodes@[0]);
    assert(a.nodes@[1] == b.nodes@[1]);
    assert(a.nodes@[j] == b.nodes@[j]);
    if liveness_gate_domain::younger_waiter(b, i, t) {
        assert(b.nodes@[0].ticket.unwrap() < b.nodes@[i].ticket.unwrap());
        assert(!r && a.nodes@[i] == b.nodes@[i]);
        assert(liveness_gate_domain::younger_waiter(a, i, t));
    } else {
        assert(liveness_gate_domain::younger_holder(b, i));
        assert(liveness_gate_domain::younger_holder(a, i));
    }
    assert(liveness_gate_domain::younger_ok(a, i, t));
    assert(liveness_gate_domain::younger_ok(a, j, t));
    internal_safety::holders_update(b, a, i, b.nodes@[0].repo, b.nodes@.len());
}

/// Harness final visit (`advance(&mut w, 0, ..); assert!(w.nodes[0].holds && w.nodes[0].ticket.is_none());
/// require_consistent(&w);`).
pub(super) proof fn waiter_granted(b: World, a: World, r: bool, t: u64)
    requires liveness_gate_domain::post_inv(b, t),
        liveness_gate_steps::advanced(b, a, 0, r),
        liveness_gate_steps::advanced_grants(b, a, 0, r),
    ensures a.nodes@[0].holds, a.nodes@[0].ticket.is_none(), checking::accepted(a),
{
    liveness_gate_release::reserved(b, t);
    reveal(liveness_gate_domain::post_inv);
    reveal(liveness_gate_steps::advanced_grants);
    liveness_gate_visit::visit_frames(b, a, 0, r);
}
}
