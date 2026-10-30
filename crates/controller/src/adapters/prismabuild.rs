use super::record::Records;
use crate::executor::Output as Out;
use crate::{
    Result,
    executor::{Completion, Delivery, Executor, Observation, Reconciliation},
};
use std::path::Path;
use tl3_kernel::EffectKind;
use tl3_kernel::{Event, Reply};

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
                kind: EffectKind::SubmitChecks,
                ..
            } => vec![Observation::Kernel(Event::Reply {
                node,
                generation: delivery.generation,
                reply: Reply::ChecksDone {
                    report: true,
                    pass: true,
                    in_footprint: true,
                },
            })],
            Out::Effect {
                kind: EffectKind::WithdrawJob,
                ..
            } => vec![],
            _ => return Err(crate::Error::Invalid("unsupported PrismaBuild effect")),
        };
        self.0
            .deliver(delivery, Completion::confirmed(observations))
    }
    fn reconcile(&mut self, delivery: &Delivery) -> Result<Reconciliation> {
        self.0.reconcile(delivery)
    }
}
