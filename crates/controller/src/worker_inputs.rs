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

pub(crate) fn retained(snapshot: &Snapshot, commit: &Commit, key: &str) -> Result<Option<Failure>> {
    let value = commit
        .rows
        .iter()
        .rev()
        .find_map(|row| match row {
            RowChange::Put { table, id, value }
                if table == "worker_input_failures" && id == key =>
            {
                Some(value)
            }
            _ => None,
        })
        .or_else(|| {
            snapshot
                .rows
                .iter()
                .find(|row| row.table == "worker_input_failures" && row.id == key)
                .map(|row| &row.value)
        });
    value.cloned().map(crate::decode_stored).transpose()
}

pub(crate) fn append(summary: &mut String, failure: Option<&Failure>) {
    if let Some(failure) = failure {
        summary.push(' ');
        summary.push_str(&failure.to_string());
    }
}
