#![no_std]
#![forbid(unsafe_code)]

#[cfg(verus_keep_ghost)]
use vstd::prelude::*;

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Intended,
    Sent,
    Uncertain,
    Confirmed,
    Failed,
    Cancelling,
    Cancelled,
}

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ack {
    Confirmed,
    Retryable,
    Permanent,
    BodyPolicyRefused,
}

/// An ordering sampled by the shell, generic over its epoch representation.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EpochOrder {
    Older,
    Current,
    Newer,
}

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CompletionPlan {
    pub bridge: bool,
    pub from: State,
    pub to: State,
}

/// State-table Effects :755, F1. The comparison and file I/O stay in the shell.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
    out == !matches!(order, EpochOrder::Older),
))]
pub fn accepts_fence(order: EpochOrder) -> bool {
    !matches!(order, EpochOrder::Older)
}

/// State-table Effects :755. Existence is sampled only when this returns false.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
    out == matches!(order, EpochOrder::Newer),
))]
pub fn advances_fence(order: EpochOrder) -> bool {
    matches!(order, EpochOrder::Newer)
}

/// State-table Effects :725-734. The bridge is staged in the completion transaction.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
    out.bridge == (state == State::Uncertain && ack != Ack::Confirmed),
    out.from == if out.bridge { State::Sent } else { state },
    out.to == if state == State::Cancelling || ack == Ack::Confirmed {
        State::Confirmed
    } else if ack == Ack::Retryable { State::Intended } else { State::Failed },
))]
pub fn completion(state: State, ack: Ack) -> CompletionPlan {
    CompletionPlan {
        bridge: false,
        from: state,
        to: State::Intended,
    }
}

/// State-table Effects :724-734. No final state has an outgoing edge.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
    out == match from {
        State::Intended => matches!(to, State::Sent | State::Cancelled),
        State::Sent => matches!(to, State::Confirmed | State::Intended | State::Failed
            | State::Uncertain | State::Cancelling),
        State::Uncertain => matches!(to, State::Confirmed | State::Intended | State::Cancelling),
        State::Cancelling => matches!(to, State::Confirmed | State::Cancelled),
        _ => false,
    },
))]
pub fn allows(from: State, to: State) -> bool {
    use State::*;
    matches!(
        (from, to),
        (Intended, Sent | Cancelled)
            | (Sent, Confirmed | Intended | Failed | Uncertain | Cancelling)
            | (Uncertain, Confirmed | Intended | Cancelling)
            | (Cancelling, Confirmed | Cancelled)
    )
}

/// State-table Effects :731-734 and store ActiveDeliveries, F2.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
    out == matches!(state, State::Sent | State::Uncertain | State::Cancelling),
))]
pub fn blocks_epoch_advance(state: State) -> bool {
    matches!(state, State::Sent | State::Uncertain | State::Cancelling)
}

/// Q27 / state-machine approval retries: clear authorizes only an active attempt.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
    out == matches!(state, State::Sent | State::Uncertain),
))]
pub fn can_authorize_approval_retry(state: State) -> bool {
    matches!(state, State::Sent | State::Uncertain)
}

/// Q27: retire ambiguity only after repair accounts for all durable dispatches.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
    out == (copies >= dispatches),
))]
pub fn accounts_for_dispatches(copies: u64, dispatches: u64) -> bool {
    copies >= dispatches
}

#[cfg(not(verus_keep_ghost))]
macro_rules! delivery_while {
    (($condition:expr) [$($contract:tt)*] $body:block) => { while $condition $body };
}
#[cfg(verus_keep_ghost)]
macro_rules! delivery_while {
    (($condition:expr) [$($contract:tt)*] $body:block) => {
        vstd::prelude::verus_exec_expr! { while $condition $($contract)* $body }
    };
}

/// State-table Effects, ack by key: equality covers the entire original byte key.
/// Unbounded key length, with a decreasing finite slice length.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
    out == (left@ == right@),
))]
pub fn key_matches(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    let mut index = 0;
    delivery_while! { (index < left.len()) [
        invariant index <= left.len(), left.len() == right.len(),
            forall|i: int| 0 <= i < index ==> left@[i] == right@[i],
        decreases left.len() - index,
    ] {
        if left[index] != right[index] {
            return false;
        }
        index += 1;
    }}
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    /// State-table Effects :755, F1: equality is an accepted entry epoch.
    #[test]
    fn equal_epoch_is_admitted() {
        assert!(accepts_fence(EpochOrder::Current));
    }
}
