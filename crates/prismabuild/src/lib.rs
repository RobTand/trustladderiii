//! PrismaBuild effects and authoritative queue observations for the TL3 controller.

mod bell;
mod queue;
mod submit;

pub use bell::{Changes, Watcher};
pub use queue::{
    AcceptedProgress, ActionRecord, ActionState, Attempt, Lease, MAX_STDERR_TAIL_BYTES,
    Observations, ProgressObservation, RecordRead,
};
pub use submit::{
    Demand, ProgressPhase, Submission, SubmissionStatus, SubmitAction, Tool, Withdrawal,
};

use serde::{Deserialize, Serialize};
use std::{path::PathBuf, str::FromStr};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("invalid PrismaBuild input: {0}")]
    InvalidInput(String),
    #[error("PrismaBuild I/O at {path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("invalid PrismaBuild record at {path}: {reason}")]
    Record { path: PathBuf, reason: String },
    #[error(
        "PrismaBuild tool exited with {code:?}; reconcile before retrying an uncertain submission"
    )]
    Tool { code: Option<i32> },
    #[error("invalid PrismaBuild tool response: {0}")]
    ToolResponse(String),
    #[error("PrismaBuild bell unavailable: {0}")]
    Bell(String),
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ActionKey(String);

impl ActionKey {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl FromStr for ActionKey {
    type Err = Error;
    fn from_str(value: &str) -> Result<Self, Error> {
        if value.len() != 64
            || !value
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(Error::InvalidInput(
                "action key must be 64 lowercase hex characters".into(),
            ));
        }
        Ok(Self(value.into()))
    }
}

impl TryFrom<String> for ActionKey {
    type Error = Error;
    fn try_from(value: String) -> Result<Self, Error> {
        value.parse()
    }
}

impl From<ActionKey> for String {
    fn from(key: ActionKey) -> Self {
        key.0
    }
}

impl std::fmt::Display for ActionKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

/// Paths and endpoint are explicit so tests cannot accidentally use the fleet.
#[derive(Clone, Debug)]
pub struct Options {
    pub queue_root: PathBuf,
    pub tool: Tool,
    /// Numeric HTTP endpoint of the queue's bell, for example http://127.0.0.1:9470/.
    pub bell_url: String,
}

#[derive(Clone)]
pub struct PrismaBuild {
    options: Options,
    http: reqwest::Client,
    bell_url: reqwest::Url,
}

impl PrismaBuild {
    pub fn new(options: Options) -> Result<Self, Error> {
        if !options.queue_root.is_absolute() || options.queue_root.to_str().is_none() {
            return Err(Error::InvalidInput(
                "queue root must be an absolute UTF-8 path".into(),
            ));
        }
        let bell_url = reqwest::Url::parse(&options.bell_url)
            .map_err(|e| Error::InvalidInput(e.to_string()))?;
        if bell_url.scheme() != "http"
            || bell_url
                .host_str()
                .is_none_or(|h| h.parse::<std::net::IpAddr>().is_err())
            || !bell_url.username().is_empty()
            || bell_url.password().is_some()
            || bell_url.query().is_some()
            || bell_url.fragment().is_some()
            || bell_url.path() != "/"
        {
            return Err(Error::InvalidInput(
                "bell must be an HTTP origin with a numeric host".into(),
            ));
        }
        let http = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(std::time::Duration::from_secs(1))
            .build()
            .map_err(|e| Error::Bell(e.to_string()))?;
        Ok(Self {
            options,
            http,
            bell_url,
        })
    }
}
