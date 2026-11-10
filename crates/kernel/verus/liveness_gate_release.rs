//! K5 G22: node 0's slot after its last conflicting holder closes (Q48 reservation).
use super::*;

verus! {
/// The harness's starting worlds satisfy the trace invariant.
pub(super) proof fn start_inv(w: World, t: u64)
    requires liveness_gate_domain::gate_start(w, t),
    ensures liveness_gate_domain::pre_inv(w, t),
{
    reveal(liveness_gate_domain::pre_inv);
}

/// Once node 1 is closed, node 0 has a free slot (room in the width, no overlapping holder),
/// no older overlapping waiter, and is the oldest waiter in its repository's line.
pub(super) proof fn reserved(w: World, t: u64)
    requires liveness_gate_domain::post_inv(w, t),
    ensures scheduler::slot_spec(w, 0),
        !scheduler::overlapped_by_older(w, 0, t),
        oldest_waiter(w, 0),
{
    reveal(liveness_gate_domain::post_inv);
    reveal(scheduling_safe);
    reveal(oldest_waiter);
    let repo = w.nodes@[0].repo;
    let f0 = w.nodes@[0].footprint;
    internal_safety::other_holders_bridge(w, w.nodes@.len(), repo, 0);
    assert forall|j: int| 0 <= j < w.nodes@.len() && j != 0 && w.nodes@[j].holds
        && w.nodes@[j].repo == repo
        implies footprint::disjoint(w, w.nodes@[j].footprint, f0) by {
        if j == 2 {
            assert(liveness_gate_domain::younger_holder(w, 2));
        } else {
            assert(j == 3);
            assert(liveness_gate_domain::younger_holder(w, 3));
        }
        footprint::disjoint_symmetric(w, f0, w.nodes@[j].footprint);
    }
    assert(scheduler::slot_spec(w, 0));
    if scheduler::overlapped_by_older(w, 0, t) {
        let k = choose|k: int| 0 <= k < w.nodes@.len() && w.nodes@[k].phase is Ready
            && w.nodes@[k].repo == w.nodes@[0].repo && w.nodes@[k].ticket.is_some()
            && w.nodes@[k].ticket.unwrap() < t
            && !footprint::disjoint(w, w.nodes@[k].footprint, w.nodes@[0].footprint);
        if k == 2 {
            assert(liveness_gate_domain::younger_ok(w, 2, t));
        } else if k == 3 {
            assert(liveness_gate_domain::younger_ok(w, 3, t));
        }
        assert(false);
    }
    assert forall|j: int| 0 <= j < w.nodes@.len() implies
        (if w.nodes@[0].phase is Ready {
            !(w.nodes@[j].phase is Ready) || w.nodes@[j].repo != w.nodes@[0].repo
                || match w.nodes@[j].ticket {
                    None => true, Some(theirs) => theirs >= w.nodes@[0].ticket.unwrap(),
                }
        } else { !pool_precedes(w.nodes@[j], w.nodes@[0]) }) by {
        if j == 2 {
            assert(liveness_gate_domain::younger_ok(w, 2, t));
        } else if j == 3 {
            assert(liveness_gate_domain::younger_ok(w, 3, t));
        }
    }
}

/// Harness teardown result (`assert!(w.nodes[1].phase.is_closed()); assert!(slot_free(&w, 0));`):
/// with node 1 closed and holding nothing, the remaining holders leave node 0 a free slot.
pub(super) proof fn released(m: World, w: World, t: u64)
    requires liveness_gate_domain::pre_inv(m, t), liveness_gate_steps::torn_down(m, w),
    ensures liveness_gate_domain::post_inv(w, t),
        w.nodes@[1].phase is Closed,
        scheduler::slot_spec(w, 0),
{
    reveal(liveness_gate_domain::pre_inv);
    reveal(liveness_gate_domain::post_inv);
    reveal(liveness_gate_steps::torn_down);
    let repo = m.nodes@[0].repo;
    let r = repo.0 as int;
    checking::accepted_definition(m);
    assert(RepoId(r as u64) == repo);
    assert(m.gates@[r].w > 0
        && progress_gate_apply::holders(m, RepoId(r as u64), m.nodes@.len()) <= m.gates@[r].w);
    assert(w.nodes@ =~= m.nodes@.update(1, w.nodes@[1]));
    internal_safety::holders_update(m, w, 1, repo, m.nodes@.len());
    let f0 = m.nodes@[0].footprint;
    footprint::storage_frame(m, w, f0, m.nodes@[2].footprint);
    footprint::storage_frame(m, w, f0, m.nodes@[3].footprint);
    assert(w.nodes@[0] == m.nodes@[0]);
    assert(w.nodes@[2] == m.nodes@[2]);
    assert(w.nodes@[3] == m.nodes@[3]);
    assert(liveness_gate_domain::younger_ok(w, 2, t));
    assert(liveness_gate_domain::younger_ok(w, 3, t));
    assert(liveness_gate_domain::post_inv(w, t));
    reserved(w, t);
}
}
