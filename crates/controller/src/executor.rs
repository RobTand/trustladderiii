use serde::{Deserialize, Serialize};
use tl3_kernel::{Ack, EffectKind, Event, HeadId, NodeId, Notice};

use crate::Result;

/// Deliveries include ingress work that never enters the kernel.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub enum Output {
    Effect {
        node: NodeId,
        seq: u64,
        kind: EffectKind,
        head: Option<HeadId>,
    },
    Notice {
        node: NodeId,
        notice: Notice,
    },
    Comment {
        node: NodeId,
        kind: CommentKind,
    },
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub enum CommentKind {
    Classify,
    Clarify,
    FileCorrection,
    ReplacementDropped,
}

impl From<tl3_kernel::Out> for Output {
    fn from(output: tl3_kernel::Out) -> Self {
        match output {
            tl3_kernel::Out::Effect {
                node,
                seq,
                kind,
                head,
            } => Self::Effect {
                node,
                seq,
                kind,
                head,
            },
            tl3_kernel::Out::Notice { node, notice } => Self::Notice { node, notice },
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
/// Adapter tickets add the payload fields their clients need here.
pub struct Delivery {
    pub key: String,
    pub epoch: i64,
    pub output: Output,
    pub generation: u64,
    pub head_sha: Option<String>,
    pub repository: crate::Repository,
    pub issue: u64,
    pub pull_request: Option<u64>,
    pub paths: Vec<String>,
    pub attempt: u8,
    pub round: u8,
    #[serde(default)]
    pub replanned: bool,
    #[serde(default)]
    pub checks: Option<crate::adapters::prismabuild::CheckRun>,
    #[serde(default)]
    pub job_generation: Option<u64>,
    #[serde(default)]
    pub question: Option<tl3_kernel::Question>,
    #[serde(default)]
    pub note: Option<String>,
    #[serde(default)]
    pub scope_removals: Option<String>,
    #[serde(default)]
    pub marker_key: Option<String>,
    #[serde(default)]
    pub question_context: Option<serde_json::Value>,
    #[serde(default)]
    pub steering: Option<crate::steering::Task>,
    #[serde(default)]
    pub request_text: Option<String>,
}

impl Delivery {
    pub(crate) fn into_effect(self) -> tl3_store::NewEffect {
        tl3_store::NewEffect {
            key: self.key.clone(),
            kind: upstream(self.output).kind().into(),
            payload: serde_json::json!(self),
        }
    }
    pub fn marker_key(&self) -> &str {
        self.marker_key.as_deref().unwrap_or(&self.key)
    }
    pub fn node(&self) -> NodeId {
        match self.output {
            Output::Effect { node, .. }
            | Output::Notice { node, .. }
            | Output::Comment { node, .. } => node,
        }
    }
}

/// External values are interned by the controller before entering the kernel.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Observation {
    ScratchSaved {
        node: NodeId,
        reference: String,
    },
    ScratchPruned {
        reference: String,
    },
    Progress {
        key: String,
        checkpoint: String,
    },
    Quota {
        key: String,
        reset_ms: u64,
    },
    Classified {
        key: String,
        reply: Option<serde_json::Value>,
    },
    LinkedRequest {
        repository: crate::Repository,
        issue: tl3_github::Issue,
        original: NodeId,
    },
    Queue(crate::adapters::mergify::QueueFact),
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
    CiFailed {
        node: NodeId,
        infra: bool,
        checks: Vec<crate::findings::CiFailure>,
    },
    CheckPlan {
        node: NodeId,
        sha: String,
        run: crate::adapters::prismabuild::CheckRun,
    },
    Checks {
        node: NodeId,
        generation: u64,
        action_key: String,
        base_sha: String,
        sha: String,
        changed_paths: Vec<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        failed_tests: Vec<crate::findings::FailedTest>,
        report: bool,
        pass: bool,
        #[serde(default)]
        results: Vec<serde_json::Value>,
    },
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
    RebaseConflict {
        node: NodeId,
        generation: u64,
        head: String,
        target: String,
        files: Vec<String>,
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

/// The upstream record read by a look. Equality means no observed progress.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum ProgressToken {
    Mergify(crate::adapters::mergify::Progress),
}

/// Stored in a watch's `progress` field. The first read has no previous token.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct WatchProgress {
    pub previous: Option<ProgressToken>,
    pub current: ProgressToken,
}

impl WatchProgress {
    pub fn changed(&self) -> bool {
        self.previous.as_ref() != Some(&self.current)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Completion {
    pub ack: Ack,
    #[serde(default)]
    pub failure_class: Option<String>,
    /// Progress from this read, even when it produces no kernel observations.
    #[serde(default)]
    pub progress: Option<ProgressToken>,
    pub observations: Vec<Observation>,
}

impl Completion {
    pub fn permanent(class: impl Into<String>) -> Self {
        Self {
            ack: Ack::Permanent,
            failure_class: Some(class.into()),
            progress: None,
            observations: vec![],
        }
    }
    pub(crate) fn releases_holds(&self) -> bool {
        if self.progress.is_some() && self.observations.is_empty() {
            return false;
        }
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
            failure_class: None,
            ack: Ack::Confirmed,
            progress: None,
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
    Waiting {
        wait: tl3_store::ExternalWait,
        checkout: std::path::PathBuf,
    },
    Healthy,
    Quota {
        reset_ms: u64,
    },
}

/// Only the outside owner can establish that a declared wait has ended.
pub enum WaitRecord {
    Alive,
    Ended,
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
    fn implementation(&self) -> &'static str {
        std::any::type_name::<Self>()
    }
    fn watch_wait(
        &mut self,
        _delivery: &Delivery,
        _wait: Option<&tl3_store::ExternalWait>,
    ) -> Result<()> {
        Ok(())
    }
    fn wait_record(&mut self, _key: &str, _checkout: &std::path::Path) -> Result<WaitRecord> {
        Ok(WaitRecord::Ended)
    }
    /// Return Err when the response is unknown. Only upstream answers are acknowledgements.
    fn execute(&mut self, delivery: &Delivery) -> Result<Completion>;
    /// Reads by the original key and epoch remain valid after a fence advances.
    fn reconcile(&mut self, delivery: &Delivery) -> Result<Reconciliation>;
    /// End a cancelled attempt. Confirmed leaves undoing any write to teardown.
    fn reconcile_cancellation(&mut self, delivery: &Delivery) -> Result<Reconciliation> {
        self.reconcile(delivery)
    }
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
    fn recover_deliveries(&mut self, _after: u64, _since_ms: u64) -> Result<Option<u64>> {
        Ok(None)
    }
    /// Idempotent cleanup after dispatch and watch retirement are durable.
    fn retire_delivery(&mut self, _delivery: &Delivery) -> Result<()> {
        Ok(())
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

impl Upstream {
    pub const fn kind(self) -> &'static str {
        match self {
            Self::GitHub => "GitHub",
            Self::Mergify => "Mergify",
            Self::PrismaBuild => "PrismaBuild",
            Self::Workers => "Workers",
            Self::Worktrees => "Worktrees",
        }
    }
}

pub fn upstream(output: Output) -> Upstream {
    use tl3_kernel::EffectKind::*;
    match output {
        Output::Comment {
            kind: CommentKind::Classify,
            ..
        } => Upstream::Workers,
        Output::Comment { .. } | Output::Notice { .. } => Upstream::GitHub,
        Output::Effect { kind, .. } => match kind {
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
    pub fn registered_types(&self) -> std::collections::BTreeMap<Upstream, &'static str> {
        self.0
            .iter()
            .map(|(upstream, adapter)| (*upstream, adapter.implementation()))
            .collect()
    }
    pub(crate) fn retire(&mut self, store: &mut tl3_store::Store) {
        for (upstream, adapter) in &mut self.0 {
            if let Err(error) = adapter.retire(store) {
                eprintln!("adapter retirement failed for {upstream:?}: {error}");
            }
        }
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

/// Cloned HTTP clients keep connection tasks on the runtime that first used them.
pub(crate) fn runtime() -> Result<&'static tokio::runtime::Runtime> {
    static RUNTIME: std::sync::LazyLock<std::io::Result<tokio::runtime::Runtime>> =
        std::sync::LazyLock::new(|| {
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .max_blocking_threads(2)
                .enable_all()
                .build()
        });
    match &*RUNTIME {
        Ok(runtime) => Ok(runtime),
        Err(error) => Err(std::io::Error::new(error.kind(), error.to_string()).into()),
    }
}
