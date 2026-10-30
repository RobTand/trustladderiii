use super::record::Records;
use crate::{
    Result,
    executor::{Completion, Delivery, Executor, Observation, Reconciliation},
};
use std::path::Path;
use tl3_kernel::{EffectKind, Out};

pub struct Fake(Records);
impl Fake {
    pub fn new(root: &Path) -> Self {
        Self(Records::new(root))
    }
}
impl Executor for Fake {
    fn execute(&mut self, delivery: &Delivery) -> Result<Completion> {
        let node = delivery.node();
        let observations = match delivery.output {
            Out::Effect {
                kind: EffectKind::StartRebase,
                ..
            } => vec![Observation::Rebased {
                node,
                generation: delivery.generation,
                sha: format!("{:040x}", 2000 + node.0 * 100 + delivery.generation),
            }],
            Out::Effect {
                kind: EffectKind::PrepareWorktree | EffectKind::Push | EffectKind::RemoveWorktree,
                ..
            } => vec![],
            _ => return Err(crate::Error::Invalid("unsupported worktree effect")),
        };
        self.0
            .deliver(delivery, Completion::confirmed(observations))
    }
    fn reconcile(&mut self, delivery: &Delivery) -> Result<Reconciliation> {
        self.0.reconcile(delivery)
    }
}
