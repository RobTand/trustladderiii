//! Bounded diagnostics. Adapter error prose is never retained or rendered.
use crate::{
    Error, Result,
    executor::{Delivery, Output},
};
use serde::{Deserialize, Serialize};
use tl3_kernel::EffectKind;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "code", rename_all = "snake_case")]
pub enum Reason {
    BranchMismatch {
        required: Option<String>,
        prefix: Option<String>,
        generated: String,
    },
    BodyObligation,
    NonDraftObligation,
    MissingIndependentEvidence,
    BodyPolicy {
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        rules: Vec<RuleFailure>,
        #[serde(default, skip_serializing_if = "is_zero")]
        omitted_rules: usize,
    },
    BodySize,
    Permission,
    NotFound,
    InvalidRequest,
    Quota,
    UpstreamFailure,
    UnmetPublicationObligation,
    HistoricalCauseUnavailable,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuleCategory {
    Missing,
    Duplicate,
    Conflicting,
    InvalidValue,
    IncompatibleObligations,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RuleFailure {
    pub label: String,
    pub category: RuleCategory,
    pub expected_count: usize,
    pub found_count: usize,
    pub distinct_count: usize,
    pub invalid_count: usize,
    pub allowed_value_count: usize,
}

fn is_zero(value: &usize) -> bool {
    *value == 0
}

#[derive(Debug, thiserror::Error)]
#[error("local publication validation refused")]
pub(crate) struct LocalRefusal(pub Reason);

impl LocalRefusal {
    pub(crate) fn error(class: &'static str, reason: Reason) -> Error {
        Error::Permanent {
            class,
            source: Box::new(Self(reason)),
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub(crate) enum Endpoint {
    Issue,
    Pulls,
    Pull,
    CheckRuns,
    HeadChecks,
}
impl Endpoint {
    fn template(self) -> &'static str {
        match self {
            Self::Issue => "GET /repos/{owner}/{repo}/issues/{issue}",
            Self::Pulls => "GET /repos/{owner}/{repo}/pulls",
            Self::Pull => "PATCH /repos/{owner}/{repo}/pulls/{pull_number}",
            Self::CheckRuns => "POST /repos/{owner}/{repo}/check-runs",
            Self::HeadChecks => "GET /repos/{owner}/{repo}/commits/{head}/check-runs",
        }
    }
}

#[derive(Debug, thiserror::Error)]
#[error("publication HTTP operation failed")]
struct HttpOperation {
    endpoint: &'static str,
    #[source]
    source: tl3_github::Error,
}

pub(crate) fn http(error: tl3_github::Error, endpoint: Endpoint) -> Error {
    http_at(error, endpoint.template())
}
pub(crate) fn open_pr(error: tl3_github::Error) -> Error {
    http_at(error, "POST /repos/{owner}/{repo}/pulls")
}
fn http_at(error: tl3_github::Error, endpoint: &'static str) -> Error {
    if status(&error).is_none() {
        return error.into();
    }
    let endpoint = if matches!(error, tl3_github::Error::InstallationToken { .. }) {
        "POST /app/installations/{installation}/access_tokens"
    } else {
        endpoint
    };
    let class = match error.failure_class() {
        tl3_github::FailureClass::Permanent(class) => class,
        _ => return error.into(),
    };
    Error::Permanent {
        class,
        source: Box::new(HttpOperation {
            endpoint,
            source: error,
        }),
    }
}

fn status(error: &tl3_github::Error) -> Option<u16> {
    match error {
        tl3_github::Error::Http { status } => Some(*status),
        tl3_github::Error::CatchUp { source, .. }
        | tl3_github::Error::InstallationToken { source } => status(source),
        _ => None,
    }
}
pub(crate) fn http_status(error: &Error) -> Option<u16> {
    let Error::Permanent { source, .. } = error else {
        return None;
    };
    source
        .downcast_ref::<HttpOperation>()
        .and_then(|op| status(&op.source))
        .or_else(|| source.downcast_ref::<tl3_github::Error>().and_then(status))
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Origin {
    LocalValidation,
    GithubHttp,
    Unavailable,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Diagnostic {
    pub effect_kind: EffectKind,
    pub effect_key: String,
    pub repository: String,
    pub issue: u64,
    pub node: u64,
    pub head: Option<String>,
    pub class: String,
    pub origin: Origin,
    pub reason: Reason,
    pub status: Option<u16>,
    pub endpoint: Option<String>,
}

pub(crate) fn handles(delivery: &Delivery) -> bool {
    matches!(
        delivery.output,
        Output::Effect {
            kind: EffectKind::OpenPr | EffectKind::PostApproval | EffectKind::ClosePr,
            ..
        }
    )
}

impl Diagnostic {
    pub(crate) fn capture(delivery: &Delivery, error: Option<&Error>, class: &str) -> Result<Self> {
        let Output::Effect { kind, .. } = delivery.output else {
            return Err(Error::Invalid("publication diagnostic needs an effect"));
        };
        let source = match error {
            Some(Error::Permanent { source, .. }) => Some(source),
            _ => None,
        };
        let local = source.and_then(|source| source.downcast_ref::<LocalRefusal>());
        let operation = source.and_then(|source| source.downcast_ref::<HttpOperation>());
        let github = source.and_then(|source| source.downcast_ref::<tl3_github::Error>());
        let safe_class = match class {
            "pr_evidence"
            | "pr_obligation"
            | "pr_policy"
            | "permission"
            | "not_found"
            | "invalid_request"
            | "quota"
            | "upstream_server"
            | "invalid_branch"
            | "invalid_commit"
            | "invalid_remote"
            | "invalid_worktree_path"
            | "invalid_token"
            | "invalid_response"
            | "missing_branch"
            | "reserved_branch"
            | "worktree_occupied"
            | "dirty_worktree"
            | "worktree_recovery"
            | "actions_write_required"
            | "actions_run_expired"
            | "actions_refused"
            | "setup" => class,
            _ => "publication_failed",
        };
        let reason = if let Some(local) = local {
            local.0.clone()
        } else {
            match safe_class {
                "pr_evidence" => Reason::MissingIndependentEvidence,
                "pr_obligation" => Reason::UnmetPublicationObligation,
                "pr_policy" => Reason::BodyPolicy {
                    rules: Vec::new(),
                    omitted_rules: 0,
                },
                "permission" => Reason::Permission,
                "not_found" => Reason::NotFound,
                "invalid_request" => Reason::InvalidRequest,
                "quota" => Reason::Quota,
                "upstream_server" => Reason::UpstreamFailure,
                _ => Reason::HistoricalCauseUnavailable,
            }
        };
        // Only bounded, redacted identities from typed local refusals can carry text.
        let reason = match reason {
            Reason::BranchMismatch {
                required,
                prefix,
                generated,
            } => Reason::BranchMismatch {
                required: required.map(|v| crate::evidence::label(&v)).transpose()?,
                prefix: prefix.map(|v| crate::evidence::label(&v)).transpose()?,
                generated: crate::evidence::label(&generated)?,
            },
            Reason::BodyPolicy {
                rules,
                omitted_rules,
            } => {
                let omitted_rules = omitted_rules.saturating_add(rules.len().saturating_sub(16));
                let rules = rules
                    .into_iter()
                    .take(16)
                    .map(|mut rule| {
                        rule.label = safe_identity(&rule.label)?.chars().take(80).collect();
                        Ok(rule)
                    })
                    .collect::<Result<Vec<_>>>()?;
                Reason::BodyPolicy {
                    rules,
                    omitted_rules,
                }
            }
            other => other,
        };
        let origin =
            if local.is_some() || matches!(class, "pr_evidence" | "pr_obligation" | "pr_policy") {
                Origin::LocalValidation
            } else if operation.is_some() || github.is_some_and(|error| status(error).is_some()) {
                Origin::GithubHttp
            } else {
                Origin::Unavailable
            };
        Ok(Self {
            effect_kind: kind,
            effect_key: safe_identity(&delivery.key)?,
            repository: safe_identity(&format!(
                "{}/{}",
                delivery.repository.owner, delivery.repository.name
            ))?,
            issue: delivery.issue,
            node: delivery.node().0,
            head: delivery
                .head_sha
                .as_ref()
                .filter(|sha| tl3_worktrees::CommitId::new(sha.as_str()).is_ok())
                .cloned(),
            class: safe_class.into(),
            origin,
            reason,
            status: error.and_then(http_status),
            endpoint: operation.map(|op| op.endpoint.into()),
        })
    }

    pub(crate) fn render(&self) -> String {
        let origin = match self.origin {
            Origin::LocalValidation => {
                "local validation; no HTTP operation caused this refusal".into()
            }
            Origin::GithubHttp => format!(
                "GitHub HTTP{}{}",
                self.status.map(|s| format!(" {s}")).unwrap_or_default(),
                self.endpoint
                    .as_ref()
                    .map(|e| format!("; {e}"))
                    .unwrap_or_else(|| "; endpoint unavailable".into())
            ),
            Origin::Unavailable => {
                "historical cause unavailable; HTTP status and endpoint unknown".into()
            }
        };
        let reason = match &self.reason {
            Reason::BranchMismatch {
                required,
                prefix,
                generated,
            } => format!(
                "branch mismatch; required exact {}, prefix {}; generated {generated}",
                required.as_deref().unwrap_or("unspecified"),
                prefix.as_deref().unwrap_or("unspecified")
            ),
            other => serde_json::to_string(other).expect("bounded reason"),
        };
        format!(
            "Publication diagnostic: {} {:?}, key {}, repository {}, issue {}, request {}, head {}. {origin}. Reason: {reason}.",
            self.class,
            self.effect_kind,
            self.effect_key,
            self.repository,
            self.issue,
            self.node,
            self.head.as_deref().unwrap_or("unavailable")
        )
    }
}

/// Read the diagnostic written by this transaction before the resulting incident.
pub(crate) fn retained(
    snapshot: &tl3_store::Snapshot,
    commit: &tl3_store::Commit,
    key: &str,
) -> Result<Option<Diagnostic>> {
    commit
        .rows
        .iter()
        .rev()
        .find_map(|row| match row {
            tl3_store::RowChange::Put { table, id, value }
                if table == "publication_failures" && id == key =>
            {
                Some(value)
            }
            _ => None,
        })
        .or_else(|| {
            snapshot
                .rows
                .iter()
                .find(|row| row.table == "publication_failures" && row.id == key)
                .map(|row| &row.value)
        })
        .map(|value| crate::decode_stored(value.clone()))
        .transpose()
}

impl crate::Controller {
    pub(crate) fn record_publication_refusal(
        &mut self,
        delivery: &Delivery,
        error: &Error,
    ) -> Result<()> {
        if !handles(delivery) || !matches!(error, Error::Permanent { .. }) {
            return Ok(());
        }
        let diagnostic =
            Diagnostic::capture(delivery, Some(error), crate::looks::failure_class(error))?;
        let value = serde_json::json!(diagnostic);
        let snapshot = self.store.load()?;
        if snapshot.rows.iter().any(|row| {
            row.table == "publication_failures" && row.id == delivery.key && row.value == value
        }) {
            return Ok(());
        }
        eprintln!("{}", diagnostic.render());
        let mut commit = tl3_store::Commit::new(
            format!(
                "publication-refusal:{}:{}",
                delivery.key,
                snapshot.revision.get()
            ),
            serde_json::json!({"diagnostic":diagnostic}),
            &snapshot,
        );
        commit.rows.push(crate::put(
            "publication_failures",
            delivery.key.clone(),
            value,
        ));
        self.store.commit(&commit)?;
        Ok(())
    }
}

fn safe_identity(value: &str) -> Result<String> {
    Ok(serde_json::from_str(&crate::evidence::label(value)?)?)
}
