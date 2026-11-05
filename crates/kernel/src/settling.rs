//! Q30/Q39 / #805: real settling bodies, grouped for uncached verification.
use super::*;

kernel_fn! {
/// Takes passes over the nodes until one changes nothing, and returns how many
/// passes changed something. Each internal transition moves a node along a
/// finite chain, so the loop ends. Kani's unwinding check proves it within the
/// bounds.
pub(super) fn settle(w: &mut World, out: &mut Vec<Out>, mut trace: Option<&mut Vec<World>>)
    -> (result: u32) [
    requires settle_ready(*old(w)),
    ensures settle_ready(*final(w)), settle_chain(*old(w), *final(w)),
        forall|i: int| 0 <= i < final(w).nodes@.len() ==> !movable(*final(w), i),
        result as int <= potential_prefix(old(w).nodes@, old(w).nodes@.len(), |n: Node| internal_height(n)),
] {
    kernel_snapshot! { baseline = *w }
    let mut passes: u32 = 0;
    kernel_loop! { [
        invariant baseline@ == *old(w), settle_ready(*w), settle_chain(baseline@, *w),
            passes as int + potential_prefix(w.nodes@, w.nodes@.len(), |n: Node| internal_height(n))
                <= potential_prefix(baseline@.nodes@, baseline@.nodes@.len(), |n: Node| internal_height(n)),
            potential_prefix(baseline@.nodes@, baseline@.nodes@.len(), |n: Node| internal_height(n)) <= u32::MAX as int,
        decreases potential_prefix(w.nodes@, w.nodes@.len(), |n: Node| internal_height(n)),
    ] {
        kernel_snapshot! { pass_height = potential_prefix(w.nodes@, w.nodes@.len(), |n: Node| internal_height(n)) }
        let mut changed = false;
        let mut i = 0;
        kernel_while! { (i < w.nodes.len()) [
            invariant i <= w.nodes.len(), baseline@ == *old(w),
                settle_ready(*w), settle_chain(baseline@, *w),
                passes as int + pass_height@ <= potential_prefix(baseline@.nodes@, baseline@.nodes@.len(), |n: Node| internal_height(n)),
                potential_prefix(baseline@.nodes@, baseline@.nodes@.len(), |n: Node| internal_height(n)) <= u32::MAX as int,
                potential_prefix(w.nodes@, w.nodes@.len(), |n: Node| internal_height(n)) <= pass_height@,
                changed ==> potential_prefix(w.nodes@, w.nodes@.len(), |n: Node| internal_height(n)) < pass_height@,
                !changed ==> forall|j: int| 0 <= j < i ==> !movable(*w, j),
            decreases w.nodes.len() - i,
        ] {
            let mut outbox = Outbox::new();
            kernel_proof! {
                reveal(empty_outbox);
                potential_member(w.nodes@, i as int, w.nodes@.len(), |n: Node| ticket_capacity(n));
            }
            kernel_snapshot! { before_node = *w }
            let moved = advance(w, i, &mut outbox);
            kernel_proof! {
                reveal(advance_post);
                if !changed && !moved {
                    assert(w.nodes@ == before_node@.nodes@);
                    assert forall|j: int| 0 <= j <= i implies !movable(*w, j) by {
                        movable_frame(before_node@, *w, j);
                        if j < i { assert(!movable(before_node@, j)); }
                    }
                }
                assert forall|j: int| 0 <= j < w.nodes@.len() implies
                    w.nodes@[j].generation as int + generation_capacity(w.nodes@[j])
                        <= baseline@.nodes@[j].generation as int + generation_capacity(baseline@.nodes@[j])
                    && w.nodes@[j].next_seq as int + effect_capacity(w.nodes@[j])
                        <= baseline@.nodes@[j].next_seq as int + effect_capacity(baseline@.nodes@[j])
                    && w.nodes@[j].credit.earned as int + merge_capacity(w.nodes@[j])
                        <= baseline@.nodes@[j].credit.earned as int + merge_capacity(baseline@.nodes@[j])
                    && w.nodes@[j].last_cmd == baseline@.nodes@[j].last_cmd
                    && w.nodes@[j].credit.start == baseline@.nodes@[j].credit.start
                    && w.nodes@[j].credit.added == baseline@.nodes@[j].credit.added
                    && w.nodes@[j].credit.spent == baseline@.nodes@[j].credit.spent by {
                    if j != i { assert(w.nodes@[j] == before_node@.nodes@[j]); }
                }
            }
            if moved {
                outbox.drain_into(out);
                changed = true;
                if let Some(t) = trace.as_mut() { t.push(snapshot_world(w)); }
            }
            i += 1;
        } }
        if !changed { return passes; }
        passes += 1;
    } }
}
}
