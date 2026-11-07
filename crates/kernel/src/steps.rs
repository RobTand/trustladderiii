//! Q30/Q39 / #805: real steps bodies, grouped for uncached verification.
use super::*;

kernel_fn! {
/// The kernel: apply one event, then settle.
/// State-table "Counter headroom" / #771: the real step is overflow-safe under
/// the production checker's invariant, normalized storage, and per-step reserves.
/// `step_budget` bounds every kernel-owned counter and preserves command order.
pub fn step(world: World, event: Event) -> (result: StepResult) [
    requires world_checked(world), world_headroom(world), storage_ordered(world),
    ensures report::agrees_except_fact_stops_and_ci_failed(event, world_phases(world),
            world_phases(result.world), report_input(world, event), Seq::empty(), result.report.entries@,
            result.settlement_input@),
        step_budget(world, result.world),
        (progress_input(world, event) ==> progress_measure(result.world) < progress_measure(world)),
] {
    kernel_proof! { reveal(storage_ordered); checked_reserves(world); }
    let (result, _) = step_with_passes(world, event);
    result
}
}

kernel_fn! {
/// `step`, also reporting how many passes settle needed. Tests use it to size
/// the loop bound that Kani's unwinding check then proves.
/// State-table "Counter headroom" / #771: verify the real event-to-settle
/// composition under arithmetic and storage consistency, plus headroom. The
/// progress theorem separately requires command freshness for every node.
pub fn step_with_passes(mut world: World, event: Event) -> (result: (StepResult, u32)) [
    requires step_scope(world),
    ensures report::agrees_except_fact_stops_and_ci_failed(event, world_phases(world),
            world_phases(result.0.world), report_input(world, event), Seq::empty(), result.0.report.entries@,
            result.0.settlement_input@),
        step_budget(world, result.0.world),
        (progress_input(world, event) ==> progress_measure(result.0.world) < progress_measure(world)),
] {
    kernel_snapshot! { before_step = world }
    kernel_proof! { report_projection(world, event); }
    let mut outbox = Outbox::new();
    let mut report = Report::new();
    kernel_proof! { reveal(step_scope); reveal(empty_outbox); reveal(progress_input); }
    apply(&mut world, event, &mut outbox, &mut report);
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
    kernel_snapshot! { after_apply_report = report.entries@ }
    kernel_snapshot! { settlement_input = (phase_view(world.nodes@), report.entries@) }
    let mut out = Vec::new();
    outbox.drain_into(&mut out);
    let passes = settle(&mut world, &mut out, None, &mut report);
    kernel_proof! {
        report_projection(world, event);
        report::lemmas::through_settle(event, phase_view(before_step@.nodes@), phase_view(after_apply@.nodes@),
            phase_view(world.nodes@), report_input(before_step@, event), Seq::empty(), after_apply_report@, report.entries@);
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
    (StepResult { world, outputs: out, report,
        #[cfg(verus_keep_ghost)]
        settlement_input,
    }, passes)
}
}

kernel_fn! {
/// Refuse counter exhaustion before applying an event or settling the world.
#[allow(clippy::result_large_err)]
pub fn step_checked(world: World, event: Event) -> (result: Result<StepResult, StepRefusal>) [
    requires world_checked(world), storage_ordered(world),
    ensures report::counter_exhausted(world_headroom(world), result.is_err()),
        match result { Ok(success) => step_budget(world, success.world)
            && report::agrees_except_fact_stops_and_ci_failed(event, world_phases(world),
                world_phases(success.world), report_input(world, event), Seq::empty(), success.report.entries@,
                success.settlement_input@),
            Err(_) => true },
] {
    kernel_snapshot! { headroom = world_headroom(world) }
    match world.counter_exhaustion() {
        Some(exhaustion) => {
            kernel_proof! { report::lemmas::refusal(headroom@, true); }
            Err(StepRefusal { world, exhaustion })
        }
        None => {
            kernel_proof! { report::lemmas::refusal(headroom@, false); }
            Ok(step(world, event))
        }
    }
}
}
