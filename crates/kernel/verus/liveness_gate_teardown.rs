//! K5 G22 / R22 (W6 segment): the harness's teardown of the last conflicting holder, composed
//! over E11. `on_command(&mut w, 1, 1, Command::Stop, ..)`, then five rounds of
//! `advance(&mut w, 1, ..)` followed by `on_ack(&mut w, 1, effect.seq, Ack::Confirmed, ..)` when an
//! effect is in flight, then `advance(&mut w, 1, ..)`. Each step lemma also proves the next
//! call's requires (node_headroom, arithmetic_consistent, next_question, scheduling_safe and
//! the advance_pre reserves), so the exec binder is a direct restatement of the calls.
use super::*;

verus! {
/// Node 1 between calls of its teardown, with the reserves the remaining calls need.
pub(super) open spec fn tear_common(w: World) -> bool {
    w.nodes@.len() == 4 && liveness_gate_domain::healthy(w) && w.gates@.len() <= u64::MAX
        && dependencies::prerequisites_delivered(w, NodeId(1))
        && w.nodes@[1].phase == Phase::Stopping(Outcome::Stopped)
        && w.nodes@[1].question.is_none() && !w.nodes@[1].waiting_incident
        && w.nodes@[1].holds == (w.nodes@[1].pos <= 3)
        && arithmetic_consistent(w.nodes@[1]) && node_headroom(w.nodes@[1])
        && w.nodes@[1].generation as int + 4 <= u64::MAX as int
        && w.next_question < u64::MAX
}

/// Before a round's advance (and before the final advance at position 5): no effect in flight.
pub(super) open spec fn tearing(w: World) -> bool {
    tear_common(w) && w.nodes@[1].effect.is_none() && w.nodes@[1].pos <= 5
        && w.nodes@[1].next_seq as int + 8 - w.nodes@[1].pos as int <= u64::MAX as int
}

/// After a round's advance emitted the step's cleanup request, before its acknowledgement.
pub(super) open spec fn acking(w: World) -> bool {
    tear_common(w) && w.nodes@[1].pos < 5
        && w.nodes@[1].next_seq as int + 7 - w.nodes@[1].pos as int <= u64::MAX as int
        && match w.nodes@[1].effect {
            Some(e) => !e.cancelling && transition_safety::teardown_kind(e.kind)
                && current_item(w.nodes@[1]) == Some(Item::Teardown(e.kind)),
            None => false,
        }
}

/// One teardown call changes node 1 only.
pub(super) open spec fn only_blocker(b: World, a: World) -> bool {
    a.nodes@ == b.nodes@.update(1, a.nodes@[1])
        && a.gates@ == b.gates@ && a.file_sets@ == b.file_sets@
        && a.dependencies@ == b.dependencies@ && a.pool_capacity == b.pool_capacity
}

/// One harness round: advance(1), then a Confirmed on_ack exactly when an effect is in flight.
pub(super) open spec fn round(b: World, m: World, a: World, r: bool) -> bool {
    liveness_gate_steps::advanced(b, m, 1, r) && liveness_gate_steps::advanced_teardown(b, m, 1, r)
        && liveness_gate_steps::teardown_requested(b, m, 1)
        && (m.nodes@[1].effect.is_none() ==> a == m)
        && (m.nodes@[1].effect.is_some()
            ==> liveness_gate_steps::acked(m, a, 1, m.nodes@[1].effect.unwrap().seq)
                && liveness_gate_steps::teardown_requested(m, a, 1))
}

/// The Stopping agenda, item by item.
pub(super) proof fn stopping_items(n: Node)
    requires n.phase is Stopping,
    ensures n.pos == 0 ==> current_item(n) == Some(Item::Teardown(EffectKind::StopRun)),
        n.pos == 1 ==> current_item(n) == Some(Item::Teardown(EffectKind::WithdrawJob)),
        n.pos == 2 ==> current_item(n) == Some(Item::Teardown(EffectKind::ClosePr)),
        n.pos == 3 ==> current_item(n) == Some(Item::Release),
        n.pos == 4 ==> current_item(n) == Some(Item::Teardown(EffectKind::RemoveWorktree)),
        n.pos >= 5 ==> current_item(n).is_none(),
{
    let s = items(n.phase);
    assert(s.len() == 5);
    assert(s[0] == Item::Teardown(EffectKind::StopRun));
    assert(s[1] == Item::Teardown(EffectKind::WithdrawJob));
    assert(s[2] == Item::Teardown(EffectKind::ClosePr));
    assert(s[3] == Item::Release);
    assert(s[4] == Item::Teardown(EffectKind::RemoveWorktree));
}

/// C1 acceptance (internal_safety) and the storage facts across a Stop or an acknowledgement.
pub(super) proof fn c1_healthy(b: World, a: World)
    requires liveness_gate_domain::healthy(b), b.nodes@.len() == 4, b.gates@.len() <= u64::MAX,
        only_blocker(b, a),
        b.next_ticket <= a.next_ticket, b.next_question <= a.next_question,
        retirement::node_rules(a, a.nodes@[1]),
        internal_safety::apply_n2_no_grants(b.nodes@[1], a.nodes@[1]),
        a.nodes@[1].repo == b.nodes@[1].repo, a.nodes@[1].footprint == b.nodes@[1].footprint,
        a.nodes@[1].credit.withheld == b.nodes@[1].credit.withheld,
        dependencies::prerequisites_delivered(b, NodeId(1)),
        !(b.nodes@[1].phase is Closed), !(a.nodes@[1].phase is Closed),
        settle_shape(a.nodes@[1]),
    ensures liveness_gate_domain::healthy(a),
{
    internal_safety::apply_step_preserves_accepted_full(b, a, 1);
    reveal(scheduling_safe);
    assert forall|j: int| 0 <= j < b.nodes@.len()
        implies (b.nodes@[j].phase is Closed ==> a.nodes@[j].phase is Closed) by {
        if j != 1 { assert(a.nodes@[j] == b.nodes@[j]); }
    }
    headroom::open_prefix_compare(b.nodes@, a.nodes@, b.nodes@.len());
    assert forall|j: int| 0 <= j < a.nodes@.len()
        implies footprint::valid_footprint(a, a.nodes@[j].footprint)
            && ((a.nodes@[j].holds || a.nodes@[j].pool) ==> !(a.nodes@[j].phase is Closed)) by {
        footprint::storage_frame(b, a, b.nodes@[j].footprint, b.nodes@[j].footprint);
        if j != 1 { assert(a.nodes@[j] == b.nodes@[j]); }
    }
    assert forall|j: int| 0 <= j < a.nodes@.len() implies settle_shape(#[trigger] a.nodes@[j]) by {
        if j != 1 { assert(a.nodes@[j] == b.nodes@[j]); }
    }
}

/// `on_command(&mut w, 1, 1, Command::Stop, ..)`: E11.1 starts the teardown at position 0.
pub(super) proof fn stop_step(b: World, a: World)
    requires b.nodes@.len() == 4, liveness_gate_domain::healthy(b), b.gates@.len() <= u64::MAX,
        liveness_gate_domain::blocker_ok(b),
        liveness_gate_steps::stopped(b, a, 1, 1),
    ensures tearing(a), a.nodes@[1].pos == 0, only_blocker(b, a),
{
    reveal(liveness_gate_steps::stopped);
    checking::accepted_node(b, 1);
    dependencies::prerequisite_definition(b, NodeId(1));
    dependencies::prerequisite_definition(a, NodeId(1));
    c1_healthy(b, a);
}

/// A round's `advance(&mut w, 1, ..)`: E11.3 (a) skip or (c) Release moves on with no effect;
/// (b) issues the step's cleanup request.
pub(super) proof fn advance_step(b: World, a: World, r: bool)
    requires tearing(b), b.nodes@[1].pos < 5,
        liveness_gate_steps::advanced(b, a, 1, r),
        liveness_gate_steps::advanced_teardown(b, a, 1, r),
        liveness_gate_steps::teardown_requested(b, a, 1),
    ensures only_blocker(b, a),
        (a.nodes@[1].effect.is_none() && tearing(a) && a.nodes@[1].pos == b.nodes@[1].pos + 1)
            || (acking(a) && a.nodes@[1].pos == b.nodes@[1].pos),
{
    reveal(liveness_gate_steps::advanced_teardown);
    reveal(liveness_gate_steps::teardown_requested);
    liveness_gate_visit::visit_frames(b, a, 1, r);
    stopping_items(b.nodes@[1]);
    stopping_items(a.nodes@[1]);
    dependencies::prerequisite_definition(b, NodeId(1));
    dependencies::prerequisite_definition(a, NodeId(1));
}

/// A round's `on_ack(&mut w, 1, effect.seq, Ack::Confirmed, ..)`: E11.2 R25 finishes the step.
pub(super) proof fn ack_step(b: World, a: World, seq: u64)
    requires acking(b), seq == b.nodes@[1].effect.unwrap().seq,
        liveness_gate_steps::acked(b, a, 1, seq),
        liveness_gate_steps::teardown_requested(b, a, 1),
    ensures tearing(a), a.nodes@[1].pos == b.nodes@[1].pos + 1, only_blocker(b, a),
{
    reveal(liveness_gate_steps::acked);
    reveal(liveness_gate_steps::teardown_requested);
    checking::accepted_node(b, 1);
    stopping_items(b.nodes@[1]);
    assert(liveness_gate_steps::confirms_teardown(b.nodes@[1], seq));
    assert(a.nodes@ =~= b.nodes@.update(1, a.nodes@[1]));
    dependencies::prerequisite_definition(b, NodeId(1));
    dependencies::prerequisite_definition(a, NodeId(1));
    c1_healthy(b, a);
}

/// One whole harness round moves the teardown one item on.
pub(super) proof fn round_step(b: World, m: World, a: World, r: bool)
    requires tearing(b), b.nodes@[1].pos < 5, round(b, m, a, r),
    ensures tearing(a), a.nodes@[1].pos == b.nodes@[1].pos + 1, only_blocker(b, a),
{
    advance_step(b, m, r);
    if m.nodes@[1].effect.is_some() {
        ack_step(m, a, m.nodes@[1].effect.unwrap().seq);
        assert(a.nodes@ =~= b.nodes@.update(1, a.nodes@[1]));
    }
}

/// The last `advance(&mut w, 1, ..)`: E11.3 (d) closes node 1, which no longer holds its slot.
pub(super) proof fn close_step(b: World, a: World, r: bool)
    requires tearing(b), b.nodes@[1].pos == 5,
        liveness_gate_steps::advanced(b, a, 1, r),
        liveness_gate_steps::advanced_teardown(b, a, 1, r),
    ensures only_blocker(b, a), liveness_gate_domain::healthy(a),
        a.nodes@[1].phase is Closed, !a.nodes@[1].holds,
{
    reveal(liveness_gate_steps::advanced_teardown);
    liveness_gate_visit::visit_frames(b, a, 1, r);
    stopping_items(b.nodes@[1]);
}

/// R22 for the harness's blocker: Stop, five rounds and a final advance close node 1 and
/// release its slot, changing no other node. No end state is a premise.
pub(super) proof fn blocker_torn_down(w: World, s: World,
    m0: World, a0: World, m1: World, a1: World, m2: World, a2: World,
    m3: World, a3: World, m4: World, a4: World, c: World,
    r0: bool, r1: bool, r2: bool, r3: bool, r4: bool, rc: bool)
    requires w.nodes@.len() == 4, liveness_gate_domain::healthy(w), w.gates@.len() <= u64::MAX,
        liveness_gate_domain::blocker_ok(w),
        liveness_gate_steps::stopped(w, s, 1, 1),
        round(s, m0, a0, r0), round(a0, m1, a1, r1), round(a1, m2, a2, r2),
        round(a2, m3, a3, r3), round(a3, m4, a4, r4),
        liveness_gate_steps::advanced(a4, c, 1, rc),
        liveness_gate_steps::advanced_teardown(a4, c, 1, rc),
    ensures liveness_gate_steps::torn_down(w, c),
{
    stop_step(w, s);
    round_step(s, m0, a0, r0);
    round_step(a0, m1, a1, r1);
    round_step(a1, m2, a2, r2);
    round_step(a2, m3, a3, r3);
    round_step(a3, m4, a4, r4);
    close_step(a4, c, rc);
    assert(c.nodes@ =~= w.nodes@.update(1, c.nodes@[1]));
    reveal(liveness_gate_steps::torn_down);
}
}
