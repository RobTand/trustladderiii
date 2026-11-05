#[path = "../src/looks.rs"]
mod looks;
fn main() {}
#[cfg(verus_keep_ghost)]
mod witnesses {
    use super::looks::*;
    use vstd::prelude::*;
    verus! {
    /// Q7, state-table.md:270-275,309: all branches have concrete witnesses.
    /// Unbounded function contracts above; these values establish reachability only.
    fn q7_witnesses() {
        let first = counted_failure(0, 0);
        assert(!first.raise && first.failures == 1 && first.incidents == 0);
        let second = counted_failure(first.failures, first.incidents);
        assert(second.raise && second.failures == 0 && second.incidents == 1);
        let later = counted_failure(1, second.incidents);
        assert(later.raise && later.incidents == 2);
        let maximum = counted_failure(1, 255);
        assert(maximum.incidents == 255);
        // Per-effect delivery counting (:309): a different effect stays at its first failure.
        let other_effect = counted_failure(0, 0);
        assert(!other_effect.raise && other_effect.incidents == 0);
        let first_rob = incident_needs_rob(IncidentCause::ReadFailure { incidents: 1 });
        let later_rob = incident_needs_rob(IncidentCause::ReadFailure { incidents: 2 });
        let notice_rob = incident_needs_rob(IncidentCause::NoticeFailure { incidents: 2 });
        let kernel_rob = incident_needs_rob(IncidentCause::Kernel { needs_rob: true });
        assert(!first_rob && later_rob && notice_rob && kernel_rob);
        let clear_first = probe_allowed(true, first_rob);
        let hold_later = probe_allowed(true, later_rob);
        let closed = probe_allowed(false, false);
        assert(clear_first && !hold_later && !closed);
        let refused = incident_action(false, IncidentView { needs_rob: false, refused_command: true, own_effect: true });
        let ordinary = incident_action(false, IncidentView { needs_rob: false, refused_command: false, own_effect: false });
        let maintenance = incident_action(true, IncidentView { needs_rob: false, refused_command: true, own_effect: false });
        assert(refused == IncidentAction::Hold && ordinary == IncidentAction::Continue && maintenance == IncidentAction::Probe);
        let postmortem = incident_for_operation(true);
        let operation = incident_for_operation(false);
        let notice = incident_for_notice(true);
        let effect = incident_for_notice(false);
        assert(!postmortem && operation && !notice && effect);
        let fresh = count_failure(None, 0, 1000);
        let silence = count_failure(Some(0), 999, 1000);
        let boundary = count_failure(Some(0), 1000, 1000);
        let backwards = count_failure(Some(1000), 999, 1000);
        let zero = count_failure(Some(1000), 999, 0);
        assert(fresh && !silence && boundary && !backwards && zero);
        let rearmed = rearm_at(999, 1000);
        let saturated = rearm_at(u64::MAX, 1000);
        assert(rearmed == 1999 && saturated == u64::MAX);
        let moving = stalled(999, 0, 1000);
        let stall = stalled(1000, 0, 1000);
        let requeue = stall_action(StallUpstream::GitHub, true);
        let unfinished = stall_action(StallUpstream::Mergify, false);
        let worker = stall_action(StallUpstream::Workers, false);
        let owner = stall_action(StallUpstream::PrismaBuild, true);
        assert(!moving && stall && requeue == StallAction::Requeue && unfinished == StallAction::Unfinished);
        assert(worker == StallAction::StopWorker && owner == StallAction::Ignore);
    }
    }
}
