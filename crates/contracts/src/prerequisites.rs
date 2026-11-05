//! Q59 / #839: closure evidence retained on the dependent, independent of admission.
use schemars::{JsonSchema, generate::SchemaSettings};
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

/// The planner's remaining fields predate Rust-sourced contracts. Generate this
/// field from the same type as postmortem rather than hand-writing a wire mirror.
pub fn planner_schema() -> crate::Generated {
    let mut schema: serde_json::Value = serde_json::from_str(include_str!(
        "../../../workers/src/tl3_workers/schemas/planner-input.json"
    ))
    .expect("planner schema");
    let mut generator = SchemaSettings::draft2020_12()
        .with(|s| s.inline_subschemas = true)
        .into_generator();
    schema["properties"]["prerequisite_closures"] =
        <Vec<Closure>>::json_schema(&mut generator).to_value();
    crate::Generated {
        path: format!("{}/planner-input.json", crate::SCHEMA_DIRECTORY),
        text: crate::render(&schema),
    }
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
