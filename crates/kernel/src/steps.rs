//! Q30/Q39 / #805: real steps bodies, grouped for uncached verification.
use super::*;

kernel_fn! {
/// The kernel: apply one event, then settle.
/// State-table "Counter headroom" / #771: the real step is overflow-safe under
/// the production checker's invariant, normalized storage, and per-step reserves.
/// `step_budget` bounds every kernel-owned counter and preserves command order.
pub fn step(world: World, event: Event) -> (result: (World, Vec<Out>)) [
    requires world_checked(world), world_headroom(world), storage_ordered(world),
    ensures step_budget(world, result.0),
        (progress_input(world, event) ==> progress_measure(result.0) < progress_measure(world)),
] {
    kernel_proof! { reveal(storage_ordered); checked_reserves(world); }
    let (world, out, _) = step_with_passes(world, event);
    (world, out)
}
}

kernel_fn! {
/// `step`, also reporting how many passes settle needed. Tests use it to size
/// the loop bound that Kani's unwinding check then proves.
/// State-table "Counter headroom" / #771: verify the real event-to-settle
/// composition under arithmetic and storage consistency, plus headroom. The
/// progress theorem separately requires command freshness for every node.
pub fn step_with_passes(mut world: World, event: Event) -> (result: (World, Vec<Out>, u32)) [
    requires step_scope(world),
    ensures step_budget(world, result.0),
        (progress_input(world, event) ==> progress_measure(result.0) < progress_measure(world)),
] {
    kernel_snapshot! { before_step = world }
    let mut outbox = Outbox::new();
    kernel_proof! { reveal(step_scope); reveal(empty_outbox); reveal(progress_input); }
    apply(&mut world, event, &mut outbox);
    kernel_proof! {
        reveal(world_headroom);
        open_prefix_compare(before_step@.nodes@, world.nodes@, before_step@.nodes@.len());
        open_prefix_monotone(world.nodes@, before_step@.nodes@.len(), world.nodes@.len());
        settle_reserve_totals(world.nodes@);
        open_prefix_monotone(world.nodes@, 0, world.nodes@.len());
        reveal(open_prefix);
        assert forall|j: int| 0 <= j < world.nodes@.len() implies
            world.nodes@[j].generation as int + generation_capacity(world.nodes@[j]) <= u64::MAX as int
                && world.nodes@[j].next_seq as int + effect_capacity(world.nodes@[j]) <= u64::MAX as int
                && world.nodes@[j].credit.earned as int + merge_capacity(world.nodes@[j]) <= u64::MAX as int
        by { local_capacities(world.nodes@[j]); }
        assert(settle_ready(world));
    }
    kernel_snapshot! { after_apply = world }
    let mut out = Vec::new();
    outbox.drain_into(&mut out);
    let passes = settle(&mut world, &mut out, None);
    kernel_proof! {
        reveal(step_budget);
        if progress_input(before_step@, event) {
            progress::settle_measure(after_apply@, world);
        }
        assert forall|j: int| 0 <= j < before_step@.nodes@.len() implies
            world.nodes@[j].generation as int <= before_step@.nodes@[j].generation as int + GENERATION_STEP_BUDGET
                && world.nodes@[j].next_seq as int <= before_step@.nodes@[j].next_seq as int + EFFECT_STEP_BUDGET
                && world.nodes@[j].credit.start as int <= before_step@.nodes@[j].credit.start as int + CREDIT_START_STEP_BUDGET
                && world.nodes@[j].credit.earned as int <= before_step@.nodes@[j].credit.earned as int + CREDIT_EARNED_STEP_BUDGET
                && world.nodes@[j].credit.added as int <= before_step@.nodes@[j].credit.added as int + CREDIT_ADDED_STEP_BUDGET
                && world.nodes@[j].credit.spent as int <= before_step@.nodes@[j].credit.spent as int + CREDIT_SPENT_STEP_BUDGET
        by { local_capacities(after_apply@.nodes@[j]); }
        assert forall|j: int| before_step@.nodes@.len() <= j < world.nodes@.len() implies
            world.nodes@[j].generation <= GENERATION_STEP_BUDGET
                && world.nodes@[j].next_seq <= EFFECT_STEP_BUDGET
                && world.nodes@[j].credit.start <= CREDIT_START_STEP_BUDGET
                && world.nodes@[j].credit.earned <= CREDIT_EARNED_STEP_BUDGET
                && world.nodes@[j].credit.added <= CREDIT_ADDED_STEP_BUDGET
                && world.nodes@[j].credit.spent <= CREDIT_SPENT_STEP_BUDGET
        by { local_capacities(after_apply@.nodes@[j]); }
    }
    (world, out, passes)
}
}
