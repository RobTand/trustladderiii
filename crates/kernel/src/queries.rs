use crate::{EffectKind, Node, Phase};

/// The evidence or resource a settled node needs next.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Wait {
    Incident,
    Answer,
    Resume,
    Effect(EffectKind),
    WorkerReply,
    CheckResults,
    MergeQueue,
    SlotOrFiles,
    AgentPool,
    UpstreamEvent,
    None,
}

impl Node {
    pub fn waiting_on(&self) -> Wait {
        if self.waiting_incident {
            Wait::Incident
        } else if self.question.is_some() {
            Wait::Answer
        } else if self.paused {
            Wait::Resume
        } else if let Some(effect) = self.effect {
            Wait::Effect(effect.kind)
        } else if self.run.is_some() {
            Wait::WorkerReply
        } else if self.job.is_some() {
            Wait::CheckResults
        } else {
            match self.phase {
                Phase::Queued => Wait::MergeQueue,
                Phase::Ready(_) => Wait::SlotOrFiles,
                Phase::Planning { .. } => Wait::AgentPool,
                Phase::Closed(_) => Wait::None,
                _ => Wait::UpstreamEvent,
            }
        }
    }

    /// Whether the result of this acknowledged operation is still awaited.
    pub fn operation_pending(&self, kind: EffectKind, generation: u64) -> bool {
        match kind {
            EffectKind::StartRun(_) | EffectKind::StartRebase => self.run == Some(generation),
            EffectKind::SubmitChecks => self.job == Some(generation),
            EffectKind::Enqueue => {
                self.phase == Phase::Queued && generation.checked_add(1) == Some(self.generation)
            }
            _ => false,
        }
    }

    /// A moved-on node still needs the delivery to settle, but not its result.
    pub fn effect_cancelling(&self, seq: u64) -> bool {
        self.effect
            .is_some_and(|effect| effect.seq == seq && effect.cancelling)
    }
}
