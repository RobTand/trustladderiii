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
        if delivery.pull_request.is_none() {
            return Err(crate::Error::Invalid("enqueue needs a PR number"));
        }
        let observations = match delivery.output {
            Out::Effect {
                kind: EffectKind::Enqueue,
                ..
            } => vec![Observation::Merged {
                node,
                sha: delivery
                    .head_sha
                    .clone()
                    .ok_or(crate::Error::Invalid("queue needs a head"))?,
                checks_passed: true,
            }],
            _ => return Err(crate::Error::Invalid("unsupported Mergify effect")),
        };
        self.0
            .deliver(delivery, Completion::confirmed(observations))
    }
    fn reconcile(&mut self, delivery: &Delivery) -> Result<Reconciliation> {
        self.0.reconcile(delivery)
    }
}
