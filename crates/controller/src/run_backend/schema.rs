use schemars::schema_for;
use super::LaunchContext;

pub fn launch_context() -> Result<String, serde_json::Error> {
    serde_json::to_string_pretty(&schema_for!(LaunchContext)).map(|text| text + "\n")
}
