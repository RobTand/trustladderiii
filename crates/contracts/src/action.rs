//! Requests for the single Rust action. Execution arrives in later action slices.
//! The pytest edge is a subprocess; these contracts never embed an interpreter.

use crate::bounded::{CommitSha, Integer, List, Parallel, Text, UNBOUNDED, UnitPath};
use crate::rules::Role;
use crate::{CheckRequest, DeclaredSymptom, Forcing, Generated};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(
    tag = "action",
    content = "request",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum ActionRequest {
    Checks(CheckRequest),
    Tests(TestRequest),
    Generator(GeneratorRequest),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TestRequest {
    pub base: CommitSha,
    pub head: CommitSha,
    pub selectors: List<Text<1, UNBOUNDED>, 1, UNBOUNDED>,
    pub repeat: Integer<1, 10>,
    pub parallel: Parallel,
    pub role: Role,
    pub check_index: usize,
    pub forcing: Option<Forcing>,
    pub declared_symptom: Option<DeclaredSymptom>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GeneratorRequest {
    pub base: CommitSha,
    pub head: CommitSha,
    /// Already separated arguments for the configured repository generator.
    pub argv: List<Text<1, UNBOUNDED>, 1, UNBOUNDED>,
    pub outputs: List<UnitPath, 1, 128, true>,
    pub timeout_seconds: Integer<1, 600>,
}

pub fn generated() -> Vec<Generated> {
    let schema = schemars::generate::SchemaSettings::draft2020_12()
        .into_generator()
        .into_root_schema_for::<ActionRequest>()
        .to_value();
    vec![Generated {
        path: format!("{}/action-request.json", crate::SCHEMA_DIRECTORY),
        text: format!(
            "{}\n",
            serde_json::to_string_pretty(&schema).expect("schema serializes")
        ),
    }]
}
