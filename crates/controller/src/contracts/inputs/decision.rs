use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Context {
    #[schemars(extend("const" = 1))]
    pub schema_version: u32,
    pub request_version: u64,
    #[schemars(length(max = 64))]
    pub decisions: Vec<Decision>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Decision {
    pub id: String,
    #[schemars(required)]
    pub source_comment: Option<String>,
    #[schemars(with = "std::collections::BTreeMap<String, Value>")]
    pub question: Value,
    pub answer: String,
    pub allowed_scope: Vec<String>,
    pub restrictions: Vec<String>,
    pub origin_request_version: u64,
    #[schemars(required)]
    pub superseded_by: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supersession_reason: Option<String>,
}
