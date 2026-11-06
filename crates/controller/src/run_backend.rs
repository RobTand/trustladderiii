//! Shared execution transport. Daytime base/head judging remains with the harness.
pub mod cli;
pub mod schema;
pub mod prepared;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::num::NonZeroU32;
use std::path::PathBuf;
pub use tl3_config::ContainerMode;

pub mod durable;

pub type Result<T> = std::result::Result<T, BackendError>;

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, thiserror::Error)]
#[error("{code:?}: {message}")]
#[serde(deny_unknown_fields)]
pub struct BackendError {
    pub code: ErrorCode,
    pub message: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode { InvalidRequest, Unavailable, AcceptanceUnknown, EvidenceInvalid, Conflict }

impl BackendError {
    pub fn invalid(message: &str) -> Self {
        Self { code: ErrorCode::InvalidRequest, message: message.into() }
    }
    pub fn unknown(message: &str) -> Self {
        Self { code: ErrorCode::AcceptanceUnknown, message: message.into() }
    }
}

impl From<std::io::Error> for BackendError {
    fn from(_: std::io::Error) -> Self {
        Self { code: ErrorCode::Unavailable, message: "backend I/O unavailable; retain intent and reattach".into() }
    }
}
impl From<serde_json::Error> for BackendError {
    fn from(_: serde_json::Error) -> Self {
        Self { code: ErrorCode::EvidenceInvalid, message: "invalid or incompatible structured evidence".into() }
    }
}

pub fn token(value: &str) -> bool {
    !value.is_empty() && value.len() <= 128
        && value.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
}
pub fn hex(value: &str, length: usize) -> bool {
    value.len() == length && value.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
pub fn generation(value: &str) -> bool {
    token(value) || value.split_once('/').is_some_and(|(run, invocation)| token(run) && hex(invocation, 32))
}
pub fn f58(value: &str) -> bool {
    let rest = value.strip_prefix('f').or_else(|| value.strip_prefix('ƒ'));
    rest.is_some_and(|s| !s.is_empty() && s.len() <= 16
        && s.bytes().all(|b| b"123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz".contains(&b)))
}
pub fn absolute(path: &std::path::Path) -> bool {
    path.is_absolute() && path.to_str().is_some_and(|s| s.len() <= 4096 && !s.chars().any(char::is_control))
        && path.components().all(|c| matches!(c, std::path::Component::RootDir | std::path::Component::Normal(_)))
}

impl BackendId {
    pub fn validate(&self) -> Result<()> {
        if !generation(&self.instance) || !self.uri.starts_with("local:///") || self.uri.len() > 4096 || self.uri.chars().any(char::is_control) || !f58(&self.job) {
            return Err(BackendError::invalid("expected Flux instance generation and f58 job ID; PB keys are unsupported"));
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
    pub job: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BackendTag { Flux }

/// Host-runner plumbing, not scheduler attestation or own-cgroup qualification.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct LaunchContext {
    pub schema: LaunchSchema,
    pub run: BackendId,
    pub owner: String,
    pub correlation: String,
    pub request_digest: String,
    pub scratch: OwnedScratch,
    pub output_dir: PathBuf,
    pub progress: ProgressPolicy,
    pub native_threads: NonZeroU32,
    pub runtime: ExecutionRuntime,
    pub runtime_identity: RuntimeIdentity,
    pub placement: LaunchPlacement,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum LaunchPlacement {
    Observed { allocation: BlobRef, topology: BlobRef, host: String, rank: u32, os_pus: Vec<u32> },
    Unqualified { reason: String },
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
pub enum LaunchSchema {
    #[serde(rename = "tl3.launch_context.v1")]
    V1,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct OwnedScratch {
    pub approved_root: PathBuf,
    pub leaf: PathBuf,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ProgressPolicy {
    pub journal: PathBuf,
    pub no_progress_seconds: Option<NonZeroU32>,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ExecutionRuntime {
    Native { interpreter: PathBuf },
    RootlessPodman {
        mode: ContainerMode,
        executable: PathBuf,
        image: String,
        trusted_root: PathBuf,
        interpreter: PathBuf,
        memory_limit_bytes: Option<std::num::NonZeroU64>,
    },
}


#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TreeIdentity {
    pub head: String,
    pub tree: String,
    pub input_digest: String,
    pub root: PathBuf,
    pub manifest: PathBuf,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RunnerIdentity {
    pub executable: PathBuf,
    pub executable_digest: String,
    pub arguments: Vec<String>,
    pub manifest: PathBuf,
    pub manifest_digest: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RuntimeIdentity {
    pub manifest: PathBuf,
    pub manifest_digest: String,
    pub runtime: ExecutionRuntime,
    pub architecture: String,
    pub expected_image: Option<String>,
    pub preflight: BlobRef,
    pub host_dependency: Option<HostDependency>,
    pub image: Option<ImageIdentity>,
    pub cleanup: Option<CleanupProgram>,
}
impl RuntimeIdentity {
    pub fn placement(&self) -> Placement {
        Placement { architecture: self.architecture.clone(), host_dependency: self.host_dependency.clone() }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Placement {
    pub architecture: String,
    pub host_dependency: Option<HostDependency>,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct HostDependency { pub host: String, pub reason: String }

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RuntimePreflight {
    pub schema: String,
    pub architecture: String,
    pub image_identity: Option<String>,
    pub image_present: bool,
    pub identity_verified: bool,
    pub image: Option<ImageIdentity>,
    pub mode: Option<ContainerMode>,
    pub no_cgroups_supported: bool,
    pub init_available: bool,
    pub user_scope_supported: bool,
}

/// Prepared by the runtime builder from the ops inventory, never a compiled digest catalog.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ImageIdentity {
    pub inventory: BlobRef,
    pub profile: String,
    pub reference: String,
    pub index_digest: Option<String>,
    pub manifest_digest: String,
    pub config_digest: String,
    pub platform: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CleanupProgram {
    pub executable: PathBuf,
    pub executable_digest: String,
    pub arguments: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Reservation {
    pub cpus: NonZeroU32,
    pub gpus: u32,
    pub memory_bytes: std::num::NonZeroU64,
    pub memory_guarantee: MemoryGuarantee,
    pub parallelism: NonZeroU32,
    pub native_threads: NonZeroU32,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MemoryGuarantee { DeclaredOnly, RequireAdmission }

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Suite {
    Full { membership_digest: String },
    SelectedCases { membership_digest: String },
    WorkerTests { input_digest: String },
    Generator { input_digest: String },
    DaytimeChecks { plan_digest: String, base: String, head: String, reviewed: Option<String> },
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub repo: String,
    pub tree: TreeIdentity,
    pub suite: Suite,
    pub reservation: Reservation,
    pub runner: RunnerIdentity,
    pub environment: std::collections::BTreeMap<String, String>,
    pub runtime: RuntimeIdentity,
    pub scratch_root: PathBuf,
    pub no_progress_seconds: Option<NonZeroU32>,
    pub walltime_seconds: NonZeroU32,
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

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Owner {
    pub owner: String,
    pub correlation: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Ending { Passed, Failed, Refused, InfraLost, Cancelled }

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum WaitStatus {
    Exited { raw: u32, code: u8 },
    Signalled { raw: u32, signal: u8, core_dumped: bool },
    Other { raw: u32 },
}

pub fn decode_wait_status(raw: u32) -> WaitStatus {
    if raw & 0x7f == 0 && raw <= 0xffff {
        WaitStatus::Exited { raw, code: ((raw >> 8) & 0xff) as u8 }
    } else if raw & 0x7f != 0x7f && raw <= 0xff {
        WaitStatus::Signalled { raw, signal: (raw & 0x7f) as u8, core_dumped: raw & 0x80 != 0 }
    } else {
        WaitStatus::Other { raw }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum NativeFact {
    Start,
    Finish { status: WaitStatus },
    Exception { exception_type: String, severity: u32, note: String },
    Clean,
    Other { name: String },
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Event {
    pub sequence: u64,
    pub digest: String,
    pub fact: NativeFact,
    pub progress: NativeProgress,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct BlobRef {
    pub sha256: String,
    pub bytes: u64,
    pub path: PathBuf,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Receipt {
    pub schema: String,
    pub issuer: ReceiptIssuer,
    pub request: BlobRef,
    pub intent: SubmissionIntent,
    pub binding: Option<RunBinding>,
    pub tree: TreeIdentity,
    pub runner: RunnerIdentity,
    pub runtime: RuntimeIdentity,
    pub reservation: Reservation,
    pub ending: Ending,
    pub reason: String,
    pub native_events: Vec<Event>,
    pub evidence: Vec<BlobRef>,
    pub report: Option<BlobRef>,
    pub runtime_ownerships: Vec<RuntimeOwnership>,
    pub cleanup: std::collections::BTreeMap<String, BlobRef>,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReceiptIssuer { Tl3 }

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Terminal {
    pub ending: Ending,
    pub reason: String,
    pub receipt: BlobRef,
}

/// Trusted host-runner envelope. Daytime payload is judged by the existing harness.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RunnerReport {
    pub schema: String,
    pub run: BackendId,
    pub request_digest: String,
    pub tree: TreeIdentity,
    pub runner_digest: String,
    pub runtime_digest: String,
    pub suite: Suite,
    pub complete: bool,
    pub cleanup_complete: bool,
    pub tests_started: u64,
    pub outcome: ReportOutcome,
    pub cases: Vec<CaseFact>,
    pub payload: Option<BlobRef>,
    pub runtime_binding: Option<RuntimeBindingInput>,
    #[serde(default)]
    pub runtime_bindings: Vec<RuntimeBindingInput>,
    pub cleanup: Option<BlobRef>,
    pub stdout: Option<BlobRef>,
    pub worker_payload: Option<WorkerPayload>,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct InputManifest {
    pub schema: String,
    pub head: String,
    pub tree: String,
    pub files: Vec<InputFile>,
}
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct InputFile { pub path: PathBuf, pub sha256: String }

#[derive(Clone, Copy, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReportOutcome { Passed, TestsFailed, ChecksCompleted, WorkerPayloadCompleted, RefusedBeforeTests, EnvironmentLost }

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CaseFact {
    pub target: String,
    pub name: String,
    pub outcome: CaseOutcome,
    pub duration_ms: u64,
    pub disposition: Option<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CaseOutcome { Passed, Failed, Error, Skipped, Xfailed, Xpassed }

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Membership {
    pub schema: String,
    pub cases: Vec<CaseIdentity>,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq, PartialOrd, Ord)]
#[serde(deny_unknown_fields)]
pub struct CaseIdentity { pub target: String, pub name: String }

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Submission {
    Accepted { binding: RunBinding },
    Unknown { reason: String },
    Refused { terminal: Terminal },
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Observation {
    Terminal { binding: Option<RunBinding>, terminal: Terminal },
    Unknown { binding: Option<RunBinding>, reason: String },
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Availability { Available, Busy, Incompatible, Unknown }

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Probe {
    pub availability: Availability,
    pub reason: String,
    pub total_cpus: u64,
    pub free_cpus: u64,
    pub total_gpus: u64,
    pub free_gpus: u64,
    pub memory_guarantee: MemoryGuarantee,
}

/// Implementations own dispatch/recovery, event interpretation and evidence retention.
pub trait RunBackend {
    fn submit(&self, request: Request, intent: SubmissionIntent) -> Result<Submission>;
    fn attach(&self, owner: &Owner, stop: &WatchStop, emit: &mut dyn FnMut(&Event) -> Result<()>) -> Result<Observation>;
    fn cancel(&self, owner: &Owner) -> Result<CancelAck>;
    fn probe(&self, reservation: &Reservation, placement: &Placement) -> Result<Probe>;
    fn liveness(&self, owner: &Owner) -> Result<Liveness>;
    fn launch_context(&self, owner: &Owner, job: &str) -> Result<LaunchContext>;
    fn retain_runtime(&self, owner: &Owner, ownership: RuntimeOwnership) -> Result<()>;
    fn validate_allocation(&self, owner: &Owner, facts: &AllocationInput) -> Result<CpuGrant>;
    fn cleanup_runtime(&self, owner: &Owner) -> Result<CleanupResult>;
    fn worker_payload(&self, owner: &Owner, terminal: &Terminal) -> Result<WorkerPayload>;
    fn daytime_payload(&self, owner: &Owner, terminal: &Terminal) -> Result<crate::contracts::report::CheckReport>;
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CancelAck {
    pub binding: Option<RunBinding>,
    pub acknowledged: bool,
    pub terminal: Option<Terminal>,
}

/// Provisioned access only. URI is deliberately separate from instance generation.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Options {
    pub executable: PathBuf,
    pub arguments: Vec<String>,
    pub environment: std::collections::BTreeMap<String, String>,
    pub instance_generation: String,
    pub uri: String,
    pub journal_root: PathBuf,
    pub bootstrap: HostBootstrap,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HostBootstrap {
    pub cli: PathBuf,
    pub config: PathBuf,
}

/// Stops and reaps local observers through the caller's attach thread; never cancels native execution.
#[derive(Clone, Default)]
pub struct WatchStop {
    inner: std::sync::Arc<std::sync::Mutex<WatchState>>,
}

#[derive(Default)]
struct WatchState { stopped: bool, groups: std::collections::BTreeSet<u32> }

impl WatchStop {
    pub fn stop(&self) {
        let mut state = self.inner.lock().expect("watch stop mutex");
        state.stopped = true;
        for group in &state.groups { kill_group(*group); }
    }
    pub fn is_stopped(&self) -> bool { self.inner.lock().expect("watch stop mutex").stopped }
    pub(crate) fn register(&self, group: u32) {
        let mut state = self.inner.lock().expect("watch stop mutex");
        state.groups.insert(group);
        if state.stopped { kill_group(group); }
    }
    pub(crate) fn unregister(&self, group: u32) {
        self.inner.lock().expect("watch stop mutex").groups.remove(&group);
    }
}

pub(crate) fn kill_group(group: u32) {
    if let Ok(group) = i32::try_from(group) && let Some(group) = rustix::process::Pid::from_raw(group) {
        let _ = rustix::process::kill_process_group(group, rustix::process::Signal::KILL);
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ProcessPlacement {
    pub rank: u32,
    pub host: String,
    pub allowed_cpus: String,
    pub cgroup: String,
}

/// Before-container allocation facts. No container or cgroup qualification is inferred.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AllocationInput {
    pub run: BackendId,
    pub request_digest: String,
    pub allocation: BlobRef,
    pub topology: BlobRef,
    pub host: ProcessPlacement,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CpuGrant {
    pub run: BackendId,
    pub request_digest: String,
    pub host: String,
    pub rank: u32,
    pub os_pus: Vec<u32>,
    pub allocation: BlobRef,
    pub topology: BlobRef,
}

/// Raw facts collected outside the test runner's stdout. Rust interprets them.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RuntimeBindingInput {
    pub execution: String,
    pub container_id: Option<String>,
    pub run: BackendId,
    pub request_digest: String,
    pub allocation: BlobRef,
    pub topology: BlobRef,
    pub host: ProcessPlacement,
    pub container: Option<ProcessPlacement>,
    pub container_scope: Option<ContainerScopeFacts>,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ContainerScopeFacts {
    pub unit: String,
    pub invocation_id: String,
    pub cgroup: String,
    pub cpuset_effective: String,
    pub memory_max: Option<u64>,
    pub memory_swap_max: Option<u64>,
    pub container_id: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TopologyMapping {
    pub run: BackendId,
    pub request_digest: String,
    pub ranks: Vec<RankMapping>,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RankMapping {
    pub rank: u32,
    pub host: String,
    pub cores: Vec<CoreMapping>,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CoreMapping {
    pub flux_core: u32,
    pub os_pus: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RuntimeQualification {
    pub qualified: bool,
    pub reason: String,
    pub evidence: Vec<BlobRef>,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CollectorInput {
    pub run: BackendId,
    pub stdout: BlobRef,
    pub runtime_binding: Option<RuntimeBindingInput>,
    #[serde(default)]
    pub runtime_bindings: Vec<RuntimeBindingInput>,
    pub cleanup: Option<BlobRef>,
}

/// Retained before starting repository code. Immutable across cleanup retries.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RuntimeOwnership {
    pub execution: String,
    pub run: BackendId,
    pub owner: Owner,
    pub request_digest: String,
    pub host: String,
    pub rank: u32,
    pub launcher: ProcessIdentity,
    pub process_group: u32,
    pub container: Option<ContainerOwnership>,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ProcessIdentity { pub pid: u32, pub start_ticks: u64 }

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ContainerOwnership {
    pub mode: ContainerMode,
    pub id: String,
    pub labels: std::collections::BTreeMap<String, String>,
    pub scope: Option<ScopeIdentity>,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ScopeIdentity { pub unit: String, pub invocation_id: String, pub cgroup: String }

/// One bounded JSON stdin request to the trusted independent runtime cleanup entry.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CleanupInput { pub schema: String, pub ownership: RuntimeOwnership }

/// Raw observations from the trusted host helper, interpreted only by Rust.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CleanupEvidence {
    pub schema: String,
    pub ownership: RuntimeOwnership,
    pub launcher: ProcessStop,
    /// kill(-owned_pgid, 0) must return ESRCH after reap/removal.
    pub process_group_probe_errno: Option<i32>,
    pub container: Option<ContainerStop>,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ProcessStop {
    Absent,
    Reaped { status: i32 },
    Reused { observed_start_ticks: u64 },
    Present { observed_start_ticks: u64 },
    Unknown,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ContainerStop {
    pub id: String,
    pub remove_exit: i32,
    /// Native podman container exists exact-ID status: 1 absent, 125 error.
    pub exists_exit: i32,
    pub scope: Option<ScopeStop>,
    /// Label-filtered sweep may only account for the retained exact container.
    pub sweep_ids: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ScopeStop {
    pub identity: ScopeIdentity,
    pub observed_invocation_id: Option<String>,
    /// None means the retained cgroup path is authoritatively absent.
    pub cgroup_events: Option<String>,
    pub cgroup_procs: String,
    /// systemd inactive/dead or unit-not-found, never an arbitrary boolean.
    pub active_state: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CleanupResult {
    pub stopped: bool,
    pub evidence: Vec<BlobRef>,
    pub pending_executions: Vec<String>,
    pub accepted_execution: Option<String>,
    pub reason: String,
}

/// Transport-only retained worker protocol. Existing worker semantic validators remain authoritative.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum WorkerPayload {
    Tests { summary: BlobRef, symptom_runs: Vec<BlobRef> },
    Generator { artifact: BlobRef },
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkerCollection { pub report: BlobRef, pub payload: WorkerPayload }

/// Existing generator bounds: decoded outputs 32 MiB, encoded artifact 48 MiB.
pub const GENERATOR_OUTPUT_BYTES: usize = 32 * 1024 * 1024;
pub const GENERATOR_ARTIFACT_BYTES: usize = 48 * 1024 * 1024;
pub const GENERATOR_STDOUT_BYTES: usize = GENERATOR_ARTIFACT_BYTES + 1024 * 1024;
pub const WORKER_TEST_STDOUT_BYTES: usize = 1024 * 1024;

/// Rust-owned input to immutable pre-submit bundle publication. FXR supplies raw files only.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PrepareBundleInput {
    pub source_root: PathBuf,
    pub code_inventory: PathBuf,
    pub collector: PathBuf,
    pub cleanup: PathBuf,
    pub trusted_root: PathBuf,
    pub producer_commit: String,
    pub container_interpreter: PathBuf,
    pub image_inventory: PathBuf,
    pub image_inspect: PathBuf,
    pub profile: String,
    pub mode: ContainerMode,
    pub prerequisites: ModePrerequisites,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ModePrerequisites { pub no_cgroups_supported: bool, pub init_available: bool, pub user_scope_supported: bool }

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CodeInventory { pub interpreter: PathBuf, pub files: Vec<CodeFile> }

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CodeFile { pub path: PathBuf, pub sha256: String, pub bytes: u64 }

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct BundleFile { pub path: PathBuf, pub sha256: String, pub bytes: u64, pub mode: u32 }

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RuntimeBundleManifest {
    pub schema: String,
    pub owner: String,
    pub uid: u32,
    pub producer_commit: String,
    pub interpreter: PathBuf,
    pub podman: PathBuf,
    pub container_interpreter: PathBuf,
    pub collector: PathBuf,
    pub cleanup: PathBuf,
    pub trusted_root: PathBuf,
    pub files: Vec<BundleFile>,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PreparedRuntime { pub bundle: BlobRef, pub runner: RunnerIdentity, pub runtime: RuntimeIdentity, pub scratch_root: PathBuf }

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct EventCursor { pub sequence: u64, pub digest: String }

#[derive(Clone, Copy, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum NativePhase { Queued, Running, Finishing, Terminal, Unknown }

#[derive(Clone, Copy, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProgressAuthority { Historical, FreshNative, CommittedTerminal, Unknown }

/// Persisted before the event callback. Historical progress never proves current liveness.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct NativeProgress {
    pub owner: Owner,
    pub run: BackendId,
    pub cursor: EventCursor,
    pub phase: NativePhase,
    pub authority: ProgressAuthority,
}

/// One bounded on-demand observation at the existing scheduled Look boundary, never a poller.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Liveness {
    pub owner: Owner,
    pub run: Option<BackendId>,
    pub cursor: Option<EventCursor>,
    pub phase: NativePhase,
    pub authority: ProgressAuthority,
    pub observed_unix_ms: u64,
    pub terminal: Option<Terminal>,
    pub evidence: Option<BlobRef>,
    pub reason: String,
}
