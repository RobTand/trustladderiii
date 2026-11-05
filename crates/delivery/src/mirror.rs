//! Read-only mirror of controller lib.rs:1231-1438. Scheduling, observations,
//! store loads, durable transitions, execute and reconcile remain separate events.
use crate::{Ack, State, completion};
#[cfg(verus_keep_ghost)]
use vstd::prelude::*;

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Read {
    Found,
    Absent,
    Pending,
}

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Wait,
    Send,
    Uncertain,
    Cancel,
    Cancelling,
    Complete,
    Retry,
    Ambiguous,
}

/// State-table :724-734. Special queue shutdown and approval history checks run
/// before this ordinary per-effect branch; their conformance is scoped separately.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
    out == if cancelling && state == State::Intended { Action::Cancel }
    else if cancelling && matches!(state, State::Sent | State::Uncertain) { Action::Cancelling }
    else if outage && !cancelling { Action::Wait }
    else if state == State::Intended { Action::Send }
    else if state == State::Sent { Action::Uncertain }
    else { Action::Wait },
))]
pub fn dispatch(state: State, cancelling: bool, outage: bool) -> Action {
    if cancelling {
        match state {
            State::Intended => return Action::Cancel,
            State::Sent | State::Uncertain => return Action::Cancelling,
            _ => {}
        }
    }
    if outage && !cancelling {
        return Action::Wait;
    }
    match state {
        State::Intended => Action::Send,
        State::Sent => Action::Uncertain,
        _ => Action::Wait,
    }
}

/// State-table reconciliation rows, including holds. Found is handled by the
/// real completion plan; Pending is never an acknowledgement or evidence of absence.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
    out == match read {
        Read::Found => Action::Complete,
        Read::Absent => if state == State::Cancelling { Action::Cancel }
            else if !held { Action::Retry } else { Action::Wait },
        Read::Pending => if approval && !cancelling && !held { Action::Ambiguous }
            else { Action::Wait },
    },
))]
pub fn reconciliation(
    state: State,
    read: Read,
    held: bool,
    approval: bool,
    cancelling: bool,
) -> Action {
    match read {
        Read::Found => Action::Complete,
        Read::Absent if state == State::Cancelling => Action::Cancel,
        Read::Absent if !held => Action::Retry,
        Read::Pending if approval && !cancelling && !held => Action::Ambiguous,
        _ => Action::Wait,
    }
}

/// State-table Effects: only the caller's separately committed action changes state.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
    out == match action {
        Action::Send => State::Sent,
        Action::Uncertain => State::Uncertain,
        Action::Cancel => State::Cancelled,
        Action::Cancelling => State::Cancelling,
        Action::Retry => State::Intended,
        Action::Complete => if state == State::Cancelling || ack == Ack::Confirmed {
            State::Confirmed
        } else if ack == Ack::Retryable { State::Intended } else { State::Failed },
        _ => state,
    },
))]
pub fn committed(state: State, action: Action, ack: Ack) -> State {
    match action {
        Action::Send => State::Sent,
        Action::Uncertain => State::Uncertain,
        Action::Cancel => State::Cancelled,
        Action::Cancelling => State::Cancelling,
        Action::Retry => State::Intended,
        Action::Complete => completion(state, ack).to,
        _ => state,
    }
}

/// Q27 / lib.rs:1270-1293: an authorization bypasses prior-history recovery.
/// Without it, a prior ambiguous dispatch cannot POST again; authoritative
/// Absent is valid only for preflight/refused attempts (publishing.rs:518-532).
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
    out == if authorized || !prior_dispatch {Action::Send}
        else if !predecessor && read == Read::Absent {Action::Send}
        else if !predecessor && read == Read::Found {Action::Complete}
        else {Action::Ambiguous},
))]
pub fn approval_dispatch(
    prior_dispatch: bool,
    predecessor: bool,
    authorized: bool,
    read: Read,
) -> Action {
    if authorized || !prior_dispatch {
        return Action::Send;
    }
    match read {
        Read::Absent if !predecessor => Action::Send,
        Read::Found if !predecessor => Action::Complete,
        _ => Action::Ambiguous,
    }
}

/// An issuer retains this process-local attempt across ownership loss. The store
/// row can subsequently be drained by a new owner; `durably_sent` is historical
/// evidence, not a claim that the row remains Sent after recovery.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Issuer {
    pub epoch: u64,
    pub durably_sent: bool,
    pub owns: bool,
    pub in_call: bool,
    pub admitted: bool,
    pub issued: bool,
}

/// State-table :724-734 / lib.rs:1293-1300: durable Sent precedes adapter entry.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out =>
    requires state == State::Sent,
    ensures out.epoch == epoch, out.durably_sent, out.owns, out.in_call,
        !out.admitted, !out.issued,
))]
pub fn begin_issuer(state: State, epoch: u64) -> Issuer {
    Issuer {
        epoch,
        durably_sent: state == State::Sent,
        owns: true,
        in_call: true,
        admitted: false,
        issued: false,
    }
}

/// State-table :755 F1: entry read is a distinct sample, before any fence write
/// or HTTP read/write. Another owner may advance the fence after this sample.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
    out.admitted == (before.epoch >= sampled_fence),
    out.epoch == before.epoch, out.durably_sent == before.durably_sent,
    out.owns == before.owns, out.in_call == before.in_call, out.issued == before.issued,
))]
pub fn entry(before: Issuer, sampled_fence: u64) -> Issuer {
    let order = if before.epoch < sampled_fence {
        crate::EpochOrder::Older
    } else if before.epoch == sampled_fence {
        crate::EpochOrder::Current
    } else {
        crate::EpochOrder::Newer
    };
    Issuer {
        admitted: crate::accepts_fence(order),
        ..before
    }
}

/// Store lib.rs:73-80 F3: ownership loss does not erase an issuer or its HTTP call.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
    !out.owns, out.epoch == before.epoch, out.durably_sent == before.durably_sent,
    out.admitted == before.admitted, out.in_call == before.in_call, out.issued == before.issued,
))]
pub fn lose_ownership(before: Issuer) -> Issuer {
    Issuer {
        owns: false,
        ..before
    }
}

/// F3: no issue-time fence is assumed. This mirrors the real checked-entry then
/// GET/Dispatching/POST ordering. A zombie still has the original Sent evidence.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out =>
    requires before.durably_sent, before.admitted, before.in_call,
    ensures out.issued, out.durably_sent, out.epoch == before.epoch,
        out.admitted == before.admitted, out.owns == before.owns,
        out.in_call == before.in_call,
))]
pub fn issue_http(before: Issuer) -> Issuer {
    Issuer {
        issued: true,
        ..before
    }
}
