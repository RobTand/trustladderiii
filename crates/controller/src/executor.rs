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
    GitHubComment {
        repository: crate::Repository,
        issue: u64,
        comment: tl3_github::Comment,
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
    pub fn confirmed(observations: Vec<Observation>) -> Self {
        Self {
            ack: Ack::Confirmed,
            observations,
        }
    }
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
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
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
    pub fn register(&mut self, upstream: Upstream, executor: impl Executor + 'static) {
        self.0.insert(upstream, Box::new(executor));
    }

    pub(crate) fn adapter(&mut self, delivery: &Delivery) -> Result<&mut (dyn Executor + '_)> {
        match self.0.get_mut(&upstream(delivery.output)) {
            Some(adapter) => Ok(adapter.as_mut()),
            None => Err(crate::Error::Invalid("missing upstream adapter")),
        }
    }
}
