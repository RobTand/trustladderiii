//! #826: bounded operator diagnostics, separate from the model-facing test summary.
use super::bounded::Text;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    SetupFailure,
    UnknownIncomplete,
}

/// A diagnostic read only from a verified, bounded PB action log.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Diagnostic {
    pub outcome: Outcome,
    pub phase: Text<1, 64>,
    pub termination_reason: Text<1, 64>,
    pub test_started: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub test: Option<Text<0, 2048>>,
    pub stderr_tail: Text<1, 1024>,
}
