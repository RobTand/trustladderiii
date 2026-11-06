//! Shared execution transport. Daytime base/head judging remains with the harness.
pub mod cli;
pub mod schema;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::num::NonZeroU32;
use std::path::PathBuf;

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct BackendId {
    pub backend: BackendTag,
    pub instance: String,
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
        executable: PathBuf,
        image: String,
        trusted_root: PathBuf,
        interpreter: PathBuf,
    },
}
