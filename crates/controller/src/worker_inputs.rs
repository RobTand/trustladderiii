//! Input-contract failures contain schema metadata only, never rejected values.
use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use tl3_store::{Commit, RowChange, Snapshot};

pub(crate) const CLASS: &str = "worker_input_schema";

#[derive(Clone, Debug, Deserialize, Serialize, thiserror::Error)]
#[error("worker inputs break role schema: role={role} pointer={pointer} rule={rule}")]
pub struct Failure {
    role: String,
    pointer: String,
    rule: String,
}

impl Failure {
    pub(crate) fn capture(role: &str, error: &jsonschema::ValidationError<'_>) -> Self {
        use jsonschema::error::ValidationErrorKind;
        if let ValidationErrorKind::AnyOf { context }
        | ValidationErrorKind::OneOfNotValid { context } = error.kind()
            && let Some((_, leaf)) = context
                .iter()
                .filter_map(|branch| {
                    branch
                        .iter()
                        .map(|error| Self::capture(role, error))
                        .max_by_key(|failure| failure.pointer.matches('/').count())
                        .map(|failure| (branch.len(), failure))
                })
                // The closest branch has the fewest violations. Prefer its
                // deepest field when equally close branches remain.
                .min_by_key(|(count, failure)| {
                    (
                        *count,
                        std::cmp::Reverse(failure.pointer.matches('/').count()),
                    )
                })
        {
            return leaf;
        }
        let mut pointer = error.instance_path().to_string();
        let field = match error.kind() {
            ValidationErrorKind::AdditionalProperties { unexpected } => {
                unexpected.first().map(String::as_str)
            }
            ValidationErrorKind::Required { property } => property.as_str(),
            _ => None,
        };
        if let Some(field) = field {
            pointer.push('/');
            pointer.push_str(&field.replace('~', "~0").replace('/', "~1"));
        }
        // Keep the complete diagnostic within the trace's 512-character tag cap.
        let bounded = |text: &str, limit| {
            text.chars()
                .take(limit)
                .map(|c| if c.is_control() { '_' } else { c })
                .collect()
        };
        Self {
            role: bounded(role, 32),
            pointer: bounded(&pointer, 320),
            rule: bounded(error.kind().keyword(), 48),
        }
    }
}

pub(crate) fn of(error: &Error) -> Option<&Failure> {
    match error {
        Error::WorkerInputs(failure) => Some(failure),
        _ => None,
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct Context {
    #[serde(flatten)]
    diagnostic: Failure,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    maker_head: Option<String>,
}

impl Context {
    pub(crate) fn capture(
        failure: &Failure,
        snapshot: &Snapshot,
        delivery: &crate::executor::Delivery,
    ) -> Self {
        let node = delivery.node().0.to_string();
        let preserved = snapshot
            .rows
            .iter()
            .any(|row| row.table == "nodes" && row.id == node && row.value["worktree"] == true);
        let maker_head = delivery
            .head_sha
            .as_ref()
            .filter(|head| {
                failure.role == "reviewer"
                    && preserved
                    && snapshot.rows.iter().any(|row| {
                        row.table == "maker_replies"
                            && row.id == node
                            && row.value["kind"] == "done"
                            && row.value["head"].as_str() == Some(head.as_str())
                    })
            })
            .cloned();
        Self {
            diagnostic: failure.clone(),
            maker_head,
        }
    }

    pub(crate) fn description(&self) -> String {
        let step = match self.diagnostic.role.as_str() {
            "reviewer" => "Review",
            "maker" => "Maker",
            "planner" => "Planning",
            "ingest" => "Request reading",
            "audit" => "Independent request reading",
            "classifier" => "Comment classification",
            "writer" => "Message writing",
            "postmortem" => "Postmortem",
            _ => "Worker",
        };
        let mut description = format!("{step} setup failed.");
        if self.maker_head.is_some() {
            description.push_str(" The maker's change is complete and preserved.");
        }
        description
    }
}

impl std::fmt::Display for Context {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} {}", self.description(), self.diagnostic)
    }
}

fn stored(snapshot: &Snapshot, commit: &Commit, table: &str, key: &str) -> Result<Option<Context>> {
    for row in commit.rows.iter().rev() {
        match row {
            RowChange::Put {
                table: t,
                id,
                value,
            } if t == table && id == key => return crate::decode_stored(value.clone()).map(Some),
            RowChange::Delete { table: t, id } if t == table && id == key => return Ok(None),
            _ => {}
        }
    }
    snapshot
        .rows
        .iter()
        .find(|row| row.table == table && row.id == key)
        .map(|row| crate::decode_stored(row.value.clone()))
        .transpose()
}

pub(crate) fn retained(snapshot: &Snapshot, commit: &Commit, key: &str) -> Result<Option<Context>> {
    stored(snapshot, commit, "worker_input_failures", key)
}

pub(crate) fn current(
    snapshot: &Snapshot,
    commit: &Commit,
    node: tl3_kernel::NodeId,
) -> Result<Option<Context>> {
    stored(
        snapshot,
        commit,
        "worker_input_contexts",
        &node.0.to_string(),
    )
}

pub(crate) fn append(summary: &mut String, failure: Option<&impl std::fmt::Display>) {
    if let Some(failure) = failure {
        summary.push(' ');
        summary.push_str(&failure.to_string());
    }
}
