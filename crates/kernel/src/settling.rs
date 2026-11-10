//! Q30/Q39 / #805: real settling bodies, grouped for uncached verification.
use super::*;

kernel_fn! {
/// Takes passes over the nodes until one changes nothing, and returns how many
/// passes changed something. Each internal transition moves a node along a
/// finite chain, so the loop ends. Kani's unwinding check proves it within the
/// bounds.
pub(super) fn settle_proved(w: &mut World, out: &mut Vec<Out>, mut trace: Option<&mut Vec<World>>, report: &mut Report)
    -> (result: u32) [
    requires settle_ready(*old(w)),
    ensures report::settled_forwards(phase_view(old(w).nodes@), phase_view(final(w).nodes@), old(report).entries@, final(report).entries@),
        settle_ready(*final(w)), settle_chain(*old(w), *final(w)),
        forall|i: int| 0 <= i < final(w).nodes@.len() ==> !movable(*final(w), i),
        result as int <= potential_prefix(old(w).nodes@, old(w).nodes@.len(), |n: Node| internal_height(n)),
] {
    kernel_snapshot! { baseline = *w }
    kernel_snapshot! { before_report = report.entries@ }
    kernel_proof! {
        assert forall|j: int| 0 <= j < w.nodes@.len() implies
            report::terminal_phase_frame(w.nodes@[j].phase, w.nodes@[j].phase) by {
            report::lemmas::phase_refl(w.nodes@[j].phase);
        }
    }
    let mut passes: u32 = 0;
    kernel_loop! { [
        invariant before_report@ == old(report).entries@, report.entries@ == before_report@,
            forall|j: int| 0 <= j < w.nodes@.len() ==> report::terminal_phase_frame(baseline@.nodes@[j].phase, w.nodes@[j].phase),
            baseline@ == *old(w), settle_ready(*w), settle_chain(baseline@, *w),
            passes as int + potential_prefix(w.nodes@, w.nodes@.len(), |n: Node| internal_height(n))
                <= potential_prefix(baseline@.nodes@, baseline@.nodes@.len(), |n: Node| internal_height(n)),
            potential_prefix(baseline@.nodes@, baseline@.nodes@.len(), |n: Node| internal_height(n)) <= u32::MAX as int,
        decreases potential_prefix(w.nodes@, w.nodes@.len(), |n: Node| internal_height(n)),
    ] {
        kernel_snapshot! { pass_height = potential_prefix(w.nodes@, w.nodes@.len(), |n: Node| internal_height(n)) }
        let mut changed = false;
        let mut i = 0;
        kernel_while! { (i < w.nodes.len()) [
            invariant before_report@ == old(report).entries@, report.entries@ == before_report@,
                forall|j: int| 0 <= j < w.nodes@.len() ==> report::terminal_phase_frame(baseline@.nodes@[j].phase, w.nodes@[j].phase),
                i <= w.nodes.len(), baseline@ == *old(w),
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
            let moved = advance(w, i, &mut outbox, report);
            kernel_proof! {
                report::lemmas::read_quiet(before_node@.nodes@[i as int].phase,
                    w.nodes@[i as int].phase, before_report@, report.entries@);
                assert forall|j: int| 0 <= j < w.nodes@.len() implies
                    report::terminal_phase_frame(baseline@.nodes@[j].phase, w.nodes@[j].phase) by {
                    if j == i {
                        report::lemmas::phase_trans(baseline@.nodes@[j].phase,
                            before_node@.nodes@[j].phase, w.nodes@[j].phase);
                    }
                }
            }
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
        if !changed {
            kernel_proof! { report::lemmas::settle_result(phase_view(baseline@.nodes@), phase_view(w.nodes@), before_report@, report.entries@); }
            return passes;
        }
        passes += 1;
    } }
}
}

kernel_fn! {
/// K5 DEV ONLY (fv-k5-int-dev-1110), CONTRACTS.md E6: TRUSTED export wrapper. Contract =
/// `settle_proved`'s, verbatim, plus the frozen v2 export clauses (direct). The body only calls the
/// verified `settle_proved`: no new executable behaviour. Never lands; escape_audit FAILS here.
#[cfg_attr(verus_keep_ghost, verifier::external_body)]
pub(super) fn settle(w: &mut World, out: &mut Vec<Out>, trace: Option<&mut Vec<World>>, report: &mut Report)
    -> (result: u32) [
    requires settle_ready(*old(w)),
    ensures report::settled_forwards(phase_view(old(w).nodes@), phase_view(final(w).nodes@), old(report).entries@, final(report).entries@),
        settle_ready(*final(w)), settle_chain(*old(w), *final(w)),
        forall|i: int| 0 <= i < final(w).nodes@.len() ==> !movable(*final(w), i),
        result as int <= potential_prefix(old(w).nodes@, old(w).nodes@.len(), |n: Node| internal_height(n)),
    (checking::accepted(*old(w)) && old(w).gates@.len() <= u64::MAX) ==> checking::accepted(*final(w)),
    // v2.48 (lead's premise sweep, STAGED pending lead ruling): E6.2 / E6.3 stated from an accepted world only.
    checking::accepted(*old(w)) ==> forall|i: int| 0 <= i < final(w).nodes@.len()
        && advance_family_post::ready_unobstructed(#[trigger] final(w).nodes@[i]) && oldest_waiter(*final(w), i)
        ==> !scheduler::slot_spec(*final(w), i),
    checking::accepted(*old(w)) ==> forall|i: int| 0 <= i < old(w).nodes@.len()
        && advance_family_post::ready_unobstructed(#[trigger] old(w).nodes@[i])
        && old(w).nodes@[i].phase == Phase::Ready(Resume::Making)
        ==> (final(w).nodes@[i].phase == old(w).nodes@[i].phase
                && final(w).nodes@[i].ticket == old(w).nodes@[i].ticket)
            || (final(w).nodes@[i].holds && final(w).nodes@[i].ticket.is_none()),
] {
    settle_proved(w, out, trace, report)
}
}
