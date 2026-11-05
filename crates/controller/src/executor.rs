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
    Restart,
    ConsoleNote,
    /// Q54: a code-written line for the coordinator on the operations issue.
    OperationsNote,
    /// #403: a judged postmortem of a node, run by a worker outside the kernel.
    Postmortem,
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
    #[serde(default)]
    pub proposed: Option<crate::adoption::ProposedPullRequest>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence: Option<crate::evidence::Evidence>,
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
/// Observations are event payloads, and a release replays those an older one
/// stored (#371). A field added later must be optional and skip serializing
/// when absent; `observation_shapes` enforces this.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Observation {
    Prerequisite(crate::merge_dependencies::ExternalFact),
    ScratchSaved {
        node: NodeId,
        reference: String,
    },
    ScratchPruned {
        reference: String,
    },
    /// #430: held worker span logs reached their disk cap, so the oldest
    /// unshipped ones were dropped.
    TraceLogsCapped {
        evicted: u64,
    },
    Progress {
        key: String,
        checkpoint: String,
    },
    Quota {
        key: String,
        reset_ms: u64,
        /// The model that reported quota; absent in waits recorded before Q53.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<String>,
    },
    Classified {
        key: String,
        reply: Option<serde_json::Value>,
    },
    /// #403: a postmortem run's validated reply, or its fixed failure class.
    Postmortem {
        key: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reply: Option<serde_json::Value>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        failure: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        judge_failure: Option<tl3_store::JudgeFailure>,
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
    WorkerFailed {
        node: NodeId,
        generation: u64,
        failure_class: String,
        failure_message: String,
    },
    WorkerReply {
        node: NodeId,
        generation: u64,
        role: String,
        reply: serde_json::Value,
        /// The model that ran, so review can stay a different family (Q53).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<String>,
    },
    Kernel(Event),
    /// Commands carry intent; steering allocates their kernel command ID.
    Steering {
        node: NodeId,
        command: tl3_kernel::Command,
    },
    CiFailed {
        node: NodeId,
        infra: bool,
        checks: Vec<crate::findings::CiFailure>,
    },
    WorktreePrepared {
        node: NodeId,
        repository: std::path::PathBuf,
        base_sha: String,
        sha: String,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diagnostic: Option<crate::publication_failure::Diagnostic>,
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
            diagnostic: None,
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
            diagnostic: None,
            failure_class: None,
            ack: Ack::Confirmed,
            progress: None,
            observations,
        }
    }
}

pub enum Look {
    Found(Box<Completion>),
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
    Found(Box<Completion>),
    /// Authoritative absence, with any earlier executor attempt known to have ended.
    Absent,
    Pending,
}

