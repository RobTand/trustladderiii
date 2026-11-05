//! #817: actual Wait, EffectKind, classifier and repair decisions.
mod dispatch_theorems {
    use crate::{Wait, decision, model};
    use decision::*;
    use vstd::prelude::*;
    verus! {
    /// State-table Q7 / #816. K=1 recovery transaction after the immutable
    /// dispatch read, or one fenced process-ending ingress. No time bound and
    /// no fairness premise. The shell's reads/writes are checked differentially.
    fn silent_operation_is_repaired(e: Evidence, matching: bool) -> (result: DispatchStatus)
        requires !e.incident, !e.ending,
        ensures matches!(result,
            DispatchStatus::Dispatched(_) | DispatchStatus::Ending
                | DispatchStatus::Retry(_) | DispatchStatus::Incident),
    {
        let repaired = model::step(e, matching);
        classify(Wait::WorkerReply, repaired)
    }

    /// Q7: authoritative dead-process evidence cannot be covered by a timer.
    fn dead_process_requires_repair(e: Evidence) -> (result: Repair)
        requires !e.incident, !e.ending, e.quota_reset.is_none(), e.class == DeliveryClass::Worker,
            e.process == ProcessEvidence::Ended,
        ensures classification(Wait::WorkerReply, e) == DispatchStatus::Silent,
            result == Repair::LostWorker,
    {
        supervisor_ending(true, true)
    }

    /// Q39 / pausing: without an acknowledged start there is no loss action,
    /// regardless of other sampled evidence. A blocked ticket remains a ticket.
    fn no_loss_before_start(e: Evidence) -> (result: Repair)
        requires !e.incident, !e.ending, e.quota_reset.is_none(), e.paused, e.ticket, e.obstruction,
            !e.watch,
        ensures classification(Wait::AgentPool, e) == DispatchStatus::PoolWait,
            result == Repair::Keep,
    {
        supervisor_ending(false, true)
    }

    /// Pausing row: an authorized pause with no operation evidence has no
    /// deadline. These extra premises describe an unstarted pause, not a run
    /// whose existing allowance must continue to cover its pool occupancy.
    fn unstarted_pause_is_paused(e: Evidence) -> (result: DispatchStatus)
        requires !e.incident, !e.ending, e.quota_reset.is_none(), e.paused,
            !e.watch, !e.intent, !e.deadline,
        ensures result == DispatchStatus::Paused,
    {
        classify(Wait::Resume, e)
    }

    /// Q39 / Pausing: an existing durable retry still covers a paused pool
    /// occupant. Otherwise its ungranted neighbour would become silent.
    fn paused_operation_retains_retry(e: Evidence) -> (result: DispatchStatus)
        requires !e.incident, !e.ending, e.quota_reset.is_none(), e.paused,
            e.class == DeliveryClass::Worker, e.process == ProcessEvidence::Unknown,
            e.deadline,
        ensures result == DispatchStatus::Retry(RetryPending::Allowance),
    {
        classify(Wait::Resume, e)
    }

    /// State-table delivery classes / #817 review: an immutable intent alone
    /// cannot certify any non-worker operation. Once needs the bound action.
    fn non_worker_needs_contract(e: Evidence) -> (result: DispatchStatus)
        requires !e.incident, !e.ending, e.quota_reset.is_none(),
            e.class != DeliveryClass::Worker, e.contract == ContractEvidence::Absent,
        ensures result != DispatchStatus::Dispatched(e.class),
            result == if e.deadline { DispatchStatus::Retry(RetryPending::Allowance) }
                else { DispatchStatus::Silent },
    {
        operation_status(e)
    }

    /// Once: another delivery class's intent cannot stand in for a PB binding.
    fn once_requires_action_binding(e: Evidence) -> (result: DispatchStatus)
        requires e.class == DeliveryClass::Once,
        ensures result == DispatchStatus::Dispatched(DeliveryClass::Once)
            ==> e.contract == ContractEvidence::ActionBound,
    {
        operation_status(e)
    }

    /// State-table effect delivery / #817: witnesses require actual adapter
    /// evidence; neither a worker process nor another class's receipt suffices.
    fn non_worker_contract_witnesses(e: Evidence) {
        let once = Evidence { class: DeliveryClass::Once, intent: true, contract: ContractEvidence::ActionBound, ..e };
        assert(operation_classification(once) == DispatchStatus::Dispatched(DeliveryClass::Once));
        let converges = Evidence { class: DeliveryClass::Converges, intent: true, contract: ContractEvidence::RecordedIntent, ..e };
        assert(operation_classification(converges) == DispatchStatus::Dispatched(DeliveryClass::Converges));
        let dedup = Evidence { class: DeliveryClass::AtLeastOnce, intent: true, contract: ContractEvidence::RecordedIntent, ..e };
        assert(operation_classification(dedup) == DispatchStatus::Dispatched(DeliveryClass::AtLeastOnce));
    }

    /// #816: all decisions preserve the original operation. No action in the
    /// executable repair language can launch or manufacture a successful reply.
    fn repair_is_total(awaited: bool, covered: bool, matching: bool, watch: bool,
        allowance: bool, incident: bool)
    {
        let first = preflight(awaited, covered);
        assert(first == Repair::Keep || first == Repair::ReadDispatch);
        let second = repair_dispatch(matching, watch, allowance, incident);
        assert(matches!(second, Repair::Keep | Repair::Restore { .. } | Repair::MissingIncident));
        if matching {
            assert(second == Repair::Restore { watch: !watch, allowance: !allowance });
        }
    }

    /// Q53 / #828: a completed quota attempt remains a durable retry, even
    /// with dead-process evidence and a missing watch. No loss or repair occurs.
    fn quota_is_a_retry(e: Evidence, until: u64) -> (result: DispatchStatus)
        requires !e.incident, e.quota_reset == Some(until),
        ensures result == DispatchStatus::Retry(RetryPending::ModelQuota { until }),
    {
        let held = model::step(e, true);
        assert(held == e);
        classify(Wait::WorkerReply, held)
    }

    /// #816 P2 / #771: an arbitrary number of unrelated drives or deadline
    /// wakes cannot enable a scan while the durable parking row remains.
    fn no_poll_when_parked() {
        let first = scan_queued(true);
        let next = scan_queued(true);
        assert(!first && !next);
        let cleared = scan_queued(false);
        assert(cleared);
    }

    /// #817 nonvacuity: all six primary outcomes have concrete witnesses.
    fn outcome_witnesses() {
        let mut e = Evidence {
            class: DeliveryClass::Worker, intent: true, contract: ContractEvidence::Absent, watch: true,
            process: ProcessEvidence::Live, ending: false, deadline: false, quota_reset: None,
            incident: false, paused: false, ticket: false, obstruction: false,
            question: false, external: false, closed: false, admission: false,
        };
        assert(classification(Wait::WorkerReply, e) == DispatchStatus::Dispatched(DeliveryClass::Worker));
        e.ending = true;
        assert(classification(Wait::WorkerReply, e) == DispatchStatus::Ending);
        e.ending = false; e.watch = false; e.process = ProcessEvidence::Unknown; e.deadline = true;
        assert(classification(Wait::WorkerReply, e) == DispatchStatus::Retry(RetryPending::Allowance));
        e.incident = true;
        assert(classification(Wait::Incident, e) == DispatchStatus::Incident);
        e.incident = false; e.paused = true; e.deadline = false;
        assert(classification(Wait::Resume, e) == DispatchStatus::Paused);
        e.ticket = true; e.obstruction = true;
        assert(classification(Wait::AgentPool, e) == DispatchStatus::PoolWait);
    }
    }
}
