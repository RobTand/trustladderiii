#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct AllowanceState {
    pub delivery: Delivery,
    #[serde(default)]
    pub maintenance: bool,
    due_ms: Option<u64>,
    progress: Option<String>,
    progress_ms: u64,
    quota_ms: Option<u64>,
    #[serde(default)]
    last_quota_ms: Option<u64>,
    #[serde(default)]
    last_quota_model: Option<String>,
    /// #759: since this step's first model ending with no model turn served
    /// since. A progress checkpoint clears it.
    #[serde(default)]
    unserved_since_ms: Option<u64>,
    /// #759: the operations note for this run of endings is posted.
    #[serde(default)]
    ladder_out_noticed: bool,
    stopping: bool,
    pub incident: Option<String>,
    #[serde(default)]
    healthy: bool,
    #[serde(default)]
    found: Option<String>,
    #[serde(default)]
    failure: Option<String>,
    #[serde(default)]
    read_failures: u8,
    #[serde(default)]
    read_incidents: u8,
    /// When the last failed read was counted.
    #[serde(default)]
    counted_failure_ms: Option<u64>,
    #[serde(default)]
    external_wait: Option<tl3_store::ExternalWait>,
    #[serde(default)]
    wait_ended: bool,
}

impl AllowanceState {
    pub(crate) fn new(delivery: Delivery, now: u64, delay: u64) -> Self {
        Self {
            delivery,
            maintenance: false,
            due_ms: Some(now.saturating_add(delay)),
            progress: None,
            progress_ms: now,
            quota_ms: None,
            last_quota_ms: None,
            last_quota_model: None,
            unserved_since_ms: None,
            ladder_out_noticed: false,
            stopping: false,
            incident: None,
            healthy: false,
            found: None,
            failure: None,
            read_failures: 0,
            read_incidents: 0,
            counted_failure_ms: None,
            external_wait: None,
            wait_ended: false,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Incident {
    pub id: String,
    pub node: Option<NodeId>,
    #[serde(default)]
    pub input_failure: Option<crate::input_errors::InputFailure>,
    pub open: bool,
    pub needs_rob: bool,
    pub blocking: bool,
    pub summary: String,
    pub signature: IncidentSignature,
    #[serde(default)]
    pub effect: Option<String>,
    #[serde(default)]
    pub post_key: Option<String>,
    /// Absent on incidents stored before Q51; see `Incident::cause`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cause: Option<Cause>,
}

/// Who caused an incident (Q51, Rob, 2026-11-01). A new TL3 release clears
/// TL3-side incidents once; upstream incidents keep the probe and Rob rules.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Cause {
    /// GitHub, Mergify, PrismaBuild or the model proxy.
    Upstream,
    /// TL3's own code, prompts, tools or harness environment.
    Tl3,
}

pub(crate) struct AttachedIncident {
    pub upstream: Upstream,
    pub class: String,
    pub diagnostic: Option<crate::publication_failure::Diagnostic>,
    pub input_failure: Option<crate::worker_inputs::Context>,
}

impl Cause {
    /// Workers and worktrees are TL3's own harness: agent runs, reply
    /// validation, Q50 blockers and local Git checkouts. A worktree failure
    /// that reaches GitHub, a Git remote or its token, is upstream. Agent runs
    /// that fail because the model proxy is down still count as TL3-side;
    /// the controller can't yet tell them from worker bugs.
    pub(crate) fn of(upstream: Upstream, class: &str) -> Self {
        if matches!(
            class,
            "worker_input_schema"
                | "redaction_infra"
                | "pr_evidence"
                | "pr_obligation"
                | "pr_policy"
        ) {
            return Cause::Tl3;
        }
        match upstream {
            Upstream::Workers => Cause::Tl3,
            Upstream::Worktrees if matches!(class, "git_remote" | "invalid_token") => {
                Cause::Upstream
            }
            Upstream::Worktrees => Cause::Tl3,
            // The trusted check harness is TL3's code, running inside a PrismaBuild action.
            Upstream::PrismaBuild if class == crate::adapters::prismabuild::HARNESS => Cause::Tl3,
            Upstream::GitHub | Upstream::Mergify | Upstream::PrismaBuild => Cause::Upstream,
        }
    }
}

impl Incident {
    pub(crate) fn cause(&self) -> Cause {
        self.cause.unwrap_or(if self.input_failure.is_some() {
            Cause::Upstream
        } else {
            Cause::of(self.signature.upstream, &self.signature.class)
        })
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(try_from = "StoredSignature")]
pub(crate) struct IncidentSignature {
    pub upstream: Upstream,
    pub class: String,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum StoredSignature {
    Structured { upstream: Upstream, class: String },
    Legacy(String),
}

impl TryFrom<StoredSignature> for IncidentSignature {
    type Error = &'static str;

    fn try_from(value: StoredSignature) -> std::result::Result<Self, Self::Error> {
        let (upstream, class) = match value {
            StoredSignature::Structured { upstream, class } => (upstream, class),
            StoredSignature::Legacy(signature) => {
                let (source, class) = signature
                    .split_once(':')
                    .ok_or("invalid incident signature")?;
                let upstream = [
                    Upstream::GitHub,
                    Upstream::Mergify,
                    Upstream::PrismaBuild,
                    Upstream::Workers,
                    Upstream::Worktrees,
                ]
                .into_iter()
                .find(|upstream| upstream.kind() == source)
                .ok_or("unknown incident upstream")?;
                (upstream, class.into())
            }
        };
        Ok(Self { upstream, class })
    }
}

pub(crate) enum IncidentCause {
    Kernel { needs_rob: bool },
    ReadFailure { incidents: u64 },
    NoticeFailure { incidents: u64 },
}

