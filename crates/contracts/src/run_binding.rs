//! Authoritative Flux run-binding wire types and their lexical validation.
//! Expected request, profile, tree and ownership checks remain with their callers.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// A lexical binding failure, independent of controller operation errors.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BindingError {
    message: &'static str,
}

impl BindingError {
    fn invalid(message: &'static str) -> Self {
        Self { message }
    }

    pub const fn message(&self) -> &'static str {
        self.message
    }
}

impl std::fmt::Display for BindingError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "InvalidRequest: {}", self.message)
    }
}

impl std::error::Error for BindingError {}

pub fn token(value: &str) -> bool {
    !value.is_empty()
        && !matches!(value, "." | "..")
        && value.len() <= 128
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
}
pub fn hex(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
pub fn generation(value: &str) -> bool {
    token(value)
        || value
            .split_once('/')
            .is_some_and(|(run, invocation)| token(run) && hex(invocation, 32))
}
const JOB_ALPHABET: &[u8; 58] = b"123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";

/// Flux job IDs are u64 identities. Locale-dependent prefixes never cross our boundary.
pub fn canonical_job_id(value: &str) -> Result<String, BindingError> {
    let invalid = || BindingError::invalid("invalid or overflowing Flux job ID");
    let mut number = 0_u64;
    if let Some(digits) = value.strip_prefix('f').or_else(|| value.strip_prefix('ƒ')) {
        if digits.is_empty() || digits.len() > 11 {
            return Err(invalid());
        }
        for digit in digits.bytes() {
            let digit = JOB_ALPHABET
                .iter()
                .position(|b| *b == digit)
                .ok_or_else(invalid)?;
            number = number
                .checked_mul(58)
                .and_then(|n| n.checked_add(digit as u64))
                .ok_or_else(invalid)?;
        }
    } else {
        if value.is_empty() || value.len() > 20 || !value.bytes().all(|b| b.is_ascii_digit()) {
            return Err(invalid());
        }
        number = value.parse().map_err(|_| invalid())?;
    }
    let mut digits = Vec::new();
    loop {
        digits.push(JOB_ALPHABET[(number % 58) as usize]);
        number /= 58;
        if number == 0 {
            break;
        }
    }
    digits.reverse();
    Ok(format!(
        "f{}",
        String::from_utf8(digits).expect("ASCII alphabet")
    ))
}

pub fn f58(value: &str) -> bool {
    (value.starts_with('f') || value.starts_with('ƒ')) && canonical_job_id(value).is_ok()
}

fn deserialize_job_id<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<String, D::Error> {
    canonical_job_id(&String::deserialize(deserializer)?).map_err(serde::de::Error::custom)
}

fn serialize_job_id<S: serde::Serializer>(
    value: &str,
    serializer: S,
) -> std::result::Result<S::Ok, S::Error> {
    canonical_job_id(value)
        .map_err(serde::ser::Error::custom)?
        .serialize(serializer)
}
pub fn absolute(path: &std::path::Path) -> bool {
    path.is_absolute()
        && path
            .to_str()
            .is_some_and(|s| s.len() <= 4096 && !s.chars().any(char::is_control))
        && path.components().all(|c| {
            matches!(
                c,
                std::path::Component::RootDir | std::path::Component::Normal(_)
            )
        })
}

impl BackendId {
    pub fn validate(&self) -> Result<(), BindingError> {
        if !generation(&self.instance)
            || !self.uri.starts_with("local:///")
            || self.uri.len() > 4096
            || self.uri.chars().any(char::is_control)
            || canonical_job_id(&self.job).is_err()
        {
            return Err(BindingError::invalid(
                "expected Flux instance generation and f58 job ID; PB keys are unsupported",
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct BackendId {
    pub backend: BackendTag,
    pub instance: String,
    pub uri: String,
    #[serde(
        deserialize_with = "deserialize_job_id",
        serialize_with = "serialize_job_id"
    )]
    pub job: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BackendTag {
    Flux,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SubmissionIntent {
    pub backend: BackendTag,
    pub instance: String,
    pub uri: String,
    pub owner: String,
    pub correlation: String,
    pub request_digest: String,
    pub output_dir: PathBuf,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RunBinding {
    pub id: BackendId,
    pub intent: SubmissionIntent,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Ending {
    Passed,
    Failed,
    Refused,
    InfraLost,
    Cancelled,
}

impl SubmissionIntent {
    pub fn validate(&self) -> Result<(), BindingError> {
        if !generation(&self.instance)
            || !self.uri.starts_with("local:///")
            || self.uri.len() > 4096
            || self.uri.chars().any(char::is_control)
            || !token(&self.owner)
            || !token(&self.correlation)
            || !hex(&self.request_digest, 64)
            || !absolute(&self.output_dir)
        {
            return Err(BindingError::invalid("invalid Flux submission intent"));
        }
        Ok(())
    }
}
impl RunBinding {
    pub fn validate(&self) -> Result<(), BindingError> {
        self.id.validate()?;
        self.intent.validate()?;
        if self.id.backend != self.intent.backend
            || self.id.instance != self.intent.instance
            || self.id.uri != self.intent.uri
        {
            return Err(BindingError::invalid("Flux identity and intent differ"));
        }
        Ok(())
    }
}
