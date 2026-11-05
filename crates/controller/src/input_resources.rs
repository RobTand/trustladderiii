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
    #[doc(hidden)]
    pub fn prerequisite(repo: &Repository, kind: PrerequisiteKind, number: u64) -> Self {
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
#[doc(hidden)]
pub fn github(error: tl3_github::Error, resource: Resource, installation: u64) -> Error {
    if error.failure_class().as_str() != "not_found" {
        return error.into();
    }
    fn requested(error: &tl3_github::Error, resource: Resource, installation: u64) -> Resource {
        match error {
            tl3_github::Error::InstallationToken { .. } => {
                Resource::GithubInstallation { installation }
            }
            tl3_github::Error::CatchUp { endpoint, source } => requested(
                source,
                Resource::GithubEndpoint {
                    path: endpoint.clone(),
                },
                installation,
            ),
            _ => resource,
        }
    }
    let requested = requested(&error, resource, installation);
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

#[doc(hidden)]
pub fn of(error: &Error) -> Option<&Failure> {
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

/// #815/#833: retain catch-up's safe endpoint/status diagnostic, never arbitrary adapter prose.
pub(crate) fn catch_up_diagnostic(error: &Error) -> String {
    let Error::Permanent { source, .. } = error else {
        return String::new();
    };
    let source = source
        .downcast_ref::<LookupError>()
        .map(|error| &error.source)
        .or_else(|| source.downcast_ref::<tl3_github::Error>());
    match source {
        Some(error @ tl3_github::Error::CatchUp { .. }) => format!(": {error}"),
        _ => String::new(),
    }
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

fn recorded(snapshot: &Snapshot, node: NodeId) -> Result<Option<Context>> {
    snapshot
        .rows
        .iter()
        .find(|row| row.table == "input_resource_contexts" && row.id == node.0.to_string())
        .map(|row| crate::decode_stored(row.value.clone()))
        .transpose()
}

/// Q7/Q51/#833: rejection feedback survives launches until ingest accepts a valid reply.
pub(crate) fn awaiting_correction(snapshot: &Snapshot, node: NodeId) -> Result<bool> {
    Ok(recorded(snapshot, node)?
        .is_some_and(|context| matches!(context.failure.problem, Problem::KindMismatch { .. })))
}

/// Q7/Q51/#833: send only typed, adapter-owned identities through ingest's existing feedback.
pub(crate) fn retry_feedback(
    snapshot: &Snapshot,
    node: NodeId,
    generation: u64,
) -> Result<Option<Failure>> {
    Ok(recorded(snapshot, node)?
        .filter(|context| {
            context.generation < generation
                && matches!(context.failure.problem, Problem::KindMismatch { .. })
        })
        .map(|context| context.failure))
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

#[cfg(test)]
mod tests {
    use super::*;

    /// #833: token acquisition failures name the installation even inside catch-up reads.
    #[test]
    fn installation_404_keeps_its_identity_and_status_through_wrappers() {
        let error = github(
            tl3_github::Error::CatchUp {
                endpoint: "repos/owner/sandbox/issues".into(),
                source: Box::new(tl3_github::Error::InstallationToken {
                    source: Box::new(tl3_github::Error::Http { status: 404 }),
                }),
            },
            Resource::GithubIssue {
                repository: "owner/sandbox".into(),
                number: 12,
            },
            3,
        );
        assert_eq!(
            of(&error).unwrap().requested,
            Resource::GithubInstallation { installation: 3 }
        );
        assert!(error.is_github_not_found());
    }

    /// #833: endpoint provenance is structured metadata, rather than error-message parsing.
    #[test]
    fn catch_up_404_keeps_the_requested_endpoint() {
        let error = github(
            tl3_github::Error::CatchUp {
                endpoint: "repos/owner/sandbox/issues/comments".into(),
                source: Box::new(tl3_github::Error::Http { status: 404 }),
            },
            Resource::GithubEndpoint {
                path: "repos/owner/sandbox".into(),
            },
            3,
        );
        let failure = of(&error).unwrap();
        assert_eq!(
            failure.requested,
            Resource::GithubEndpoint {
                path: "repos/owner/sandbox/issues/comments".into()
            }
        );
        assert_eq!(
            serde_json::from_value::<Failure>(serde_json::to_value(failure).unwrap()).unwrap(),
            *failure
        );
    }
}
