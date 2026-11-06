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
    /// Code-owned row result. Legacy provenance never opts into a re-plan.
    #[serde(default)]
    pub allowed_empty: bool,
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
    OwnerDecision,
    Infrastructure,
    Credit,
    Quota,
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
    FailedWork,
    InputAllowance,
    MessageLimit,
    FormatAllowanceConsumed,
}

impl PlanCheckProvenance {
    /// Q27 (2026-11-06): only a completed bounded correction with no possible
    /// declaration edit qualifies. Owner and resource decisions keep their route.
    pub fn empty_allowed_replan(&self) -> bool {
        self.repair == PlanRepair::Attempted
            && !self.objections.is_empty()
            && self.objections.iter().all(|objection| {
                objection.allowed_empty
                    && matches!(
                        objection.class,
                        ObjectionClass::SizeExecutionFit | ObjectionClass::Other
                    )
                    && objection.rule != "path.protected"
            })
    }
}
