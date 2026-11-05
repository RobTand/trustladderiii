//! Planner reply migration (#820). Missing proof declarations remain distinct from empties.
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PlannerReply(pub Value);

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Proof(pub Value);

impl Proof {
    pub fn complete(&self) -> Option<super::super::checks::ProofDeclaration> {
        None
    }
}

pub fn schema_text() -> String {
    "{}\n".into()
}
