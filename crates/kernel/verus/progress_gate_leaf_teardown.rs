//! State-table Stop and successful cleanup / #806: the last blocker releases.
use super::progress_gate_leaf_state::*;
use super::*;

verus! {
/// #806: a node-local cleanup which adds no holder, pool user or ticket preserves
/// the global checker once its changed node passes the production node rules.
pub(super) proof fn removal_safe(before: World, after: World, i: int)
    requires domain(before), retirement_wide::accepted(before), 0 <= i < 4,
        frame(before, after, i), retirement::node_rules(after, after.nodes@[i]),
        !after.nodes@[i].credit.withheld,
        after.nodes@[i].repo == before.nodes@[i].repo,
        after.nodes@[i].footprint == before.nodes@[i].footprint,
        after.nodes@[i].holds ==> before.nodes@[i].holds,
        after.nodes@[i].pool ==> before.nodes@[i].pool,
        after.nodes@[i].ticket.is_some() ==> after.nodes@[i].ticket == before.nodes@[i].ticket,
    ensures domain(after), retirement_wide::accepted(after),
{
    reveal(retirement_wide::accepted); reveal_with_fuel(potential_prefix, 5);
    assert forall|j: int| 0 <= j < 4 implies retirement::node_rules(after, after.nodes@[j]) by {
        if j != i { retirement_frames::node_frame(before, after, before.nodes@[j]); }
    }
    assert forall|a: int, b: int| 0 <= a < b < 4 implies retirement_wide::pair(after, a, b) by {
        footprint::storage_frame(before, after, before.nodes@[a].footprint, before.nodes@[b].footprint);
        assert(retirement_wide::pair(before, a, b));
    }
}

/// Stop row / #806: the original Queued blocker enters Stopped teardown exactly.
pub(super) proof fn stop_safe(before: World, after: World)
    requires domain(before), retirement_wide::accepted(before),
        before.nodes@[1].phase == Phase::Queued, before.nodes@[1].effect.is_none(),
        frame(before, after, 1), after.nodes@[1] == retirement::stopped(
            Node { last_cmd: 1, ..before.nodes@[1] }, Outcome::Stopped),
    ensures domain(after), retirement_wide::accepted(after),
{
    reveal(retirement_wide::accepted); reveal(retirement::stopped); reveal(retirement::node_rules);
    retirement_frames::node_frame(before, after, before.nodes@[1]);
    removal_safe(before, after, 1);
}

/// Ack row / #806: successful matching cleanup preserves the full checker.
pub(super) proof fn ack_safe(before: World, after: World)
    requires domain(before), retirement_wide::accepted(before),
        progress_gate_leaf_steps::cleanup_ack_input(before.nodes@[1], before.nodes@[1].effect.unwrap().seq),
        frame(before, after, 1), after.nodes@[1] == progress_gate_leaf_steps::cleanup_ack(before.nodes@[1]),
    ensures domain(after), retirement_wide::accepted(after),
{
    reveal(progress_gate_leaf_steps::cleanup_ack); reveal(progress_gate_leaf_steps::cleanup_confirmed);
    reveal(retirement_wide::accepted); reveal(retirement::node_rules);
    retirement_frames::node_frame(before, after, before.nodes@[1]);
    removal_safe(before, after, 1);
}
}

