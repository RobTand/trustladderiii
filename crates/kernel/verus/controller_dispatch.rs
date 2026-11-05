//! #817: actual Wait, EffectKind, classifier and repair decisions.
extern crate alloc;
use vstd::prelude::*;
#[path = "../src/lib.rs"]
mod kernel;
pub use kernel::*;
extern crate self as tl3_kernel;
#[path = "../../controller/src/dispatch_liveness/decision.rs"]
mod decision;
#[path = "../../controller/src/dispatch_liveness/model.rs"]
mod model;

mod dispatch_theorems {
    use super::*;
    use decision::*;
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

    /// Q39 / pausing: unstarted tickets and pauses have no loss action.
    fn no_loss_before_start(e: Evidence) -> (result: Repair)
        requires !e.incident, !e.ending, e.quota_reset.is_none(), e.paused, e.ticket, e.obstruction,
            !e.watch,
        ensures classification(Wait::Resume, e) == DispatchStatus::Paused,
            classification(Wait::AgentPool, e) == DispatchStatus::PoolWait,
            result == Repair::Keep,
    {
        supervisor_ending(false, true)
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

    /// #817 nonvacuity: all six primary outcomes have concrete witnesses.
    fn outcome_witnesses() {
        let mut e = Evidence {
            class: DeliveryClass::Worker, intent: true, watch: true,
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
        e.incident = false; e.paused = true;
        assert(classification(Wait::Resume, e) == DispatchStatus::Paused);
        e.ticket = true; e.obstruction = true;
        assert(classification(Wait::AgentPool, e) == DispatchStatus::PoolWait);
    }
    }
}
fn main() {}
