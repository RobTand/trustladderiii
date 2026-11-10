//! Simulation checks: every submitted check run passes at once. Registered under the
//! controller routing tag `Upstream::PrismaBuild`, which the Flux checks adapter owns.
use crate::adapters::record::Records;
use crate::executor::Output as Out;
use crate::{
    Result,
    executor::{Completion, Delivery, Executor, Observation, Reconciliation},
};
use std::path::Path;
use tl3_kernel::{EffectKind, Event, Reply};

pub struct Fake(Records);
impl Fake {
    pub fn new(root: &Path) -> Self {
        Self(Records::new(root))
    }
}
impl Executor for Fake {
    fn contract_evidence(
        &mut self,
        delivery: &Delivery,
    ) -> Result<crate::dispatch_liveness::decision::ContractEvidence> {
        self.0.contract_evidence(delivery)
    }

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
            _ => return Err(crate::Error::Invalid("unsupported checks effect")),
        };
        self.0
            .deliver(delivery, Completion::confirmed(observations))
    }
    fn reconcile(&mut self, delivery: &Delivery) -> Result<Reconciliation> {
        self.0.reconcile(delivery)
    }
}
