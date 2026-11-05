//! H1/H2/H6 decisions over samples taken by the Mergify adapter.
#[cfg(verus_keep_ghost)]
use vstd::prelude::*;

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Receipt {
    Confirmed,
    Other,
    Pending,
    Absent,
}

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReceiptStage {
    Initial,
    Final,
}

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReceiptAction {
    ReadRecord,
    SaveRecord,
    Return,
    Prepare,
    Unknown,
}

/// State-table Effects, H2: preserve the distinct initial/final receipt reads.
/// Full finite domain; no sequence bound. Only Confirmed can save a receipt.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
    (out == ReceiptAction::ReadRecord) == (stage == ReceiptStage::Initial && read == Receipt::Confirmed && recorded.is_none()),
    (out == ReceiptAction::SaveRecord) == (read == Receipt::Confirmed && (stage == ReceiptStage::Final || recorded == Some(false))),
    (out == ReceiptAction::Prepare) == (stage == ReceiptStage::Initial && read == Receipt::Absent),
    (out == ReceiptAction::Return) == (read == Receipt::Other || stage == ReceiptStage::Initial && read == Receipt::Confirmed && recorded == Some(true)),
    (out == ReceiptAction::Unknown) == (read == Receipt::Pending || stage == ReceiptStage::Final && read == Receipt::Absent),
))]
pub fn receipt(stage: ReceiptStage, read: Receipt, recorded: Option<bool>) -> ReceiptAction {
    match read {
        Receipt::Confirmed => match stage {
            ReceiptStage::Initial => match recorded {
                None => ReceiptAction::ReadRecord,
                Some(false) => ReceiptAction::SaveRecord,
                Some(true) => ReceiptAction::Return,
            },
            ReceiptStage::Final => ReceiptAction::SaveRecord,
        },
        Receipt::Other => ReceiptAction::Return,
        Receipt::Pending => ReceiptAction::Unknown,
        Receipt::Absent => match stage {
            ReceiptStage::Initial => ReceiptAction::Prepare,
            ReceiptStage::Final => ReceiptAction::Unknown,
        },
    }
}

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AdmissionQueue {
    Merged,
    Active,
    Terminal,
    Other,
}

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AckRead {
    pub current: bool,
    pub merged: bool,
    pub conflicts: bool,
    pub confirmed: bool,
    pub person_removed: Option<bool>,
    pub queue: AdmissionQueue,
    pub no_requeue: bool,
    pub check_changed: bool,
}

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AckDecision {
    Retryable,
    Conflict,
    ReadPerson,
    Confirmed,
    ReadIssue,
}

/// Publishing step 4, K-ack-sound and #608 stale-head dispatch guard.
/// Full finite domain, after the unchanged hold/refusal/rerun gates.
/// Conflict confirms reconciliation to carry a Conflict fact, not queue admission.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
    (out == AckDecision::Retryable) == (!read.current && read.queue != AdmissionQueue::Merged && !read.confirmed),
    out == AckDecision::Conflict ==> read.current && !read.merged && read.conflicts,
    out == AckDecision::Confirmed ==> read.person_removed.is_some() && (
        read.confirmed || read.person_removed == Some(true) || read.queue == AdmissionQueue::Merged
        || read.current && (read.queue == AdmissionQueue::Active
            || read.queue == AdmissionQueue::Terminal && read.no_requeue && read.check_changed)),
    read.current && !read.merged && read.conflicts ==> out == AckDecision::Conflict,
    out == AckDecision::ReadPerson ==> read.person_removed.is_none(),
    out == AckDecision::ReadIssue ==> read.person_removed == Some(false) && !read.confirmed && read.queue != AdmissionQueue::Merged
        && (!read.current || read.queue != AdmissionQueue::Active
            && !(read.queue == AdmissionQueue::Terminal && read.no_requeue && read.check_changed)),
))]
pub fn acknowledgement(read: AckRead) -> AckDecision {
    if !read.current && read.queue != AdmissionQueue::Merged && !read.confirmed {
        AckDecision::Retryable
    } else if read.current && !read.merged && read.conflicts {
        AckDecision::Conflict
    } else if read.person_removed.is_none() {
        AckDecision::ReadPerson
    } else if read.confirmed
        || read.person_removed == Some(true)
        || read.queue == AdmissionQueue::Merged
        || read.current
            && (read.queue == AdmissionQueue::Active
                || read.queue == AdmissionQueue::Terminal && read.no_requeue && read.check_changed)
    {
        AckDecision::Confirmed
    } else {
        AckDecision::ReadIssue
    }
}

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DequeueProbe {
    Clear,
    Pending,
    Events,
}

