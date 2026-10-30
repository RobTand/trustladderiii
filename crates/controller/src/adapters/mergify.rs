use super::record::Records;
use crate::executor::Output as Out;
use crate::{
    Result,
    executor::{Completion, Delivery, Executor, Observation, Reconciliation},
};
use std::path::Path;
use tl3_kernel::EffectKind;

pub struct Fake {
    records: Records,
    hold_merges: bool,
}
impl Fake {
    pub fn new(root: &Path) -> Self {
        Self {
            records: Records::new(root),
            hold_merges: root.join("hold-merges").exists(),
        }
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
        self.records.deliver(
            delivery,
            Completion::confirmed(if self.hold_merges {
                vec![]
            } else {
                observations
            }),
        )
    }
    fn reconcile(&mut self, delivery: &Delivery) -> Result<Reconciliation> {
        self.records.reconcile(delivery)
    }
}
