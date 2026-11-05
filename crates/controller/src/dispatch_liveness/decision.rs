use tl3_kernel::{EffectKind, Wait};
#[cfg(verus_keep_ghost)]
use vstd::prelude::*;

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProcessEvidence {
    Live,
    Ended,
    Unknown,
}

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeliveryClass {
    Worker,
    Once,
    Converges,
    AtLeastOnce,
}

/// State-table delivery evidence belongs to the adapter, not the outbox.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContractEvidence {
    Absent,
    ActionBound,
    RecordedIntent,
    Ending,
}

/// Q53: the classifier retains the durable retry deadline. Role, model, family
/// and the original operation key remain in the quota facts/status view (#828).
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RetryPending {
    Allowance,
    ModelQuota { until: u64 },
}

/// #817 / #828 integration seam: `until` is the persisted quota reset in Unix ms.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(result => ensures
    result == (RetryPending::ModelQuota { until }),
))]
pub fn quota_retry(until: u64) -> RetryPending {
    RetryPending::ModelQuota { until }
}

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DispatchStatus {
    Dispatched(DeliveryClass),
    Ending,
    Retry(RetryPending),
    Incident,
    Paused,
    PoolWait,
    Answer,
    MergeQueue,
    SlotOrFiles,
    Prerequisite,
    Admission,
    Closed,
    Silent,
}

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Evidence {
    pub class: DeliveryClass,
    pub intent: bool,
    pub contract: ContractEvidence,
    pub watch: bool,
    pub process: ProcessEvidence,
    pub ending: bool,
    pub deadline: bool,
    pub quota_reset: Option<u64>,
    pub incident: bool,
    pub paused: bool,
    pub ticket: bool,
    pub obstruction: bool,
    pub question: bool,
    pub external: bool,
    pub closed: bool,
    pub admission: bool,
}

/// State-table effect-delivery classes. Rebase belongs to the worktree adapter.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(result => ensures
    result == delivery_contract(kind),
))]
pub fn delivery_class(kind: EffectKind) -> DeliveryClass {
    match kind {
        EffectKind::StartRun(_) | EffectKind::StopRun => DeliveryClass::Worker,
        EffectKind::SubmitChecks => DeliveryClass::Once,
        EffectKind::PostApproval => DeliveryClass::AtLeastOnce,
        EffectKind::StartRebase
        | EffectKind::PrepareWorktree
        | EffectKind::Push
        | EffectKind::OpenPr
        | EffectKind::Enqueue
        | EffectKind::WithdrawJob
        | EffectKind::ClosePr
        | EffectKind::RemoveWorktree => DeliveryClass::Converges,
    }
}

/// #817 / Q7, Q30, Q39, pause, question and effect-delivery rows.
/// Evidence is for this operation only; historical replies never cover a wait.
/// Silence is explicit input to repair, never a successful reconciliation result.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(result => ensures
    result == classification(wait, e),
))]
pub fn classify(wait: Wait, e: Evidence) -> DispatchStatus {
    if e.incident {
        return DispatchStatus::Incident;
    }
    if let Some(until) = e.quota_reset {
        return DispatchStatus::Retry(quota_retry(until));
    }
    if e.ending {
        return DispatchStatus::Ending;
    }
    match wait {
        Wait::Incident => DispatchStatus::Silent,
        Wait::Resume => {
            let operation = operation_status(e);
            if operation != DispatchStatus::Silent {
                operation
            } else if e.paused {
                DispatchStatus::Paused
            } else {
                DispatchStatus::Silent
            }
        }
        Wait::AgentPool => {
            if e.ticket && e.obstruction {
                DispatchStatus::PoolWait
            } else {
                DispatchStatus::Silent
            }
        }
        Wait::Effect(kind) => {
            let mut effect = e;
            effect.class = delivery_class(kind);
            operation_status(effect)
        }
        Wait::WorkerReply => operation_status(e),
        Wait::CheckResults => {
            let mut checks = e;
            checks.class = DeliveryClass::Once;
            operation_status(checks)
        }
        Wait::Answer => {
            if e.question {
                DispatchStatus::Answer
            } else {
                DispatchStatus::Silent
            }
        }
        Wait::MergeQueue => {
            if e.external {
                DispatchStatus::MergeQueue
            } else {
                DispatchStatus::Silent
            }
        }
        Wait::SlotOrFiles => {
            if e.ticket && e.obstruction {
                DispatchStatus::SlotOrFiles
            } else {
                DispatchStatus::Silent
            }
        }
        Wait::UpstreamEvent => {
            if e.external {
                DispatchStatus::Prerequisite
            } else if e.admission {
                DispatchStatus::Admission
            } else {
                DispatchStatus::Silent
            }
        }
        Wait::None => {
            if e.closed {
                DispatchStatus::Closed
            } else {
                DispatchStatus::Silent
            }
        }
    }
}

/// Once requires the PB action binding. Converging and deduplicated effects
/// require their adapter's own durable intent. An outbox row proves neither.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(result => ensures
    result == contract_covers_spec(class, evidence),
))]
pub fn contract_covers(class: DeliveryClass, evidence: ContractEvidence) -> bool {
    match class {
        DeliveryClass::Worker => false,
        DeliveryClass::Once => evidence == ContractEvidence::ActionBound,
        DeliveryClass::Converges | DeliveryClass::AtLeastOnce => {
            evidence == ContractEvidence::RecordedIntent
        }
    }
}