/// H6 / Effects: sample event history only when retrying an unconcluded dequeue.
/// Full Boolean domain; no sequence bound.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
    (out == DequeueProbe::Clear) == (!retry || concluded),
    (out == DequeueProbe::Pending) == (retry && !concluded && !labeled),
    (out == DequeueProbe::Events) == (retry && !concluded && labeled),
))]
pub fn dequeue_probe(retry: bool, concluded: bool, labeled: bool) -> DequeueProbe {
    if !retry || concluded {
        DequeueProbe::Clear
    } else if !labeled {
        DequeueProbe::Pending
    } else {
        DequeueProbe::Events
    }
}

/// H6 / #608: a labeled dequeue must be newer than the latest queued event.
/// Without a queued event, compare the complete timestamp, including nanoseconds.
/// Unbounded in event-history length; IDs and timestamps span their full Rust domains.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
    out == (labeled && match queued {
        Some(before) => id > before,
        None => match finished {
            Some(before) => at.0 > before.0 || at.0 == before.0 && at.1 >= before.1,
            None => false,
        },
    }),
))]
pub fn dequeue_fresh(
    labeled: bool,
    id: u64,
    queued: Option<u64>,
    at: (i64, u32),
    finished: Option<(i64, u32)>,
) -> bool {
    if !labeled {
        return false;
    }
    match queued {
        Some(before) => id > before,
        None => match finished {
            Some(before) => at.0 > before.0 || at.0 == before.0 && at.1 >= before.1,
            None => false,
        },
    }
}

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PersonProbe {
    RepairedHead,
    Events,
    Skip,
}

/// Human dequeue/close rows: an existing baseline suppresses repaired-head reads.
/// Full finite domain; the later snapshot or baseline comparison is separate.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
    (out == PersonProbe::RepairedHead) == (!baseline && repaired.is_none()),
    (out == PersonProbe::Events) == (baseline || repaired == Some(true)),
    (out == PersonProbe::Skip) == (!baseline && repaired == Some(false)),
))]
pub fn person_probe(baseline: bool, repaired: Option<bool>) -> PersonProbe {
    if baseline {
        PersonProbe::Events
    } else {
        match repaired {
            None => PersonProbe::RepairedHead,
            Some(true) => PersonProbe::Events,
            Some(false) => PersonProbe::Skip,
        }
    }
}

/// Human dequeue/close rows: without a snapshot, events pre-dating intent are stale.
/// All u64 event IDs; no history-length bound.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
    out == match baseline { Some(before) => event > before, None => false },
))]
pub fn person_fresh(baseline: Option<u64>, event: u64) -> bool {
    match baseline {
        Some(before) => event > before,
        None => false,
    }
}

/// H2 / #751: capture the latest event from the original issue-events sample.
/// One fold step over all u64 IDs; induction applies to arbitrary history lengths.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
    out >= previous, out >= event, out == previous || out == event,
))]
pub fn latest_event(previous: u64, event: u64) -> u64 {
    if event > previous { event } else { previous }
}

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IntentBoundary {
    Save,
    Fence,
}

/// Effects / #751: the production SaveIntent handler interprets both slots in order.
/// Bound: exactly two boundaries, before the next dispatch/read effect is consumed.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
    out[0] == IntentBoundary::Save, out[1] == IntentBoundary::Fence,
))]
pub fn intent_boundaries() -> [IntentBoundary; 2] {
    [IntentBoundary::Save, IntentBoundary::Fence]
}

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LabelDecision {
    IntentMissing,
    Retryable,
    Closed,
    Write,
}

/// Effects / #751: label dispatch requires an intent and a fresh open approved head.
/// Full Boolean domain; the shell consumes Save and Fence before this pull sample.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
    (out == LabelDecision::IntentMissing) == !intent,
    (out == LabelDecision::Retryable) == (intent && !current),
    (out == LabelDecision::Closed) == (intent && current && (!open || merged)),
    (out == LabelDecision::Write) == (intent && current && open && !merged),
))]
pub fn label(intent: bool, current: bool, open: bool, merged: bool) -> LabelDecision {
    if !intent {
        LabelDecision::IntentMissing
    } else if !current {
        LabelDecision::Retryable
    } else if !open || merged {
        LabelDecision::Closed
    } else {
        LabelDecision::Write
    }
}

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BaselineIntent {
    Label,
    Requeue,
}

/// H2 / #608: the original issue-label sample chooses a fresh intent's mode.
/// Full Boolean domain; an existing intent bypasses construction entirely.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
    (out == BaselineIntent::Requeue) == queue_label,
    (out == BaselineIntent::Label) == !queue_label,
))]
pub fn baseline_intent(queue_label: bool) -> BaselineIntent {
    if queue_label {
        BaselineIntent::Requeue
    } else {
        BaselineIntent::Label
    }
}
