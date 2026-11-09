use super::*;
pub(super) fn reference_plan(key: &str, effect: &Effect, completion: &Completion) -> Vec<EffectTransition> {
    let mut commit = Staged::default();
        if effect.state == EffectState::Uncertain && !matches!(completion.ack, Ack::Confirmed | Ack::DeliveryAccepted) {
            // A terminal failed attempt was found. Return to the sending state
            // through the effect machine before recording its failure.
            commit.transitions.push(EffectTransition {
                key: key.into(),
                from: EffectState::Uncertain,
                to: EffectState::Intended,
            });
            commit.transitions.push(EffectTransition {
                key: key.into(),
                from: EffectState::Intended,
                to: EffectState::Sent,
            });
        }
        let from = if effect.state == EffectState::Uncertain && !matches!(completion.ack, Ack::Confirmed | Ack::DeliveryAccepted) {
            EffectState::Sent
        } else {
            effect.state
        };
        let state = if effect.state == EffectState::Cancelling {
            EffectState::Confirmed
        } else {
            match completion.ack {
                Ack::Confirmed | Ack::DeliveryAccepted => EffectState::Confirmed,
                Ack::Retryable => EffectState::Intended,
                Ack::Permanent | Ack::BodyPolicyRefused => EffectState::Failed,
            }
        };
        commit.transitions.push(EffectTransition {
            key: key.into(),
            from,
            to: state,
        });
    commit.transitions
}
