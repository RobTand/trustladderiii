use super::*;
use schemars::{JsonSchema, schema_for};

pub fn launch_context() -> std::result::Result<String, serde_json::Error> {
    serde_json::to_string_pretty(&schema_for!(LaunchContext)).map(|text| text + "\n")
}

#[allow(dead_code)]
#[derive(JsonSchema)]
struct WireSchema {
    session_request: cli::SessionRequest,
    command: cli::Command,
    frame: cli::Frame,
    options: Options,
    launch_context: LaunchContext,
    report: RunnerReport,
    input_manifest: InputManifest,
    membership: Membership,
    topology: TopologyMapping,
    receipt: Receipt,
    runtime_preflight: RuntimePreflight,
    cleanup: CleanupEvidence,
    cleanup_input: CleanupInput,
    runtime_ownership: RuntimeOwnership,
}

pub fn cli() -> std::result::Result<String, serde_json::Error> {
    serde_json::to_string_pretty(&schema_for!(WireSchema)).map(|text| text + "\n")
}
