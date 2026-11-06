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
    status_tool: Tool,
    http: reqwest::Client,
    bell_url: reqwest::Url,
}

impl PrismaBuild {
    /// Override the read-only status executable, for recorded upstream fixtures.
    pub fn with_status_tool(mut self, tool: Tool) -> Self {
        self.status_tool = tool;
        self
    }

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
            status_tool: Tool {
                program: "python3".into(),
                args: vec!["/mnt/shared/prismabuild-fleet/repo/tools/pbstatus.py".into()],
            },
            options,
            http,
            bell_url,
        })
    }
}

// pbstatus is the PB-owned read boundary. Never retain its jobs, argv or tokens.
#[derive(Deserialize)]
struct FleetStatus {
    schema: String,
    transport: String,
    complete: bool,
    pool: PoolStatus,
    queue_space: QueueSpace,
    nodes: Vec<WorkerStatus>,
}
#[derive(Deserialize)]
struct PoolStatus {
    complete: bool,
}
#[derive(Deserialize)]
struct QueueSpace {
    state: String,
}
#[derive(Deserialize)]
struct WorkerStatus {
    healthy: bool,
    state: String,
    capacity: Option<Capacity>,
}
#[derive(Deserialize)]
struct Capacity {
    cpu: u32,
    mem_gb: u32,
}

impl PrismaBuild {
    /// Q7 / #850: one read-only fleet census, independent of any old action's verdict.
    /// A fresh, admitting CPU worker must have capacity for this delivery.
    /// Busy capacity is allowed: this tests service recovery, not immediate placement.
    pub async fn can_serve(&self, demand: &Demand) -> Result<bool, Error> {
        use std::{process::Stdio, time::Duration};
        use tokio::{io::AsyncReadExt, process::Command};
        if demand.gpu {
            return Ok(false);
        }
        let probe = async {
            let mut child = Command::new(&self.status_tool.program)
                .args(&self.status_tool.args)
                .args([
                    "--json",
                    "--transport",
                    "pool",
                    "--recent",
                    "0",
                    "--timeout-s",
                    "2",
                    "--queue-root",
                ])
                .arg(&self.options.queue_root)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .kill_on_drop(true)
                .spawn()
                .map_err(|source| Error::Io {
                    path: self.status_tool.program.clone(),
                    source,
                })?;
            let mut bytes = Vec::new();
            child
                .stdout
                .take()
                .expect("piped status stdout")
                .take(1024 * 1024 + 1)
                .read_to_end(&mut bytes)
                .await
                .map_err(|source| Error::Io {
                    path: self.status_tool.program.clone(),
                    source,
                })?;
            if bytes.len() > 1024 * 1024 {
                return Ok(false);
            }
            let status = child.wait().await.map_err(|source| Error::Io {
                path: self.status_tool.program.clone(),
                source,
            })?;
            if !status.success() {
                return Ok(false);
            }
            let fleet: FleetStatus = serde_json::from_slice(&bytes)
                .map_err(|_| Error::ToolResponse("invalid fleet status".into()))?;
            Ok(fleet.schema == "prismabuild.pbstatus.v1"
                && fleet.transport == "pool"
                && fleet.complete
                && fleet.pool.complete
                && fleet.queue_space.state == "ok"
                && fleet.nodes.iter().any(|node| {
                    node.healthy
                        && node.state == "live"
                        && node.capacity.as_ref().is_some_and(|c| {
                            c.cpu >= demand.cpus.get() && c.mem_gb >= demand.mem_gb
                        })
                }))
        };
        tokio::time::timeout(Duration::from_secs(5), probe)
            .await
            .unwrap_or(Ok(false))
    }
}
