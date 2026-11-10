//! K5 G22 / W6: the exec binder. Each function makes the Kani harness's real calls
//! (`advance`, `on_command(.., Command::Stop, ..)`, `on_ack(.., Ack::Confirmed, ..)`, each with a
//! fresh Outbox and Report) and proves the step relation from the callee's exported contract.
//! `gate_leaf_harness` runs `check_gate_leaf_progress`'s whole sequence and asserts each of its
//! assertions where the harness does.
use super::*;

verus! {
/// One node's advance reserves (advance_pre without the outbox and the shared ticket counter).
pub(super) open spec fn node_room(n: Node) -> bool {
    n.generation as int + generation_capacity(n) <= u64::MAX as int
        && n.next_seq as int + effect_capacity(n) <= u64::MAX as int
        && n.credit.earned as int + merge_capacity(n) <= u64::MAX as int
}
}

kernel_fn! {
/// `advance(&mut w, i, &mut Outbox::new(), &mut Report::new())`.
pub(super) fn bind_advance(mut w: World, i: usize) -> (result: (World, bool)) [
    requires i < w.nodes@.len(), scheduling_safe(w), node_room(w.nodes@[i as int]),
        w.next_ticket as int + ticket_capacity(w.nodes@[i as int]) <= u64::MAX as int,
    ensures liveness_gate_steps::advanced(w, result.0, i as int, result.1),
        liveness_gate_steps::advanced_grants(w, result.0, i as int, result.1),
        liveness_gate_steps::advanced_slot(w, result.0, i as int, result.1),
        liveness_gate_steps::advanced_teardown(w, result.0, i as int, result.1),
        node_room(result.0.nodes@[i as int]),
        result.0.next_ticket as int + ticket_capacity(result.0.nodes@[i as int])
            <= w.next_ticket as int + ticket_capacity(w.nodes@[i as int]),
] {
    let mut out = Outbox::new();
    kernel_proof! { assert(out.len == 0) by { reveal(empty_outbox); } }
    let moved = advance(&mut w, i, &mut out, &mut Report::new());
    kernel_proof! {
        reveal(liveness_gate_steps::advanced);
        reveal(liveness_gate_steps::advanced_grants);
        reveal(liveness_gate_steps::advanced_slot);
        reveal(liveness_gate_steps::advanced_teardown);
        reveal(advance_post);
    }
    (w, moved)
}
}

kernel_fn! {
/// `on_command(&mut w, 1, 1, Command::Stop, &mut Outbox::new())`.
pub(super) fn bind_stop(mut w: World) -> (result: World) [
    requires 1 < w.nodes@.len(), node_headroom(w.nodes@[1]), arithmetic_consistent(w.nodes@[1]),
        w.next_question < u64::MAX,
    ensures liveness_gate_steps::stopped(w, result, 1, 1),
] {
    let mut out = Outbox::new();
    kernel_proof! { assert(out.len == 0) by { reveal(empty_outbox); } }
    on_command(&mut w, 1, 1, Command::Stop, &mut out);
    kernel_proof! { reveal(liveness_gate_steps::stopped); }
    w
}
}

kernel_fn! {
/// `on_ack(&mut w, 1, seq, Ack::Confirmed, &mut Outbox::new())`.
pub(super) fn bind_ack(mut w: World, seq: u64) -> (result: World) [
    requires 1 < w.nodes@.len(), node_headroom(w.nodes@[1]), arithmetic_consistent(w.nodes@[1]),
        w.next_question < u64::MAX,
    ensures liveness_gate_steps::acked(w, result, 1, seq),
] {
    let mut out = Outbox::new();
    kernel_proof! { assert(out.len == 0) by { reveal(empty_outbox); } }
    on_ack(&mut w, 1, seq, Ack::Confirmed, &mut out);
    kernel_proof! { reveal(liveness_gate_steps::acked); }
    w
}
}

kernel_fn! {
/// One harness round: `advance(&mut w, 1, ..); if let Some(effect) = w.nodes[1].effect {
/// on_ack(&mut w, 1, effect.seq, Ack::Confirmed, ..) }`.
pub(super) fn bind_round(w: World) -> (result: World) [
    requires liveness_gate_teardown::tearing(w), w.nodes@[1].pos < 5,
    ensures liveness_gate_teardown::tearing(result),
        result.nodes@[1].pos == w.nodes@[1].pos + 1,
        liveness_gate_teardown::only_blocker(w, result),
        result.next_ticket <= w.next_ticket,
] {
    kernel_snapshot! { g = w }
    kernel_proof! {
        local_capacities(w.nodes@[1]);
        liveness_gate_teardown::stopping_items(w.nodes@[1]);
    }
    let (m, moved) = bind_advance(w, 1);
    kernel_proof! { liveness_gate_teardown::advance_step(g@, m, moved); }
    kernel_snapshot! { gm = m }
    let effect = m.nodes[1].effect;
    let a = match effect {
        Some(e) => bind_ack(m, e.seq),
        None => m,
    };
    kernel_proof! {
        if gm@.nodes@[1].effect.is_some() {
            reveal(liveness_gate_steps::acked);
            liveness_gate_teardown::ack_step(gm@, a, gm@.nodes@[1].effect.unwrap().seq);
            assert(a.nodes@ =~= g@.nodes@.update(1, a.nodes@[1]));
        }
    }
    a
}
}