kernel_fn! {
/// #806: one real advance and any Confirmed ack consumes one of five cleanup items.
pub(super) fn cleanup_round_exec(mut w: World) -> (result: World) [
    requires domain(w), retirement_wide::accepted(w), low(w), storage_ordered(w),
        tracked(w.nodes@[1]), w.nodes@[1].phase == Phase::Stopping(Outcome::Stopped),
        w.nodes@[1].pos < 5, w.nodes@[1].generation <= 32, w.nodes@[1].next_seq <= 32,
    ensures domain(result), retirement_wide::accepted(result), storage_ordered(result),
        frame(w, result, 1), tracked(result.nodes@[1]),
        result.nodes@[1].phase == w.nodes@[1].phase, result.nodes@[1].pos == w.nodes@[1].pos + 1,
        result.nodes@[1].generation == w.nodes@[1].generation,
        result.nodes@[1].next_seq <= w.nodes@[1].next_seq + 1,
        result.nodes@[1].credit == w.nodes@[1].credit,
        result.nodes@[1].footprint == w.nodes@[1].footprint,
        result.nodes@[1].holds == (w.nodes@[1].holds && w.nodes@[1].pos != 3),
] proof [
    reveal(world_headroom); reveal_with_fuel(open_prefix, 5);
    reveal(empty_outbox); reveal(storage_ordered); reveal(footprint::ordered_table); reveal(retirement_wide::accepted);
    reveal(retirement::node_rules); reveal(retirement_wide_domain); reveal(retirement_wide_accepted);
    reveal(progress_gate_leaf_steps::cleanup_ack); reveal(progress_gate_leaf_steps::cleanup_confirmed);
] {
    kernel_snapshot! { initial = w }
    w = advance_leaf(w, 1);
    kernel_snapshot! { advanced = w }
    if let Some(effect) = w.nodes[1].effect {
        on_ack(&mut w, 1, effect.seq, Ack::Confirmed, &mut Outbox::new());
        kernel_proof! {
            assert(w.nodes@ =~= advanced@.nodes@.update(1, w.nodes@[1]));
            ack_safe(advanced@, w);
        }
    }
    kernel_proof! {
        progress_gate_leaf_steps::cleanup_round(initial@.nodes@[1], advanced@.nodes@[1], w.nodes@[1]);
    }
    let checked = w.check();
    assert!(checked.is_ok());
    kernel_proof! { assert(w.nodes@ =~= initial@.nodes@.update(1, w.nodes@[1])); }
    w
}
}

kernel_fn! {
/// Stop/teardown / #806: exactly five rounds consume the five real agenda items;
/// a final real advance closes. The original blocker starts with no live effect.
pub(super) fn teardown(mut w: World) -> (result: World) [
    requires domain(w), retirement_wide::accepted(w), low(w), storage_ordered(w),
        w.nodes@[1].phase == Phase::Queued, !w.nodes@[1].paused,
        !w.nodes@[1].waiting_incident, w.nodes@[1].effect.is_none(),
        w.nodes@[1].last_cmd == 0, w.nodes@[1].generation == 0, w.nodes@[1].next_seq == 0,
    ensures domain(result), retirement_wide::accepted(result), low(result), storage_ordered(result),
        frame(w, result, 1), result.nodes@[1].phase == Phase::Closed(Outcome::Stopped),
        !result.nodes@[1].holds, result.nodes@[1].footprint == w.nodes@[1].footprint,
] proof [
    reveal(world_headroom); reveal_with_fuel(open_prefix, 5);
    reveal(empty_outbox); reveal(storage_ordered); reveal(footprint::ordered_table); reveal(retirement_wide::accepted);
    reveal(retirement::node_rules); reveal(retirement::stopped);
] {
    kernel_snapshot! { initial = w }
    on_command(&mut w, 1, 1, Command::Stop, &mut Outbox::new());
    kernel_proof! {
        assert(w.nodes@ =~= initial@.nodes@.update(1, w.nodes@[1]));
        reveal(progress_gate_leaf_steps::stopped_command);
        stop_safe(initial@, w);
    }
    let mut round = 0usize;
    kernel_while! { (round < 5) [
        invariant round <= 5, domain(w), retirement_wide::accepted(w), low(w), storage_ordered(w),
            frame(initial@, w, 1), tracked(w.nodes@[1]),
            w.nodes@[1].phase == Phase::Stopping(Outcome::Stopped), w.nodes@[1].pos == round,
            w.nodes@[1].generation == 1, w.nodes@[1].next_seq <= round,
            w.nodes@[1].credit == initial@.nodes@[1].credit,
            w.nodes@[1].footprint == initial@.nodes@[1].footprint,
        decreases 5 - round,
    ] {
        w = cleanup_round_exec(w);
        kernel_proof! { assert(w.nodes@ =~= initial@.nodes@.update(1, w.nodes@[1])); }
        round += 1;
    } }
    w = advance_leaf(w, 1);
    assert!(w.nodes[1].phase.is_closed());
    kernel_proof! { assert(w.nodes@ =~= initial@.nodes@.update(1, w.nodes@[1])); }
    w
}
}
