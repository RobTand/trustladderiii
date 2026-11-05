//! Execute-shell plans. The adapter retains all reads, writes and fences.
#[cfg(verus_keep_ghost)]
use vstd::prelude::*;

#[cfg(not(verus_keep_ghost))]
macro_rules! decision_while {
    (($condition:expr) [$($contract:tt)*] $body:block) => { while $condition $body };
}
#[cfg(verus_keep_ghost)]
macro_rules! decision_while {
    (($condition:expr) [$($contract:tt)*] $body:block) => {
        vstd::prelude::verus_exec_expr! { while $condition $($contract)* $body }
    };
}
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ack {
    Confirmed,
    Other,
}

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Receipt<C> {
    Found { value: C, ack: Ack },
    Pending,
    Absent,
}

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Start<C> {
    Found { value: C, read_receipt: bool },
    Pending,
    Proceed,
}

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Attempt<A> {
    Saved(A),
    New,
}

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IntentEffect {
    Save,
    Fence,
}

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Next<E> {
    Effect(E),
    Applied,
}

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Final<C> {
    Found { value: C, deliver: bool },
    Unknown,
}

/// Preserved existing behaviour (no rule): old epochs refuse before any execute read.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures out == (incoming < current),))]
pub fn fenced(incoming: i64, current: i64) -> bool {
    incoming < current
}

/// Preserved existing behaviour (no rule): only Confirmed requests the extra receipt read; Pending stops execution.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures match receipt {
    Receipt::Found { value, ack } => matches!(out, Start::Found { value: v, read_receipt } if equal(v, value) && read_receipt == matches!(ack, Ack::Confirmed)),
    Receipt::Pending => matches!(out, Start::Pending), Receipt::Absent => matches!(out, Start::Proceed),
},))]
pub fn start<C>(receipt: Receipt<C>) -> Start<C> {
    match receipt {
        Receipt::Found { value, ack } => Start::Found {
            value,
            read_receipt: matches!(ack, Ack::Confirmed),
        },
        Receipt::Pending => Start::Pending,
        Receipt::Absent => Start::Proceed,
    }
}

/// Preserved existing behaviour (no rule): an existing Found receipt skips duplicate delivery; missing receipt requires delivery.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures out == !found,))]
pub fn needs_delivery(found: bool) -> bool {
    !found
}

/// Preserved existing behaviour (no rule): saved attempts skip fresh issue-event sampling.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures match saved { Some(value) => matches!(out, Attempt::Saved(v) if equal(v, value)), None => matches!(out, Attempt::New) },))]
pub fn attempt<A>(saved: Option<A>) -> Attempt<A> {
    match saved {
        Some(value) => Attempt::Saved(value),
        None => Attempt::New,
    }
}

/// Preserved existing behaviour (no rule): the new attempt starts Ready exactly when the queue label was present, independently of Prepare.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures match out { Some(value) => queue_label && equal(value, ready_value), None => !queue_label },))]
pub fn ready<R>(queue_label: bool, ready_value: R) -> Option<R> {
    if queue_label { Some(ready_value) } else { None }
}

/// Preserved existing behaviour (no rule): a new attempt records the maximum observed event id, or zero for no events.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures (forall|j: int| 0 <= j < events@.len() ==> events@[j] <= out) && (out == 0 || exists|j: int| 0 <= j < events@.len() && events@[j] == out),))]
pub fn last_event(events: &[u64]) -> u64 {
    let mut i = 0;
    let mut best = 0;
    decision_while! { (i < events.len()) [
        invariant i <= events@.len(),
            forall|j: int| 0 <= j < i ==> events@[j] <= best,
            best == 0 || exists|j: int| 0 <= j < i && events@[j] == best,
        decreases events@.len() - i,
    ] {
        if events[i] > best { best = events[i]; }
        i += 1;
    } }
    best
}

/// #751 K-write-ahead and #760 I/O-order fidelity: intent is durably saved before the fence and following label/command dispatch.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures out.0 == IntentEffect::Save && out.1 == IntentEffect::Fence,))]
pub fn intent() -> (IntentEffect, IntentEffect) {
    (IntentEffect::Save, IntentEffect::Fence)
}

/// Preserved existing behaviour (no rule): Applied is emitted only after the effect queue is exhausted; early returns never reach it.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures match effect { Some(value) => matches!(out, Next::Effect(v) if equal(v, value)), None => matches!(out, Next::Applied) },))]
pub fn next<E>(effect: Option<E>) -> Next<E> {
    match effect {
        Some(value) => Next::Effect(value),
        None => Next::Applied,
    }
}

/// Preserved existing behaviour (no rule): command and Retryable reconciliation return only on Found; Pending/Absent have no completion.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures match receipt { Receipt::Found { value, .. } => matches!(out, Some(v) if equal(v, value)), _ => matches!(out, None) },))]
pub fn found<C>(receipt: Receipt<C>) -> Option<C> {
    match receipt {
        Receipt::Found { value, .. } => Some(value),
        _ => None,
    }
}

/// Preserved existing behaviour (no rule): final Found returns its value; only Confirmed is delivered.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures match receipt { Receipt::Found { value, ack } => matches!(out, Final::Found { value: v, deliver } if equal(v, value) && deliver == matches!(ack, Ack::Confirmed)), _ => matches!(out, Final::Unknown) },))]
pub fn finish<C>(receipt: Receipt<C>) -> Final<C> {
    match receipt {
        Receipt::Found { value, ack } => Final::Found {
            value,
            deliver: matches!(ack, Ack::Confirmed),
        },
        _ => Final::Unknown,
    }
}
