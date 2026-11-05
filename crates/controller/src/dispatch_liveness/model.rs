//! #817 reference model. One step is a recovery transaction or process-ending
//! ingress transaction. Its decisions are the production decisions, not copies.
use super::decision::{Evidence, ProcessEvidence, Repair, repair_dispatch, supervisor_ending};
#[cfg(verus_keep_ghost)]
use vstd::prelude::*;

/// Q7 / #816: the immutable start exists, or one visible incident replaces it.
/// The shell first reads that start, then commits the selected repair. A pidfd
/// wake imports loss through existing WorkerFailed ingress before the retry.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(result => ensures
    result.incident || result.ending || result.deadline || result.quota_reset.is_some(),
    result.quota_reset == e.quota_reset,
    e.quota_reset.is_some() ==> result == e,
    result.class == e.class, result.process == e.process,
    result.contract == e.contract, result.intent == e.intent,
    result.process == ProcessEvidence::Ended && e.quota_reset.is_none() ==> result.ending || result.incident,
    result.paused == e.paused, result.ticket == e.ticket,
    result.obstruction == e.obstruction,
))]
pub fn step(mut e: Evidence, matching_dispatch: bool) -> Evidence {
    if e.incident || e.ending || e.quota_reset.is_some() {
        return e;
    }
    let ending = supervisor_ending(matching_dispatch, e.process == ProcessEvidence::Ended);
    if ending == Repair::LostWorker {
        e.ending = true;
        return e;
    }
    match repair_dispatch(matching_dispatch, e.watch, e.deadline, e.incident) {
        Repair::Restore { watch, allowance } => {
            if watch {
                e.watch = true;
            }
            if allowance {
                e.deadline = true;
            }
        }
        Repair::MissingIncident => e.incident = true,
        Repair::Keep | Repair::ReadDispatch | Repair::LostWorker => {}
    }
    e
}
