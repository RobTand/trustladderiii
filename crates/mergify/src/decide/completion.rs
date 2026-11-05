//! Completion-shell predicates. Upstream parsing and all sampling stay in the adapter.

use super::{Head, QueueState, Time};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Complete<F> {
    Confirmed { progress: bool, conflict: bool },
    Retryable,
    Permanent(F),
    Pending,
    Absent,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ack<F> {
    Confirmed,
    Retryable,
    Permanent(F),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Resolution<F> {
    Found { ack: Ack<F>, conflict: bool, progress: bool },
    Pending,
    Absent,
}

pub fn resolve<F>(result: Complete<F>, observed: bool) -> Resolution<F> {
    let _ = (result, observed);
    todo!("A1 result mapping")
}

/// A receipt is Found regardless of the acknowledgement stored in it (#760).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Receipt { Found, Pending, Absent }

pub fn confirmed(receipt: Receipt) -> bool {
    let _ = receipt;
    todo!("A1 receipt")
}

pub fn head(equal: bool) -> Head {
    let _ = equal;
    todo!("A1 head")
}

pub fn conflicts(mergeable: Option<bool>) -> bool {
    let _ = mergeable;
    todo!("A1 conflict")
}

/// None requests the hold projection; a merge must skip that projection entirely.
pub fn held(queue: QueueState, unresolved: Option<bool>) -> Option<bool> {
    let _ = (queue, unresolved);
    todo!("A1 hold gate")
}

/// None requests repaired_head; a baseline skips that projection entirely.
pub fn person_events(baseline: bool, repaired: Option<bool>) -> Option<bool> {
    let _ = (baseline, repaired);
    todo!("A1 person gate")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PersonEvidence { Dequeued(u64), Closed(u64), Other }

pub fn person_event(evidence: PersonEvidence) -> Option<u64> {
    let _ = evidence;
    todo!("A1 person evidence")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Freshness {
    Snapshot { unresolved: bool },
    Intent { last_event: Option<u64> },
}

pub fn fresh(event: u64, source: Freshness) -> bool {
    let _ = (event, source);
    todo!("A1 freshness")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Label { Queue, Queued, Dequeued, Other }

pub fn has_label(labels: &[Label], wanted: Label) -> bool {
    let _ = (labels, wanted);
    todo!("A1 label membership")
}

pub fn retry(queue_label: bool, queue: QueueState) -> bool {
    let _ = (queue_label, queue);
    todo!("A1 retry")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DequeueGate { Done, Pending, Events }

pub fn dequeue_gate(retry: bool, concluded: bool, dequeued_label: bool) -> DequeueGate {
    let _ = (retry, concluded, dequeued_label);
    todo!("A1 dequeue read gate")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LabelAction { Labeled, Other }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LabelEvent {
    pub id: u64,
    pub label: Label,
    pub action: LabelAction,
    pub at: Time,
}

pub fn newest(events: &[LabelEvent], label: Label) -> Option<usize> {
    let _ = (events, label);
    todo!("A1 newest label")
}

pub fn labeled(action: LabelAction) -> bool {
    let _ = action;
    todo!("A1 labeled")
}

pub fn time(secs: i64, nanos: u32) -> Time {
    let _ = (secs, nanos);
    todo!("A1 timestamp")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Proof { Empty, Present, Missing }

pub fn proof_missing(proof: Proof) -> bool {
    let _ = proof;
    todo!("A1 proof gate")
}

pub fn reject_queue_pr(merge_queue_pr: bool) -> bool {
    let _ = merge_queue_pr;
    todo!("A1 queue PR gate")
}

pub fn read_facts(healed: bool) -> bool {
    let _ = healed;
    todo!("A1 heal gate")
}