/// Q7: a proven dead process cannot hide behind its old allowance.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(result => ensures
    result == operation_classification(e),
))]
pub fn operation_status(e: Evidence) -> DispatchStatus {
    if e.class == DeliveryClass::Worker {
        if e.watch && e.process == ProcessEvidence::Live {
            return DispatchStatus::Dispatched(DeliveryClass::Worker);
        }
        if e.process == ProcessEvidence::Ended {
            return DispatchStatus::Silent;
        }
    } else if e.intent && contract_covers(e.class, e.contract) {
        return DispatchStatus::Dispatched(e.class);
    }
    if e.deadline {
        DispatchStatus::Retry(RetryPending::Allowance)
    } else {
        DispatchStatus::Silent
    }
}

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Repair {
    Keep,
    ReadDispatch,
    Restore { watch: bool, allowance: bool },
    MissingIncident,
    LostWorker,
}

/// #816 P2 / #771: only the durable parking marker's clearance enables a scan.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(result => ensures result == !parked,))]
pub fn scan_queued(parked: bool) -> bool {
    !parked
}

/// #816 / Q7: stages preserve effect-read and clock sampling points. There is
/// deliberately no launch or successful-reply action in this decision language.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(result => ensures
    result == if !awaited || covered { Repair::Keep } else { Repair::ReadDispatch },
))]
pub fn preflight(awaited: bool, covered: bool) -> Repair {
    if !awaited || covered {
        Repair::Keep
    } else {
        Repair::ReadDispatch
    }
}

/// #816: immutable original dispatch, original epoch; no second start.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(result => ensures
    result == if matching { Repair::Restore { watch: !watch, allowance: !allowance } }
        else if incident { Repair::Keep } else { Repair::MissingIncident },
))]
pub fn repair_dispatch(matching: bool, watch: bool, allowance: bool, incident: bool) -> Repair {
    if matching {
        Repair::Restore {
            watch: !watch,
            allowance: !allowance,
        }
    } else if incident {
        Repair::Keep
    } else {
        Repair::MissingIncident
    }
}

/// Q7: only durable, fenced absence of an acknowledged operation means loss.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(result => ensures
    result == if acknowledged && absent { Repair::LostWorker } else { Repair::Keep },
))]
pub fn supervisor_ending(acknowledged: bool, absent: bool) -> Repair {
    if acknowledged && absent {
        Repair::LostWorker
    } else {
        Repair::Keep
    }
}

#[cfg(verus_keep_ghost)]
verus! {
pub open spec fn contract_covers_spec(class: DeliveryClass, evidence: ContractEvidence) -> bool {
    match class {
        DeliveryClass::Worker => false,
        DeliveryClass::Once => evidence == ContractEvidence::ActionBound,
        DeliveryClass::Converges | DeliveryClass::AtLeastOnce => evidence == ContractEvidence::RecordedIntent,
    }
}
pub open spec fn delivery_contract(kind: EffectKind) -> DeliveryClass {
    match kind {
        EffectKind::StartRun(_) | EffectKind::StopRun => DeliveryClass::Worker,
        EffectKind::SubmitChecks => DeliveryClass::Once,
        EffectKind::PostApproval => DeliveryClass::AtLeastOnce,
        EffectKind::StartRebase | EffectKind::PrepareWorktree | EffectKind::Push
        | EffectKind::OpenPr | EffectKind::Enqueue | EffectKind::WithdrawJob
        | EffectKind::ClosePr | EffectKind::RemoveWorktree => DeliveryClass::Converges,
    }
}
pub open spec fn operation_classification(e: Evidence) -> DispatchStatus {
    if e.class == DeliveryClass::Worker && e.watch && e.process == ProcessEvidence::Live {
        DispatchStatus::Dispatched(DeliveryClass::Worker)
    } else if e.class == DeliveryClass::Worker && e.process == ProcessEvidence::Ended {
        DispatchStatus::Silent
    } else if e.class != DeliveryClass::Worker && e.intent && contract_covers_spec(e.class, e.contract) {
        DispatchStatus::Dispatched(e.class)
    } else if e.deadline { DispatchStatus::Retry(RetryPending::Allowance) } else { DispatchStatus::Silent }
}
pub open spec fn classification(wait: Wait, e: Evidence) -> DispatchStatus {
    if e.incident { DispatchStatus::Incident }
    else if let Some(until) = e.quota_reset { DispatchStatus::Retry(RetryPending::ModelQuota { until }) }
    else if e.ending { DispatchStatus::Ending }
    else { match wait {
        Wait::Incident => DispatchStatus::Silent,
        Wait::Resume => if operation_classification(e) != DispatchStatus::Silent { operation_classification(e) }
            else if e.paused { DispatchStatus::Paused } else { DispatchStatus::Silent },
        Wait::AgentPool => if e.ticket && e.obstruction { DispatchStatus::PoolWait } else { DispatchStatus::Silent },
        Wait::Effect(kind) => operation_classification(Evidence { class: delivery_contract(kind), ..e }),
        Wait::WorkerReply => operation_classification(e),
        Wait::CheckResults => operation_classification(Evidence { class: DeliveryClass::Once, ..e }),
        Wait::Answer => if e.question { DispatchStatus::Answer } else { DispatchStatus::Silent },
        Wait::MergeQueue => if e.external { DispatchStatus::MergeQueue } else { DispatchStatus::Silent },
        Wait::SlotOrFiles => if e.ticket && e.obstruction { DispatchStatus::SlotOrFiles } else { DispatchStatus::Silent },
        Wait::UpstreamEvent => if e.external { DispatchStatus::Prerequisite }
            else if e.admission { DispatchStatus::Admission } else { DispatchStatus::Silent },
        Wait::None => if e.closed { DispatchStatus::Closed } else { DispatchStatus::Silent },
    } }
}
}
