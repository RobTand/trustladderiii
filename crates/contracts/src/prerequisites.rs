//! Q59 / #839: closure evidence retained on the dependent, independent of admission.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Issue,
    PullRequest,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Closure {
    pub kind: Kind,
    #[schemars(range(min = 1))]
    pub number: u64,
    pub reason: String,
}

impl crate::render::SchemaOrder for Closure {
    fn order() -> crate::render::TypeOrder<'static> {
        crate::render::TypeOrder::of::<Self>(&[
            (
                "",
                &["type", "properties", "required", "additionalProperties"],
            ),
            ("/properties", &["kind", "number", "reason"]),
            ("/properties/kind", &["type", "enum"]),
            ("/properties/number", &["type", "format", "minimum"]),
            ("/properties/reason", &["type"]),
        ])
    }
}
