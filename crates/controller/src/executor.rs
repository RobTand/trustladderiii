use serde::{Deserialize, Serialize};
use tl3_kernel::{Ack, Event, NodeId, Out};

use crate::Result;

#[derive(Clone, Debug, Serialize, Deserialize)]
/// Adapter tickets add the payload fields their clients need here.
pub struct Delivery {
    pub key: String,
    pub epoch: i64,
    pub output: Out,
    pub generation: u64,
    pub head_sha: Option<String>,
    pub repository: crate::Repository,
    pub issue: u64,
    pub pull_request: Option<u64>,
    pub paths: Vec<String>,
    pub attempt: u8,
    pub round: u8,
    #[serde(default)]
    pub question: Option<tl3_kernel::Question>,
    #[serde(default)]
    pub note: Option<String>,
    #[serde(default)]
    pub question_context: Option<serde_json::Value>,
}

impl Delivery {
    pub fn node(&self) -> NodeId {
        match self.output {
            Out::Effect { node, .. } | Out::Notice { node, .. } => node,
        }
    }
}

/// External values are interned by the controller before entering the kernel.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Observation {
    Progress {
        key: String,
        checkpoint: String,
    },
    Quota {
        key: String,
        reset_ms: u64,
    },
    GitHubComment {
        repository: crate::Repository,
        issue: u64,
        comment: tl3_github::Comment,
    },
    WorkerReply {
        node: NodeId,
        generation: u64,
        role: String,
        reply: serde_json::Value,
    },
    Kernel(Event),
    Plan {
        node: NodeId,
        generation: u64,
        paths: Vec<String>,
        fits: bool,
    },
    Made {
        node: NodeId,
        generation: u64,
        sha: String,
    },
    Reviewed {
        node: NodeId,
        generation: u64,
        sha: String,
    },
    Rebased {
        node: NodeId,
        generation: u64,
        sha: String,
    },
    Merged {
        node: NodeId,
        sha: String,
        checks_passed: bool,
    },
    PullRequest {
        node: NodeId,
        number: u64,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Completion {
    pub ack: Ack,
    pub observations: Vec<Observation>,
}

impl Completion {
    pub(crate) fn releases_holds(&self) -> bool {
        !self.observations.iter().any(|observation| {
            matches!(
                observation,
                Observation::Progress { .. }
                    | Observation::Quota { .. }
                    | Observation::Kernel(Event::Fact {
                        fact: tl3_kernel::Fact::UpstreamStalled,
                        ..
                    })
            )
        })
    }

    pub fn confirmed(observations: Vec<Observation>) -> Self {
        Self {
            ack: Ack::Confirmed,
            observations,
        }
    }
}

pub enum Look {
    Found(Completion),
    Absent,
    Pending,
    /// An authoritative live record. Progress is an upstream-owned checkpoint.
    Alive {
        progress: String,
    },
    Healthy,
    Quota {
        reset_ms: u64,
    },
}

pub enum Reconciliation {
    Found(Completion),
    /// Authoritative absence, with any earlier executor attempt known to have ended.
    Absent,
    Pending,
}

/// Implementations fence epochs at their upstream boundary. Unknown outcomes
/// remain pending; only the upstream can establish absence or a terminal result.
pub trait Executor {
    /// Return Err when the response is unknown. Only upstream answers are acknowledgements.
    fn execute(&mut self, delivery: &Delivery) -> Result<Completion>;
    /// Reads by the original key and epoch remain valid after a fence advances.
    fn reconcile(&mut self, delivery: &Delivery) -> Result<Reconciliation>;
    fn look(&mut self, delivery: &Delivery) -> Result<Look> {
        Ok(match self.reconcile(delivery)? {
            Reconciliation::Found(completion) => Look::Found(completion),
            Reconciliation::Absent => Look::Absent,
            Reconciliation::Pending => Look::Pending,
        })
    }
    /// A separate health probe, when an incident's failed operation has ended.
    fn probe(&mut self, _delivery: &Delivery) -> Result<bool> {
        Ok(false)
    }
    /// Resume an owner-confirmed quota ending under the same operation key.
    fn resume_after_quota(&mut self, _delivery: &Delivery) -> Result<Option<Completion>> {
        Ok(None)
    }
    /// Success proves that the exact stalled attempt has ended.
    fn stop_stalled(&mut self, _delivery: &Delivery) -> Result<Completion> {
        Err(crate::Error::Invalid("adapter cannot stop a stalled run"))
    }
    /// Recover missed authenticated webhooks, returning a durable delivery watermark.
    fn recover_deliveries(&mut self, _after: Option<u64>) -> Result<Option<u64>> {
        Ok(None)
    }
    /// Reclaim upstream evidence after the controller has drained durable work.
    fn retire(&mut self, _store: &mut tl3_store::Store) -> Result<()> {
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, Eq, PartialEq, Ord, PartialOrd)]
pub enum Upstream {
    GitHub,
    Mergify,
    PrismaBuild,
    Workers,
    Worktrees,
}

pub fn upstream(output: Out) -> Upstream {
    use tl3_kernel::EffectKind::*;
    match output {
        Out::Notice { .. } => Upstream::GitHub,
        Out::Effect { kind, .. } => match kind {
            StartRun(_) | StopRun => Upstream::Workers,
            SubmitChecks | WithdrawJob => Upstream::PrismaBuild,
            StartRebase | PrepareWorktree | Push | RemoveWorktree => Upstream::Worktrees,
            PostApproval | OpenPr | ClosePr => Upstream::GitHub,
            Enqueue => Upstream::Mergify,
        },
    }
}

#[derive(Default)]
pub struct Registry(std::collections::BTreeMap<Upstream, Box<dyn Executor>>);

impl Registry {
    pub(crate) fn retire(&mut self, store: &mut tl3_store::Store) -> Result<()> {
        for adapter in self.0.values_mut() {
            adapter.retire(store)?;
        }
        Ok(())
    }
    pub fn register(&mut self, upstream: Upstream, executor: impl Executor + 'static) {
        self.0.insert(upstream, Box::new(executor));
    }

    pub(crate) fn get(&mut self, upstream: Upstream) -> Result<&mut (dyn Executor + '_)> {
        match self.0.get_mut(&upstream) {
            Some(adapter) => Ok(adapter.as_mut()),
            None => Err(crate::Error::Invalid("missing upstream adapter")),
        }
    }

    pub(crate) fn adapter(&mut self, delivery: &Delivery) -> Result<&mut (dyn Executor + '_)> {
        match self.0.get_mut(&upstream(delivery.output)) {
            Some(adapter) => Ok(adapter.as_mut()),
            None => Err(crate::Error::Invalid("missing upstream adapter")),
        }
    }
}
