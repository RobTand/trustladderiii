//! Maker completion row / #806: real on_reply preserves full consistency and decreases rank.
use super::progress_gate::*;
use super::*;

verus! {
/// #806: MakerDone only updates node0, retaining holds/footprint and dropping its pool run.
proof fn maker_safe(before: World, after: World, head: HeadId)
    requires input(before), before.nodes@[0].phase == Phase::Making, !before.nodes@[0].paused,
        after.nodes@ == before.nodes@.update(0, maker_completed(before.nodes@[0], head)),
        after.gates@ == before.gates@, after.pool_capacity == before.pool_capacity,
        after.file_sets@ == before.file_sets@, after.dependencies@ == before.dependencies@,
        after.next_ticket == before.next_ticket, after.next_question == before.next_question,
    ensures domain(after), accepted(after), world_headroom(after), storage_ordered(after),
{
    reveal(initial_node); reveal(accepted); reveal(retirement::node_rules);
    reveal(world_headroom); reveal(storage_ordered); reveal(footprint::ordered_table);
    reveal_with_fuel(potential_prefix, 5); reveal_with_fuel(open_prefix, 5);
    assert forall|i: int| 0 <= i < 4 implies retirement::node_rules(after, after.nodes@[i]) by {
        footprint::storage_frame(before, after, before.nodes@[i].footprint, before.nodes@[i].footprint);
    }
    assert forall|a: int, b: int| 0 <= a < b < 4 implies pair(after, a, b) by {
        assert(pair(before, a, b));
        footprint::storage_frame(before, after, before.nodes@[a].footprint, before.nodes@[b].footprint);
    }
    assert forall|i: int| 0 <= i < 4 implies node_headroom(after.nodes@[i]) by {}
}
}

kernel_fn! {
/// #806: original maker harness over actual on_reply and complete check, without settling.
pub(super) fn verify_maker(mut w: World) -> (after: World) [
    requires input(w), w.nodes@[0].phase == Phase::Making, !w.nodes@[0].paused,
    ensures progress_measure(w) <= 8000, progress_measure(after) <= 8000,
        domain(after), accepted(after), world_checked(after),
        progress_measure(after) < progress_measure(w),
] {
    kernel_proof! { input_scope(w); input_measure_bound(w); }
    verify_checked(&w);
    let generation = w.nodes[0].generation;
    let head = w.nodes[0].head.unwrap_or(HeadId(0));
    let mut out = Outbox::new();
    kernel_snapshot! { before = w }
    kernel_proof! { world_checked_definition(w); reveal(storage_ordered); }
    on_reply(&mut w, 0, generation, Reply::MakerDone { head }, &mut out);
    kernel_proof! {
        assert(w.nodes@ =~= before@.nodes@.update(0, w.nodes@[0]));
        maker_safe(before@, w, head);
        dependencies::indexable_definition(before@);
        progress::lift_node_progress(before@, w, 0);
    }
    verify_checked(&w);
    w
}
}
