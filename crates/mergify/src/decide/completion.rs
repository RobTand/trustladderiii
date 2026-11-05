//! Completion-shell predicates. Upstream parsing and all sampling stay in the adapter.

#[cfg(verus_keep_ghost)]
use vstd::prelude::*;

#[cfg(not(verus_keep_ghost))]
macro_rules! completion_while {
    (($condition:expr) [$($contract:tt)*] $body:block) => { while $condition $body };
}
#[cfg(verus_keep_ghost)]
macro_rules! completion_while {
    (($condition:expr) [$($contract:tt)*] $body:block) => {
        vstd::prelude::verus_exec_expr! { while $condition $($contract)* $body }
    };
}

#[cfg(not(verus_keep_ghost))]
macro_rules! completion_proof {
    ($($body:tt)*) => {};
}
#[cfg(verus_keep_ghost)]
macro_rules! completion_proof {
    ($($body:tt)*) => { vstd::prelude::verus_exec_expr! { proof { $($body)* } } };
}

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Complete<F> {
    Confirmed { progress: bool, conflict: bool },
    Retryable,
    Permanent(F),
    Pending,
    Absent,
}

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ack<F> {
    Confirmed,
    Retryable,
    Permanent(F),
}

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Resolution<F> {
    Found {
        ack: Ack<F>,
        conflict: bool,
        progress: bool,
    },
    Pending,
    Absent,
}

/// State-table docs/state-table.md:533-547 on cdacce8f, #373/#608: only Confirmed yields a confirmed receipt; conflict and progress retain their flags.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures match result {
            Complete::Confirmed { progress, conflict } => matches!(out, Resolution::Found { ack: Ack::Confirmed, conflict: c, progress: p } if c == conflict && p == (progress && observed)),
            Complete::Retryable => matches!(out, Resolution::Found { ack: Ack::Retryable, conflict: false, progress: false }),
            Complete::Permanent(f) => matches!(out, Resolution::Found { ack: Ack::Permanent(g), conflict: false, progress: false } if vstd::prelude::equal(g, f)),
            Complete::Pending => matches!(out, Resolution::Pending),
            Complete::Absent => matches!(out, Resolution::Absent),
        },))]
pub fn resolve<F>(result: Complete<F>, observed: bool) -> Resolution<F> {
    match result {
        Complete::Confirmed { progress, conflict } => Resolution::Found {
            ack: Ack::Confirmed,
            conflict,
            progress: progress && observed,
        },
        Complete::Retryable => Resolution::Found {
            ack: Ack::Retryable,
            conflict: false,
            progress: false,
        },
        Complete::Permanent(failure) => Resolution::Found {
            ack: Ack::Permanent(failure),
            conflict: false,
            progress: false,
        },
        Complete::Pending => Resolution::Pending,
        Complete::Absent => Resolution::Absent,
    }
}

/// A receipt is Found regardless of the acknowledgement stored in it (#760).
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Receipt {
    Found,
    Pending,
    Absent,
}

/// #760, state-table Publishing step 4 at docs/state-table.md:533-535 on cdacce8f: confirmation comes from a Found local receipt.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures out == matches!(receipt, Receipt::Found),))]
pub fn confirmed(receipt: Receipt) -> bool {
    matches!(receipt, Receipt::Found)
}

/// #608 and state-table Reviewing Approved(head), docs/state-table.md:533 on cdacce8f: preserve exact head equality.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures matches!(out, Head::Current) == equal,))]
pub fn head(equal: bool) -> Head {
    if equal { Head::Current } else { Head::Other }
}

/// #373, state-table Publishing conflict at docs/state-table.md:539 on cdacce8f: only mergeable=false is a conflict here.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures out == matches!(mergeable, Some(false)),))]
pub fn conflicts(mergeable: Option<bool>) -> bool {
    matches!(mergeable, Some(false))
}

/// None requests the hold projection; a merge must skip that projection entirely.
/// #608, docs/state-table.md:666 on cdacce8f: a merge wins over a human hold and skips its read.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures out == if matches!(queue, Queue::Merged) { Some(false) } else { unresolved },))]
pub fn held(queue: Queue, unresolved: Option<bool>) -> Option<bool> {
    if matches!(queue, Queue::Merged) {
        Some(false)
    } else {
        unresolved
    }
}

/// None requests repaired_head; a baseline skips that projection entirely.
/// #608/#760: baseline presence skips repaired_head before the issue-events read.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures out == if baseline { Some(true) } else { repaired },))]
pub fn person_events(baseline: bool, repaired: Option<bool>) -> Option<bool> {
    if baseline { Some(true) } else { repaired }
}

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PersonEvidence {
    Dequeued(u64),
    Closed(u64),
    Other,
}

