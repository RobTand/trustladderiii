//! #832: worker-owned provenance, carried beside the model reply in its event.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct PlanCheckProvenance {
    pub objections: Vec<PlanObjection>,
    pub repair: PlanRepair,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct PlanObjection {
    pub class: ObjectionClass,
    pub rule: String,
    pub items: Vec<String>,
    pub diagnostic: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ObjectionClass {
    SizeExecutionFit,
    Other,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case", tag = "status", deny_unknown_fields)]
pub enum PlanRepair {
    Attempted,
    Unavailable { reason: RepairUnavailable },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RepairUnavailable {
    Disabled,
    InputAllowance,
    MessageLimit,
    FormatAllowanceConsumed,
}
