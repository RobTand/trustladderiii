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
