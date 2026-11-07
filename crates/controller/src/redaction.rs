//! Controller error conversion for the shared Rust redaction engine.
pub use tl3_redaction::{generated_policy, generated_policy_identity, generated_harness_policy};

pub fn text(value: &str) -> crate::Result<String> {
    tl3_redaction::text(value)
        .map_err(|error| crate::Error::RedactionInfra(std::io::Error::other(error)))
}
