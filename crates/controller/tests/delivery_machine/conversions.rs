pub(crate) fn delivery_state(state: tl3_store::EffectState) -> tl3_delivery::State {
    use tl3_store::EffectState as S;
    match state {
        S::Intended => tl3_delivery::State::Intended,
        S::Sent => tl3_delivery::State::Sent,
        S::Uncertain => tl3_delivery::State::Uncertain,
        S::Confirmed => tl3_delivery::State::Confirmed,
        S::Failed => tl3_delivery::State::Failed,
        S::Cancelling => tl3_delivery::State::Cancelling,
        S::Cancelled => tl3_delivery::State::Cancelled,
    }
}

pub(crate) fn stored_state(state: tl3_delivery::State) -> tl3_store::EffectState {
    use tl3_delivery::State as S;
    match state {
        S::Intended => tl3_store::EffectState::Intended,
        S::Sent => tl3_store::EffectState::Sent,
        S::Uncertain => tl3_store::EffectState::Uncertain,
        S::Confirmed => tl3_store::EffectState::Confirmed,
        S::Failed => tl3_store::EffectState::Failed,
        S::Cancelling => tl3_store::EffectState::Cancelling,
        S::Cancelled => tl3_store::EffectState::Cancelled,
    }
}

pub(crate) fn delivery_ack(ack: Ack) -> tl3_delivery::Ack {
    match ack {
        Ack::Confirmed | Ack::DeliveryAccepted => tl3_delivery::Ack::Confirmed,
        Ack::Retryable => tl3_delivery::Ack::Retryable,
        Ack::Permanent => tl3_delivery::Ack::Permanent,
        Ack::BodyPolicyRefused => tl3_delivery::Ack::BodyPolicyRefused,
    }
}