/// #373, docs/state-table.md:668-669 on cdacce8f: only person dequeue/close evidence carries a hold event.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures out == match evidence { PersonEvidence::Dequeued(id) | PersonEvidence::Closed(id) => Some(id), PersonEvidence::Other => None },))]
pub fn person_event(evidence: PersonEvidence) -> Option<u64> {
    match evidence {
        PersonEvidence::Dequeued(event) | PersonEvidence::Closed(event) => Some(event),
        PersonEvidence::Other => None,
    }
}

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Freshness {
    Snapshot { unresolved: bool },
    Intent { last_event: Option<u64> },
}

/// #608: an event at or before the saved cycle cannot be fresh; a snapshot uses the unresolved hold, not event age.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures out == match source {
            Freshness::Snapshot { unresolved } => unresolved,
            Freshness::Intent { last_event } => match last_event { Some(last) => event > last, None => false },
        },))]
pub fn fresh(event: u64, source: Freshness) -> bool {
    match source {
        Freshness::Snapshot { unresolved } => unresolved,
        Freshness::Intent { last_event } => match last_event {
            Some(last) => event > last,
            None => false,
        },
    }
}

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Label {
    Queue,
    Queued,
    Dequeued,
    Other,
}

/// #608/#760: membership is exact, including distinct queued and tl3-queue labels.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures out == (exists|i: int| 0 <= i < labels@.len() && labels@[i] == wanted),))]
pub fn has_label(labels: &[Label], wanted: Label) -> bool {
    let mut i = 0;
    completion_while! { (i < labels.len()) [
        invariant i <= labels@.len(),
            forall|j: int| 0 <= j < i ==> labels@[j] != wanted,
        decreases labels@.len() - i,
    ] {
        if labels[i] == wanted {
            return true;
        }
        i += 1;
    } }
    false
}

/// #608: retry applies to the queue label or Dequeued/FailedCi/Conflict, not a person dequeue alone.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures out == (queue_label || matches!(queue, Queue::Retryable)),))]
pub fn retry(queue_label: bool, queue: Queue) -> bool {
    queue_label || matches!(queue, Queue::Retryable)
}

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DequeueGate {
    Done,
    Pending,
    Events,
}

/// #608: never read events when no retry, a concluded queue rule, or a missing dequeued label decides first.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures out == if !retry || concluded { DequeueGate::Done } else if !dequeued_label { DequeueGate::Pending } else { DequeueGate::Events },))]
pub fn dequeue_gate(retry: bool, concluded: bool, dequeued_label: bool) -> DequeueGate {
    if !retry || concluded {
        DequeueGate::Done
    } else if !dequeued_label {
        DequeueGate::Pending
    } else {
        DequeueGate::Events
    }
}

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LabelAction {
    Labeled,
    Other,
}

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LabelEvent {
    pub id: u64,
    pub label: Label,
    pub action: LabelAction,
    pub at: Time,
}

/// #608: newest-per-label uses maximum event id and the last event on a tie, including unlabeled events.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures match out {
            None => forall|j: int| 0 <= j < events@.len() ==> events@[j].label != label,
            Some(i) => i < events@.len() && events@[i as int].label == label
                && forall|j: int| 0 <= j < events@.len() && events@[j].label == label ==>
                    (events@[j].id < events@[i as int].id || (events@[j].id == events@[i as int].id && j <= i)),
        },))]
pub fn newest(events: &[LabelEvent], label: Label) -> Option<usize> {
    let mut best: Option<usize> = None;
    let mut i = 0;
    completion_while! { (i < events.len()) [
        invariant i <= events@.len(),
            match best {
                None => forall|j: int| 0 <= j < i ==> events@[j].label != label,
                Some(b) => b < i && events@[b as int].label == label
                    && forall|j: int| 0 <= j < i && events@[j].label == label ==>
                        (events@[j].id < events@[b as int].id || (events@[j].id == events@[b as int].id && j <= b)),
            },
        decreases events@.len() - i,
    ] {
        if events[i].label == label {
            best = match best {
                Some(previous) => {
                    if events[i].id >= events[previous].id {
                        Some(i)
                    } else {
                        best
                    }
                }
                None => Some(i),
            };
        }
        i += 1;
    } }
    best
}

/// #608: removing the dequeued label never proves completion of a dequeue.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures out == matches!(action, LabelAction::Labeled),))]
pub fn labeled(action: LabelAction) -> bool {
    matches!(action, LabelAction::Labeled)
}

/// #608: preserve chrono ordering, including negative Unix seconds and leap-second nanoseconds.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures out.secs == secs && out.nanos == nanos,))]
pub fn time(secs: i64, nanos: u32) -> Time {
    Time { secs, nanos }
}

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Proof {
    Empty,
    Present,
    Missing,
}

