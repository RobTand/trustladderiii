//! Private pytest subprocess facts. These report Python API observations, never verdicts.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tl3_harness_core::{phase::{Phase, Status}, summary::PythonText};
use tl3_contracts::checks::Patch;

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FrameFact {
    pub file: PythonText,
    pub function: PythonText,
    pub line: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExceptionFact {
    pub qualified_type: PythonText,
    pub message: PythonText,
    pub frames: Vec<FrameFact>,
    pub cause: Option<Box<ExceptionFact>>,
    pub context: Option<Box<ExceptionFact>>,
    pub suppress_context: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "fact", rename_all = "snake_case", deny_unknown_fields)]
pub enum PluginFact {
    Collected { nodes: Vec<PythonText> },
    Collection { node: PythonText, status: Status, exception: Option<ExceptionFact> },
    Phase { node: PythonText, phase: Phase, status: Status, exception: Option<ExceptionFact> },
    ForcingTarget { request_id: u64, patch: Patch, owner_module: PythonText, owner_file: PythonText, callable: bool },
    Delay { request_id: u64, patch_index: usize, requested_micros: u64, elapsed_micros: u64, hits: u64 },
    SourceRead { request_id: u64, path: PythonText, resolved_path: PythonText, operation: SourceOperation, frames: Vec<FrameFact> },
    Finished { exit_code: i32, collected: usize },
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SourceOperation { Open, ReadText, ReadBytes, LoaderGetSource }
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "response", rename_all = "snake_case", deny_unknown_fields)]
pub enum PluginResponse {
    SourceRead { request_id: u64, decision: SourceDecision },
    Forcing { request_id: u64, decision: ForcingDecision },
    Delay { request_id: u64, authorized_micros: u64 },
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SourceDecision { Allow, RefuseSubjectSource }
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "decision", rename_all = "snake_case", deny_unknown_fields)]
pub enum ForcingDecision { Apply, Refuse { reason: String } }
