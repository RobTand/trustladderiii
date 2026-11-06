use serde::{Deserialize, Serialize};
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

impl From<Failure> for tl3_controller::Error {
    fn from(failure: Failure) -> Self {
        Self::WorkerInputs(serde_json::from_value(serde_json::to_value(failure).unwrap()).unwrap())
    }
}
