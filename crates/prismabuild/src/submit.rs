use crate::{ActionKey, ActionState, Error, PrismaBuild};
use serde::Deserialize;
use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::OsString,
    num::NonZeroU32,
    path::PathBuf,
    time::Duration,
};
use tokio::process::Command;

#[derive(Clone, Debug)]
pub struct Tool {
    pub program: PathBuf,
    pub args: Vec<OsString>,
}

impl Tool {
    pub fn published() -> Self {
        Self {
            program: "python3".into(),
            args: vec!["/mnt/shared/prismabuild-fleet/repo/tools/pbrun.py".into()],
        }
    }
}

#[derive(Clone, Debug)]
pub struct Demand {
    pub cpus: NonZeroU32,
    pub mem_gb: u32,
    pub gpu: bool,
}

#[derive(Clone, Debug)]
pub struct ProgressPhase {
    pub name: String,
    pub stall_allowance: Duration,
}

#[derive(Clone, Debug)]
pub struct SubmitAction {
    pub checkout: PathBuf,
    pub command: Vec<OsString>,
    pub demand: Demand,
    pub container_image: String,
    pub progress: Vec<ProgressPhase>,
    pub progress_cycle: bool,
    pub env: BTreeMap<String, String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubmissionStatus {
    Submitted,
    Attached,
    CacheHit,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Submission {
    pub key: ActionKey,
    pub status: SubmissionStatus,
    pub published_unix: Option<f64>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Withdrawal {
    pub key: ActionKey,
    pub state_before: Option<String>,
    /// A cancellation decision, not confirmation that the execution scope is empty.
    pub withdrawn: bool,
}

#[derive(Deserialize)]
struct SubmissionWire {
    schema: String,
    action_key: ActionKey,
    transport: String,
    status: SubmissionStatus,
    published_unix: Option<f64>,
    publication: Option<Publication>,
}

#[derive(Deserialize)]
struct Publication {
    published_unix: Option<f64>,
}

#[derive(Deserialize)]
struct WithdrawalWire {
    schema: String,
    action_key: ActionKey,
    state_before: Option<String>,
    withdrawn: bool,
}

fn digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

impl SubmitAction {
    fn validate(&self) -> Result<(), Error> {
        let image = self
            .container_image
            .strip_prefix("sha256:")
            .or_else(|| self.container_image.strip_prefix("content:sha256:"))
            .or_else(|| {
                self.container_image
                    .split_once("@sha256:")
                    .filter(|(repo, _)| !repo.is_empty())
                    .map(|(_, hash)| hash)
            });
        if !self.checkout.is_absolute()
            || self.command.first().is_none_or(|c| c.is_empty())
            || self.demand.mem_gb == 0
            || image.is_none_or(|hash| !digest(hash))
            || self.progress.is_empty()
        {
            return Err(Error::InvalidInput("submission needs an absolute checkout, command, positive memory demand, pinned image and progress phases".into()));
        }
        let mut names = BTreeSet::new();
        for phase in &self.progress {
            if phase.name.is_empty()
                || !phase
                    .name
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
                || phase.stall_allowance.is_zero()
                || !names.insert(&phase.name)
            {
                return Err(Error::InvalidInput(
                    "progress phases need unique names and positive stall allowances".into(),
                ));
            }
        }
        if self.env.keys().any(|name| {
            name.is_empty()
                || name.contains(['=', '\0'])
                || name.starts_with("PRISMABUILD_ACTION_PROGRESS_")
        }) {
            return Err(Error::InvalidInput(
                "invalid environment variable name".into(),
            ));
        }
        Ok(())
    }
}

impl PrismaBuild {
    pub(crate) fn tool(&self) -> Command {
        let mut command = Command::new(&self.options.tool.program);
        command.args(&self.options.tool.args).kill_on_drop(true);
        let parent = rustix::process::getpid();
        // Linux ties PDEATHSIG to the spawning thread. Callers must keep that thread
        // alive until this tool ends; the controller uses its owner thread, never spawn_blocking.
        // SAFETY: the child runs only async-signal-safe prctl/getppid calls before exec.
        unsafe {
            command.pre_exec(move || {
                rustix::process::set_parent_process_death_signal(Some(
                    rustix::process::Signal::KILL,
                ))?;
                if rustix::process::getppid() != Some(parent) {
                    return Err(std::io::Error::from(std::io::ErrorKind::Interrupted));
                }
                Ok(())
            });
        }
        command
    }

    pub async fn submit(&self, action: &SubmitAction) -> Result<Submission, Error> {
        self.submission(self.action_command(action, "--detach", 0)?)
            .await
    }

    fn action_command(
        &self,
        action: &SubmitAction,
        mode: &str,
        priority: i32,
    ) -> Result<Command, Error> {
        action.validate()?;
        let mut command = self.tool();
        command
            .arg(mode)
            .args([
                "--transport",
                "pool",
                "--anywhere",
                "--max-attempts",
                "1",
                "--cwd",
            ])
            .arg(&action.checkout)
            .arg("--cpus")
            .arg(action.demand.cpus.to_string())
            .arg("--demand")
            .arg(format!(
                "mem_gb={},gpu={}",
                action.demand.mem_gb,
                u8::from(action.demand.gpu)
            ))
            .arg("--container-image")
            .arg(&action.container_image);
        for phase in &action.progress {
            command.arg("--progress-phase").arg(format!(
                "{}={}",
                phase.name,
                phase.stall_allowance.as_secs_f64()
            ));
        }
        if action.progress_cycle {
            command.arg("--progress-cycle");
        }
        // Every process is bounded even when a caller omits its native-library settings.
        let mut env = BTreeMap::from([
            ("OMP_NUM_THREADS".to_owned(), "1".to_owned()),
            ("MKL_NUM_THREADS".to_owned(), "1".to_owned()),
            ("OPENBLAS_NUM_THREADS".to_owned(), "1".to_owned()),
        ]);
        env.extend(action.env.clone());
        for (name, value) in env {
            command.arg("--env").arg(format!("{name}={value}"));
        }
        command.arg("--priority").arg(priority.to_string());
        command.arg("--").args(&action.command);
        Ok(command)
    }

    async fn submission(&self, mut command: Command) -> Result<Submission, Error> {
        let output = command.output().await.map_err(|source| Error::Io {
            path: self.options.tool.program.clone(),
            source,
        })?;
        if !output.status.success() {
            return Err(Error::Tool {
                code: output.status.code(),
            });
        }
        let wire: SubmissionWire = serde_json::from_slice(&output.stdout)
            .map_err(|e| Error::ToolResponse(e.to_string()))?;
        if !matches!(
            wire.schema.as_str(),
            "prismabuild.submission.v1" | "prismaquant.prismabuild.pbrun_detach.v1"
        ) || wire.transport != "pool"
        {
            return Err(Error::ToolResponse(
                "unsupported submission schema or transport".into(),
            ));
        }
        let published_unix = wire
            .publication
            .map(|p| p.published_unix)
            .unwrap_or(wire.published_unix);
        if published_unix.is_some_and(|v| !v.is_finite() || v < 0.0)
            || (wire.status != SubmissionStatus::CacheHit && published_unix.is_none())
        {
            return Err(Error::ToolResponse(
                "submission is missing a valid publication".into(),
            ));
        }
        Ok(Submission {
            key: wire.action_key,
            status: wire.status,
            published_unix,
        })
    }

    /// Seal inputs into CAS without publishing executable work. Persist the key before publish.
    pub async fn seal(&self, action: &SubmitAction) -> Result<ActionKey, Error> {
        self.seal_at_priority(action, 0).await
    }

    pub async fn seal_at_priority(
        &self,
        action: &SubmitAction,
        priority: i32,
    ) -> Result<ActionKey, Error> {
        #[derive(Deserialize)]
        struct Sealed {
            schema: String,
            action_key: ActionKey,
            transport: String,
        }
        let mut command = self.action_command(action, "--seal-only", priority)?;
        let output = command.output().await.map_err(|source| Error::Io {
            path: self.options.tool.program.clone(),
            source,
        })?;
        if !output.status.success() {
            return Err(Error::Tool {
                code: output.status.code(),
            });
        }
        let wire: Sealed = serde_json::from_slice(&output.stdout)
            .map_err(|e| Error::ToolResponse(e.to_string()))?;
        if wire.schema != "prismabuild.sealed.v1" || wire.transport != "pool" {
            return Err(Error::ToolResponse("invalid sealed request".into()));
        }
        Ok(wire.action_key)
    }

    /// Publish the original CAS request even after the checkout or PB runtime changes.
    pub async fn publish(&self, key: &ActionKey) -> Result<Submission, Error> {
        let mut command = self.tool();
        command.args(["--publish-sealed", key.as_str(), "--detach"]);
        let submission = self.submission(command).await?;
        if &submission.key != key {
            return Err(Error::ToolResponse(
                "publication returned another action key".into(),
            ));
        }
        Ok(submission)
    }

    pub async fn withdraw(&self, key: &ActionKey, reason: &str) -> Result<Withdrawal, Error> {
        if let Some(confirmation) = self.withdrawal_confirmation(key).await? {
            return Ok(confirmation);
        }
        let output = self
            .tool()
            .args([
                "--transport",
                "pool",
                "--withdraw",
                key.as_str(),
                "--reason",
                reason,
                "--json",
            ])
            .output()
            .await
            .map_err(|source| Error::Io {
                path: self.options.tool.program.clone(),
                source,
            })?;
        // Exit 2 can mean absent or ambiguous. Only an authoritative reread confirms absence.
        if !output.status.success() {
            if output.status.code() == Some(2)
                && let Some(confirmation) = self.withdrawal_confirmation(key).await?
            {
                return Ok(confirmation);
            }
            return Err(Error::Tool {
                code: output.status.code(),
            });
        }
        let wire: WithdrawalWire = serde_json::from_slice(&output.stdout)
            .map_err(|e| Error::ToolResponse(e.to_string()))?;
        if wire.schema != "prismabuild.withdrawal.v1" || &wire.action_key != key {
            return Err(Error::ToolResponse(
                "withdrawal identity or schema mismatch".into(),
            ));
        }
        Ok(Withdrawal {
            key: wire.action_key,
            state_before: wire.state_before,
            withdrawn: wire.withdrawn,
        })
    }

    async fn withdrawal_confirmation(&self, key: &ActionKey) -> Result<Option<Withdrawal>, Error> {
        let state = match self.read(key).await?.map(|record| record.state) {
            None => None,
            Some(ActionState::Queued | ActionState::Claimed) => return Ok(None),
            Some(ActionState::Finished { .. }) => Some("finished"),
            Some(ActionState::Failed { .. }) => Some("failed"),
            Some(ActionState::LeaseLost) => Some("lease_lost"),
            Some(ActionState::Withdrawn) => Some("withdrawn"),
        };
        Ok(Some(Withdrawal {
            key: key.clone(),
            state_before: state.map(str::to_owned),
            withdrawn: state == Some("withdrawn"),
        }))
    }
}
