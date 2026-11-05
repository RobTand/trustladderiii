use serde::{Deserialize,Serialize};
#[derive(Clone, Debug, Deserialize, Serialize, thiserror::Error)]
#[error("worker inputs break role schema: role={role} pointer={pointer} rule={rule}")]
pub struct Failure {
    role: String,
    pointer: String,
    rule: String,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct Context {
    #[serde(flatten)]
    diagnostic: Failure,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    maker_head: Option<String>,
}

impl Context {
    pub(crate) fn description(&self) -> String {
        let step = match self.diagnostic.role.as_str() {
            "reviewer" => "Review",
            "maker" => "Maker",
            "planner" => "Planning",
            "ingest" => "Request reading",
            "audit" => "Independent request reading",
            "classifier" => "Comment classification",
            "writer" => "Message writing",
            "postmortem" => "Postmortem",
            _ => "Worker",
        };
        let mut description = format!("{step} setup failed.");
        if self.maker_head.is_some() {
            description.push_str(" The maker's change is complete and preserved.");
        }
        description
    }
}
impl std::fmt::Display for Context {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} {}", self.description(), self.diagnostic)
    }
}
