//! #833/Q59: resource identities survive input construction and reply validation.
use crate::{Error, Repository, Result, put};
use serde::{Deserialize, Serialize};
use std::fmt;
use tl3_kernel::{NodeId, PrerequisiteKind};
use tl3_store::{Commit, RowChange, Snapshot};

pub(crate) const MISMATCH: &str = "prerequisite_kind_mismatch";

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Resource {
    GithubIssue { repository: String, number: u64 },
    GithubPullRequest { repository: String, number: u64 },
    GithubIssueComments { repository: String, number: u64 },
    GithubInstallation { installation: u64 },
    GithubEndpoint { path: String },
}

impl Resource {
    pub(crate) fn prerequisite(repo: &Repository, kind: PrerequisiteKind, number: u64) -> Self {
        let repository = format!("{}/{}", repo.owner, repo.name);
        match kind {
            PrerequisiteKind::Issue => Self::GithubIssue { repository, number },
            PrerequisiteKind::PullRequest => Self::GithubPullRequest { repository, number },
        }
    }
}

impl fmt::Display for Resource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::GithubIssue { repository, number } => {
                write!(f, "GitHub issue {repository}#{number}")
            }
            Self::GithubPullRequest { repository, number } => {
                write!(f, "GitHub pull request {repository}#{number}")
            }
            Self::GithubIssueComments { repository, number } => {
                write!(f, "GitHub issue comments {repository}#{number}")
            }
            Self::GithubInstallation { installation } => {
                write!(f, "GitHub installation {installation}")
            }
            Self::GithubEndpoint { path } => write!(f, "GitHub API resource {path}"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Problem {
    NotFound,
    KindMismatch { observed: Resource },
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize, thiserror::Error)]
pub struct Failure {
    pub requested: Resource,
    pub problem: Problem,
}

impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.problem {
            Problem::NotFound => write!(f, "Missing {}.", self.requested),
            Problem::KindMismatch { observed } => write!(
                f,
                "Requested {}; observed {observed}. The prerequisite declaration must be corrected.",
                self.requested
            ),
        }
    }
}

#[derive(Debug, thiserror::Error)]
#[error("{failure}")]
pub(crate) struct LookupError {
    failure: Failure,
    #[source]
    pub(crate) source: tl3_github::Error,
}

/// Capture metadata supplied by the API caller, never by parsing error prose.
pub(crate) fn github(error: tl3_github::Error, resource: Resource, installation: u64) -> Error {
    if error.failure_class().as_str() != "not_found" {
        return error.into();
    }
    let requested = match &error {
        tl3_github::Error::InstallationToken { .. } => {
            Resource::GithubInstallation { installation }
        }
        tl3_github::Error::CatchUp { endpoint, .. } => Resource::GithubEndpoint {
            path: endpoint.clone(),
        },
        _ => resource,
    };
    Error::Permanent {
        class: "not_found",
        source: Box::new(LookupError {
            failure: Failure {
                requested,
                problem: Problem::NotFound,
            },
            source: error,
        }),
    }
}

pub(crate) fn of(error: &Error) -> Option<&Failure> {
    let source = match error {
        Error::Permanent { source, .. } | Error::Adapter(source) => source,
        _ => return None,
    };
    source.downcast_ref::<Failure>().or_else(|| {
        source
            .downcast_ref::<LookupError>()
            .map(|error| &error.failure)
    })
}

/// Q59: verify identity without changing the declaration's merge/closure semantics.
pub(crate) fn validate(
    repo: &Repository,
    kind: PrerequisiteKind,
    issue: &tl3_github::Issue,
) -> Result<()> {
    let observed = if issue.pull_request.is_some() {
        PrerequisiteKind::PullRequest
    } else {
        PrerequisiteKind::Issue
    };
    if kind != observed {
        return Err(Error::Permanent {
            class: MISMATCH,
            source: Box::new(Failure {
                requested: Resource::prerequisite(repo, kind, issue.number),
                problem: Problem::KindMismatch {
                    observed: Resource::prerequisite(repo, observed, issue.number),
                },
            }),
        });
    }
    Ok(())
}

#[derive(Clone, Deserialize, Serialize)]
pub(crate) struct Context {
    generation: u64,
    pub failure: Failure,
}

pub(crate) fn record(commit: &mut Commit, node: NodeId, generation: u64, failure: &Failure) {
    commit.rows.push(put(
        "input_resource_contexts",
        node.0.to_string(),
        serde_json::json!(Context {
            generation,
            failure: failure.clone()
        }),
    ));
}

pub(crate) fn current(
    snapshot: &Snapshot,
    commit: &Commit,
    node: NodeId,
) -> Result<Option<Failure>> {
    let id = node.0.to_string();
    let value = commit
        .rows
        .iter()
        .rev()
        .find_map(|row| match row {
            RowChange::Put {
                table,
                id: key,
                value,
            } if table == "input_resource_contexts" && key == &id => Some(value),
            _ => None,
        })
        .or_else(|| {
            snapshot
                .rows
                .iter()
                .find(|row| row.table == "input_resource_contexts" && row.id == id)
                .map(|row| &row.value)
        });
    let Some(value) = value else {
        return Ok(None);
    };
    let context: Context = crate::decode_stored(value.clone())?;
    let generation = snapshot
        .rows
        .iter()
        .find(|row| row.table == "nodes" && row.id == id)
        .and_then(|row| row.value["generation"].as_u64());
    Ok((generation == Some(context.generation)).then_some(context.failure))
}