/// Q52/#447: an empty proof is optional; only required proof absent from the body rejects publication.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures out == matches!(proof, Proof::Missing),))]
pub fn proof_missing(proof: Proof) -> bool {
    matches!(proof, Proof::Missing)
}

/// #373: Mergify merge-queue PRs cannot be queued by this shell.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures out == merge_queue_pr,))]
pub fn reject_queue_pr(merge_queue_pr: bool) -> bool {
    merge_queue_pr
}

/// #447: a body heal invalidates the checks from the preceding read.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures out == !healed,))]
pub fn read_facts(healed: bool) -> bool {
    !healed
}

/// Equality of every CheckRun field except completed_at and check_suite.
/// The shell compares opaque upstream payloads without parsing away unknown fields.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CheckComparison {
    BothMissing,
    OneMissing,
    BothPresent { fields: [bool; 10] },
}

/// #608/#760: late completed_at/check_suite metadata does not acknowledge acceptance. The ten remaining upstream field equalities decide.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures out == match comparison {
            CheckComparison::BothMissing => false,
            CheckComparison::OneMissing => true,
            CheckComparison::BothPresent { fields } => exists|j: int| 0 <= j < 10 && !fields@[j],
        },))]
pub fn check_changed(comparison: CheckComparison) -> bool {
    match comparison {
        CheckComparison::BothMissing => false,
        CheckComparison::OneMissing => true,
        CheckComparison::BothPresent { fields } => {
            let mut i = 0;
            completion_while! { (i < fields.len()) [
                invariant i <= 10,
                    forall|j: int| 0 <= j < i ==> fields@[j],
                decreases 10 - i,
            ] {
                if !fields[i] {
                    completion_proof! {
                        assert(exists|j: int| 0 <= j < 10 && !fields@[j]) by {
                            assert(!fields@[i as int]);
                        }
                    }
                    return true;
                }
                i += 1;
            } }
            false
        }
    }
}

// The lifecycle result has a failure payload opaque to completion decisions.
// This lossless boundary conversion is exercised by the differential shell test.
#[cfg(not(verus_keep_ghost))]
impl From<super::enqueue::Complete> for Complete<super::enqueue::Failure> {
    fn from(result: super::enqueue::Complete) -> Self {
        use super::enqueue::Complete as C;
        match result {
            C::Confirmed { progress, conflict } => Self::Confirmed { progress, conflict },
            C::Retryable => Self::Retryable,
            C::Permanent(failure) => Self::Permanent(failure),
            C::Pending => Self::Pending,
            C::Absent => Self::Absent,
        }
    }
}

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HoldEvidence {
    Person,
    Command,
    Other,
}

/// #608/#373: person and explicit-command holds require a later resume.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures out == !matches!(evidence, HoldEvidence::Other),))]
pub fn hold_candidate(evidence: HoldEvidence) -> bool {
    !matches!(evidence, HoldEvidence::Other)
}

/// #608: a hold resolves only when both rows exist and resume is strictly newer.
/// Missing node has no paired rows and remains unresolved.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures out == match versions {
    Some((observed, resumed)) => resumed <= observed,
    None => true,
},))]
pub fn unresolved_hold(versions: Option<(i64, i64)>) -> bool {
    match versions {
        Some((observed, resumed)) => resumed <= observed,
        None => true,
    }
}

/// The queue distinctions used by these shell gates. Every other state shares
/// the same behavior; conversion from the lifecycle enum is exhaustive.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Queue {
    Merged,
    Retryable,
    Other,
}

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Head {
    Current,
    Other,
}

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Time {
    pub secs: i64,
    pub nanos: u32,
}

#[cfg(not(verus_keep_ghost))]
impl From<super::QueueState> for Queue {
    fn from(queue: super::QueueState) -> Self {
        use super::QueueState as Q;
        match queue {
            Q::Merged => Self::Merged,
            Q::Dequeued | Q::FailedCi | Q::Conflict => Self::Retryable,
            Q::Waiting | Q::Queued | Q::ChecksRunning | Q::DequeuedByPerson | Q::Unknown => {
                Self::Other
            }
        }
    }
}
#[cfg(not(verus_keep_ghost))]
impl From<Head> for super::Head {
    fn from(head: Head) -> Self {
        match head {
            Head::Current => Self::Current,
            Head::Other => Self::Other,
        }
    }
}
#[cfg(not(verus_keep_ghost))]
impl From<Time> for super::Time {
    fn from(time: Time) -> Self {
        Self {
            secs: time.secs,
            nanos: time.nanos,
        }
    }
}
