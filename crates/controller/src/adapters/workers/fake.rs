use super::super::record::Records;
use crate::{
    Result,
    executor::{Completion, Delivery, Executor, Observation, Reconciliation},
};
use std::path::Path;
use tl3_kernel::Role;
use tl3_kernel::{EffectKind, Out};
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
        let generation = delivery.generation;
        let observations = match delivery.output {
            Out::Effect {
                kind: EffectKind::StartRun(role),
                ..
            } => match role {
                Role::Ingest => vec![Observation::Kernel(Event::Reply {
                    node,
                    generation,
                    reply: Reply::IngestDone { size: 1 },
                })],
                Role::Audit => vec![Observation::Kernel(Event::Reply {
                    node,
                    generation,
                    reply: Reply::AuditAgrees,
                })],
                Role::Plan => vec![Observation::Plan {
                    node,
                    generation,
                    paths: (0..80).map(|i| format!("src/file_{i}.py")).collect(),
                    fits: true,
                }],
                Role::Make => vec![Observation::Made {
                    node,
                    generation,
                    sha: format!("{:040x}", 1000 + node.0 * 100 + generation),
                }],
                Role::Review => vec![Observation::Reviewed {
                    node,
                    generation,
                    sha: delivery
                        .head_sha
                        .clone()
                        .ok_or(crate::Error::Invalid("review needs a head"))?,
                }],
            },
            Out::Effect {
                kind: EffectKind::StopRun,
                ..
            } => vec![],
            _ => return Err(crate::Error::Invalid("unsupported worker effect")),
        };
        self.0
            .deliver(delivery, Completion::confirmed(observations))
    }
    fn reconcile(&mut self, delivery: &Delivery) -> Result<Reconciliation> {
        self.0.reconcile(delivery)
    }
}