kernel_fn! {
/// The harness's teardown of node 1: Stop, five rounds, then `advance(&mut w, 1, ..)`.
pub(super) fn bind_teardown(w: World) -> (result: World) [
    requires w.nodes@.len() == 4, liveness_gate_domain::healthy(w), w.gates@.len() <= u64::MAX,
        liveness_gate_domain::blocker_ok(w),
    ensures liveness_gate_steps::torn_down(w, result),
        result.nodes@.len() == 4,
        result.nodes@[0] == w.nodes@[0], result.nodes@[2] == w.nodes@[2],
        result.nodes@[3] == w.nodes@[3],
        result.next_ticket <= w.next_ticket,
] {
    kernel_snapshot! { g = w }
    let s = bind_stop(w);
    kernel_proof! { liveness_gate_teardown::stop_step(g@, s); }
    kernel_snapshot! { g0 = s }
    let a0 = bind_round(s);
    kernel_snapshot! { g1 = a0 }
    let a1 = bind_round(a0);
    kernel_snapshot! { g2 = a1 }
    let a2 = bind_round(a1);
    kernel_snapshot! { g3 = a2 }
    let a3 = bind_round(a2);
    kernel_snapshot! { g4 = a3 }
    let a4 = bind_round(a3);
    kernel_snapshot! { g5 = a4 }
    kernel_proof! {
        local_capacities(a4.nodes@[1]);
        liveness_gate_teardown::stopping_items(a4.nodes@[1]);
    }
    let (c, moved) = bind_advance(a4, 1);
    kernel_proof! {
        liveness_gate_teardown::close_step(g5@, c, moved);
        liveness_gate_teardown::chain_torn_down(g@, g0@, g1@, g2@, g3@, g4@, g5@, c);
    }
    c
}
}

kernel_fn! {
/// Kani `check_gate_leaf_progress(W)` for W = 2 and 3 (and every other width), over the real
/// calls, from every world in `gate_start`. Each harness assertion is asserted where the harness
/// asserts it; the final ones are also ensures.
pub(super) fn gate_leaf_harness(w0: World) -> (w6: World) [
    requires liveness_gate_domain::gate_start(w0, w0.nodes@[0].ticket.unwrap()),
        node_room(w0.nodes@[0]), node_room(w0.nodes@[2]), node_room(w0.nodes@[3]),
        w0.next_ticket as int + 16 <= u64::MAX as int,
    ensures w6.nodes@[0].holds, w6.nodes@[0].ticket.is_none(), checking::accepted(w6),
] {
    kernel_snapshot! { t = w0.nodes@[0].ticket.unwrap() }
    kernel_snapshot! { g0 = w0 }
    kernel_proof! {
        liveness_gate_release::start_inv(w0, t@);
        local_capacities(w0.nodes@[2]);
    }
    // Loop 1: for i in 2..4 { advance(&mut w, i, ..) }
    let (w1, r1) = bind_advance(w0, 2);
    kernel_proof! {
        liveness_gate_visit::before_visit(g0@, w1, 2, r1, t@);
        liveness_gate_visit::visit_frames(g0@, w1, 2, r1);
        liveness_gate_release::pre_blocker(w1, t@);
        assert(!g0@.nodes@[2].holds && w1.nodes@[2].holds
            ==> footprint::disjoint(w1, w1.nodes@[0].footprint, w1.nodes@[2].footprint));
        local_capacities(w1.nodes@[3]);
    }
    kernel_snapshot! { g1 = w1 }
    let (w2, r2) = bind_advance(w1, 3);
    kernel_proof! {
        liveness_gate_visit::before_visit(g1@, w2, 3, r2, t@);
        liveness_gate_visit::visit_frames(g1@, w2, 3, r2);
        liveness_gate_release::pre_blocker(w2, t@);
        assert(!g1@.nodes@[3].holds && w2.nodes@[3].holds
            ==> footprint::disjoint(w2, w2.nodes@[0].footprint, w2.nodes@[3].footprint));
        assert(w2.nodes@[0].ticket == Some(t@));
    }
    kernel_snapshot! { g2 = w2 }
    // Successful teardown of the last conflicting holder, through real paths.
    let w3 = bind_teardown(w2);
    kernel_proof! {
        liveness_gate_release::released(g2@, w3, t@);
        liveness_gate_release::post_facts(w3, t@);
        assert(w3.nodes@[1].phase is Closed);
        assert(scheduler::slot_spec(w3, 0));
        local_capacities(w3.nodes@[2]);
    }
    // Loop 2: younger leaves cannot take the newly free slot.
    kernel_snapshot! { g3 = w3 }
    let (w4, r4) = bind_advance(w3, 2);
    kernel_proof! {
        liveness_gate_grant::after_visit(g3@, w4, 2, r4, t@);
        liveness_gate_visit::visit_frames(g3@, w4, 2, r4);
        liveness_gate_release::post_facts(w4, t@);
        assert(w4.nodes@[2].holds == g3@.nodes@[2].holds);
        local_capacities(w4.nodes@[3]);
    }
    kernel_snapshot! { g4 = w4 }
    let (w5, r5) = bind_advance(w4, 3);
    kernel_proof! {
        liveness_gate_grant::after_visit(g4@, w5, 3, r5, t@);
        liveness_gate_visit::visit_frames(g4@, w5, 3, r5);
        liveness_gate_release::post_facts(w5, t@);
        assert(w5.nodes@[3].holds == g4@.nodes@[3].holds);
        local_capacities(w5.nodes@[0]);
    }
    kernel_snapshot! { g5 = w5 }
    let (w6, r6) = bind_advance(w5, 0);
    kernel_proof! {
        liveness_gate_grant::waiter_granted(g5@, w6, r6, t@);
        assert(w6.nodes@[0].holds && w6.nodes@[0].ticket.is_none());
    }
    w6
}
}
