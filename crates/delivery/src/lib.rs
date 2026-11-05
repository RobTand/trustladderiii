#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;

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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ack {
    Confirmed,
    Retryable,
    Permanent,
    BodyPolicyRefused,
}

/// An ordering sampled by the shell, generic over its epoch representation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EpochOrder {
    Older,
    Current,
    Newer,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CompletionPlan {
    pub bridge: bool,
    pub from: State,
    pub to: State,
}

/// State-table Effects :755, F1. The comparison and file I/O stay in the shell.
pub fn accepts_fence(_order: EpochOrder) -> bool {
    false
}

/// State-table Effects :755. Existence is sampled only when this returns false.
pub fn advances_fence(order: EpochOrder) -> bool {
    matches!(order, EpochOrder::Newer)
}

/// State-table Effects :725-734. The bridge is staged in the completion transaction.
pub fn completion(state: State, _ack: Ack) -> CompletionPlan {
    CompletionPlan {
        bridge: false,
        from: state,
        to: State::Intended,
    }
}

/// State-table Effects :724-734. No final state has an outgoing edge.
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
pub fn blocks_epoch_advance(state: State) -> bool {
    matches!(state, State::Sent | State::Uncertain | State::Cancelling)
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