/// Implementations fence epochs at their upstream boundary. Unknown outcomes
/// remain pending; only the upstream can establish absence or a terminal result.
pub trait Executor {
    fn input_incident(
        &mut self,
        _notice: &crate::input_errors::IncidentNotice,
        _epoch: i64,
    ) -> Result<()> {
        Err(crate::Error::Invalid("adapter cannot post input incidents"))
    }
    fn intake_question(
        &mut self,
        _question: &crate::adoption::Clarification,
        _epoch: i64,
    ) -> Result<()> {
        Err(crate::Error::Invalid(
            "adapter cannot clarify adoption intake",
        ))
    }
    fn app_login(&self, _installation: u64) -> Option<String> {
        None
    }
    /// Reconcile a marked message on upstream news, including after confirmation.
    fn reconcile_message(
        &mut self,
        _repository: &crate::Repository,
        _issue: u64,
        _comment: &tl3_github::Comment,
    ) -> Result<()> {
        Ok(())
    }
    /// The issue as GitHub has it now, read fresh (Q39).
    fn issue(
        &mut self,
        _repository: &crate::Repository,
        _number: u64,
    ) -> Result<tl3_github::Issue> {
        Err(crate::Error::Invalid("adapter cannot read issues"))
    }
    /// One read at declaration; subsequent changes arrive through GitHub ingress.
    fn prerequisite(
        &mut self,
        _repository: &crate::Repository,
        _kind: tl3_kernel::PrerequisiteKind,
        _number: u64,
    ) -> Result<tl3_kernel::DependencyStatus> {
        Err(crate::Error::Invalid(
            "adapter cannot read merge prerequisites",
        ))
    }
    /// #817: read identity/termination evidence without executing or reconciling.
    /// Pending alone cannot certify a live process.
    fn dispatch_evidence(
        &mut self,
        _delivery: &Delivery,
    ) -> Result<(crate::dispatch_liveness::ProcessEvidence, bool)> {
        Ok((crate::dispatch_liveness::ProcessEvidence::Unknown, false))
    }
    /// #817: read adapter-owned delivery evidence without I/O mutations or a
    /// new upstream request. Missing evidence leaves the durable retry in charge.
    fn contract_evidence(
        &mut self,
        _delivery: &Delivery,
    ) -> Result<crate::dispatch_liveness::decision::ContractEvidence> {
        Ok(crate::dispatch_liveness::decision::ContractEvidence::Absent)
    }
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
    /// Facts learned while a delivery is still pending, without acknowledging it.
    fn observations(&mut self, _delivery: &Delivery) -> Result<Vec<Observation>> {
        Ok(vec![])
    }
    /// Return Err when the response is unknown. Only upstream answers are acknowledgements.
    fn execute(&mut self, delivery: &Delivery) -> Result<Completion>;
    /// Reads by the original key and epoch remain valid after a fence advances.
    fn reconcile(&mut self, delivery: &Delivery) -> Result<Reconciliation>;
    /// Q27: canonical matching copies observed by this repair, if the adapter can prove it.
    fn repair_approval(&mut self, _delivery: &Delivery) -> Result<(Reconciliation, Option<usize>)> {
        Err(crate::Error::Invalid(
            "executor does not support approval repair",
        ))
    }

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
    /// #773: the existing controller look schedules a Hindsight delivery probe.
    fn probe_memory(&mut self) -> Result<()> {
        Ok(())
    }
    /// #828: whether an operator can name this configured model.
    fn configured_model(&self, _model: &str) -> bool {
        false
    }
    /// #828 / Q53: membership in this operation's currently eligible model ladder.
    fn quota_model_eligible(&mut self, _delivery: &Delivery, _model: &str) -> Result<bool> {
        Ok(false)
    }
    /// Resume an owner-confirmed quota ending under the same operation key.
    fn resume_after_quota(&mut self, _delivery: &Delivery) -> Result<Option<Completion>> {
        Ok(None)
    }
    /// Q53: when a quota-limited operation can resume. An adapter whose step has
    /// another model left resumes it now; otherwise the announced reset stands.
    fn quota_resume_ms(&mut self, _delivery: &Delivery, announced: u64, _now: u64) -> Result<u64> {
        Ok(announced)
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
            kind: CommentKind::Classify | CommentKind::Postmortem,
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

#[cfg(test)]
mod observation_shapes {
    use super::*;
    use serde_json::json;

    /// #371: observations are event payloads, and a release replays the ones an
    /// older release stored. A field added later must serialize like the older
    /// shape when absent, so it needs `skip_serializing_if` with its default.
    #[test]
    fn absent_later_fields_serialize_like_the_older_shape() {
        assert_eq!(
            json!(Observation::Quota {
                key: "effect:0:5".into(),
                reset_ms: 7,
                model: None,
            }),
            json!({"Quota": {"key": "effect:0:5", "reset_ms": 7}})
        );
        assert_eq!(
            json!(Observation::WorkerReply {
                node: NodeId(0),
                generation: 3,
                role: "maker".into(),
                reply: json!({"kind": "done"}),
                model: None,
            }),
            json!({"WorkerReply": {"node": 0, "generation": 3, "role": "maker",
                "reply": {"kind": "done"}}})
        );
    }

    /// Q39 added priority to the kernel's request events and nodes. Unranked,
    /// its default, keeps both in their older shapes.
    #[test]
    fn unranked_priority_serializes_like_the_older_shape() {
        use tl3_kernel::{Event, HeadId, Node, Priority, RepoId};
        let request = |priority| {
            json!(Observation::Kernel(Event::NewRequest {
                repo: RepoId(1),
                priority,
            }))
        };
        assert_eq!(
            request(Priority::Unranked),
            json!({"Kernel": {"NewRequest": {"repo": 1}}})
        );
        assert_eq!(
            request(Priority::P1)["Kernel"]["NewRequest"]["priority"],
            "P1"
        );
        assert_eq!(
            json!(Observation::Kernel(Event::NewAdoption {
                repo: RepoId(1),
                head: HeadId(2),
                priority: Priority::Unranked,
            })),
            json!({"Kernel": {"NewAdoption": {"repo": 1, "head": 2}}})
        );
        let node = Node::new(RepoId(0));
        let stored = json!(node);
        assert!(stored.get("priority").is_none(), "{stored}");
        assert_eq!(serde_json::from_value::<Node>(stored).unwrap(), node);
        let ranked = Node {
            priority: Priority::P2,
            ..node
        };
        assert_eq!(json!(ranked)["priority"], "P2");
    }

    #[test]
    fn every_optional_observation_field_skips_its_default() {
        let body = include_str!("executor.rs")
            .split_once("pub enum Observation {")
            .unwrap()
            .1
            .split_once("\n}\n")
            .unwrap()
            .0;
        // Shipped serialized before the rule; skipping now would change stored shapes.
        let grandfathered = [("Classified", "reply"), ("Checks", "results")];
        let mut variant = "";
        let mut attributes: Vec<&str> = Vec::new();
        let mut checked = 0;
        for line in body.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with("#[") {
                attributes.push(trimmed);
                continue;
            }
            if trimmed.is_empty() || trimmed.starts_with("//") {
                continue;
            }
            if !line.starts_with("        ") {
                variant = trimmed
                    .split(|c: char| !c.is_alphanumeric() && c != '_')
                    .next()
                    .unwrap();
                attributes.clear();
                continue;
            }
            let Some((field, kind)) = trimmed.split_once(':') else {
                attributes.clear();
                continue;
            };
            let defaulted = attributes
                .iter()
                .any(|a| a.starts_with("#[serde(") && a.contains("default"));
            if (defaulted || kind.trim_start().starts_with("Option<"))
                && !attributes.iter().any(|a| a.contains("skip_serializing_if"))
            {
                assert!(
                    grandfathered.contains(&(variant, field)),
                    "observation field {variant}.{field} needs skip_serializing_if, so its absence serializes like the older shape"
                );
            }
            checked += 1;
            attributes.clear();
        }
        assert!(checked > 20, "the guard must see the Observation fields");
    }
}
