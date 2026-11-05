use crate::contracts::bounded::{Nullable, Text, UNBOUNDED};
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
    #[schemars(length(min = 1))]
    pub id: String,
    #[schemars(with = "Nullable<String>")]
    pub source_comment: Option<String>,
    #[schemars(with = "std::collections::BTreeMap<String, Value>")]
    pub question: Value,
    #[schemars(length(min = 1))]
    pub answer: String,
    #[schemars(with = "Vec<Text<1, UNBOUNDED>>")]
    pub allowed_scope: Vec<String>,
    #[schemars(with = "Vec<Text<1, UNBOUNDED>>")]
    pub restrictions: Vec<String>,
    pub origin_request_version: u64,
    #[schemars(with = "Nullable<String>")]
    pub superseded_by: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "String", length(min = 1))]
    pub supersession_reason: Option<String>,
}
