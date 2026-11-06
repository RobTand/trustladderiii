//! #845: trusted controller inputs to the existing configured-generator executor.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GenerationRequest {
    pub checkout: PathBuf,
    pub state: PathBuf,
    pub command: String,
    pub generated_files: Vec<String>,
    pub files: Vec<String>,
    pub protected_paths: Vec<String>,
    pub image: String,
    pub tools_path: PathBuf,
    pub cpus: u32,
    pub mem_gb: u32,
    pub timeout_s: u32,
}
