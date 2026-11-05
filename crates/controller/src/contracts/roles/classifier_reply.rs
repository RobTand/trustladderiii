use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ClassifierReply(serde_json::Value);

pub fn schema_text() -> String {
    "{}\n".into()
}
