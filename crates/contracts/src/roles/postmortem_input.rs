//! Typed postmortem-input wire contract (#819).

use super::super::bounded::{
    AnyJson, DecimalText, JsonArray, JsonObject, Natural, RepositoryName, Text, UNBOUNDED, present,
};
use super::super::render::{self, Layout, SchemaOrder, TypeOrder, TypeUse};
use schemars::{JsonSchema, generate::SchemaSettings};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Record {
    pub id: Natural<1>,
    pub trigger: Text<1, UNBOUNDED>,
    pub reason: Text<1, UNBOUNDED>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Case {
    pub request: JsonObject,
    pub outcome: Text<1, UNBOUNDED>,
    pub grades: JsonArray,
    pub phases: JsonArray,
    pub questions: JsonArray,
    pub incidents: JsonArray,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "AnyJson")]
    pub plan: Option<serde_json::Value>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "AnyJson")]
    pub brief: Option<serde_json::Value>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "AnyJson")]
    pub audit: Option<serde_json::Value>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "AnyJson")]
    pub findings: Option<serde_json::Value>,
    pub release: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Rubric {
    pub version: Natural<1>,
    pub dimensions: JsonObject,
    #[serde(flatten)]
    #[schemars(skip)]
    extra: serde_json::Map<String, serde_json::Value>,
}

impl Rubric {
    pub fn new(version: Natural<1>, dimensions: JsonObject) -> Self {
        Self {
            version,
            dimensions,
            extra: serde_json::Map::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TraceTags {
    pub node: DecimalText,
    pub request: DecimalText,
    pub repo: RepositoryName,
    pub issue: DecimalText,
}

/// The case file the controller writes for one postmortem (#403). The worker redacts `case` before any judge reads it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PostmortemInput {
    pub record: Record,
    pub case: Case,
    pub rubric: Rubric,
    pub release: String,
    pub grade_model: Text<1, UNBOUNDED>,
    /// The run's whole wall-clock allowance (#486).
    pub deadline_secs: Natural<1>,
    pub trace_tags: TraceTags,
}

impl SchemaOrder for Record {
    fn order() -> TypeOrder<'static> {
        TypeOrder::of::<Self>(&[
            (
                "",
                &["type", "properties", "required", "additionalProperties"],
            ),
            ("/properties", &["id", "trigger", "reason"]),
        ])
    }
}

impl SchemaOrder for Case {
    fn order() -> TypeOrder<'static> {
        TypeOrder::of::<Self>(&[
            (
                "",
                &["type", "properties", "required", "additionalProperties"],
            ),
            (
                "/properties",
                &[
                    "request",
                    "outcome",
                    "grades",
                    "phases",
                    "questions",
                    "incidents",
                    "plan",
                    "brief",
                    "audit",
                    "findings",
                    "release",
                ],
            ),
        ])
    }
}

impl SchemaOrder for Rubric {
    fn order() -> TypeOrder<'static> {
        TypeOrder::of::<Self>(&[
            ("", &["type", "properties", "required"]),
            ("/properties", &["version", "dimensions"]),
        ])
    }
}

impl SchemaOrder for TraceTags {
    fn order() -> TypeOrder<'static> {
        TypeOrder::of::<Self>(&[
            (
                "",
                &["type", "properties", "required", "additionalProperties"],
            ),
            ("/properties", &["node", "request", "repo", "issue"]),
        ])
    }
}

pub fn schema() -> Value {
    let mut generator = SchemaSettings::draft2020_12()
        .with(|s| s.inline_subschemas = true)
        .into_generator();
    let mut schema = PostmortemInput::json_schema(&mut generator).to_value();
    schema["$schema"] = "https://json-schema.org/draft/2020-12/schema".into();
    schema["$id"] = format!("{}postmortem-input.json", super::super::SCHEMA_BASE).into();
    schema
}

pub fn generated() -> Result<super::super::Generated, render::RenderError> {
    let record = Record::order();
    let case = Case::order();
    let rubric = Rubric::order();
    let tags = TraceTags::order();
    let text = Text::<1, UNBOUNDED>::order();
    let natural = Natural::<1>::order();
    let decimal = DecimalText::order();
    let repo = RepositoryName::order();
    let shared = [
        TypeUse {
            pointer: "/properties/record",
            order: &record,
        },
        TypeUse {
            pointer: "/properties/case",
            order: &case,
        },
        TypeUse {
            pointer: "/properties/rubric",
            order: &rubric,
        },
        TypeUse {
            pointer: "/properties/trace_tags",
            order: &tags,
        },
        TypeUse {
            pointer: "/properties/record/properties/id",
            order: &natural,
        },
        TypeUse {
            pointer: "/properties/rubric/properties/version",
            order: &natural,
        },
        TypeUse {
            pointer: "/properties/record/properties/trigger",
            order: &text,
        },
        TypeUse {
            pointer: "/properties/record/properties/reason",
            order: &text,
        },
        TypeUse {
            pointer: "/properties/case/properties/outcome",
            order: &text,
        },
        TypeUse {
            pointer: "/properties/grade_model",
            order: &text,
        },
        TypeUse {
            pointer: "/properties/trace_tags/properties/node",
            order: &decimal,
        },
        TypeUse {
            pointer: "/properties/trace_tags/properties/request",
            order: &decimal,
        },
        TypeUse {
            pointer: "/properties/trace_tags/properties/issue",
            order: &decimal,
        },
        TypeUse {
            pointer: "/properties/trace_tags/properties/repo",
            order: &repo,
        },
    ];
    let inline: &[render::ObjectOrder<'_>] = &[
        (
            "",
            &[
                "$schema",
                "$id",
                "description",
                "type",
                "properties",
                "required",
                "additionalProperties",
            ],
        ),
        (
            "/properties",
            &[
                "record",
                "case",
                "rubric",
                "release",
                "grade_model",
                "deadline_secs",
                "trace_tags",
            ],
        ),
        (
            "/properties/deadline_secs",
            &["type", "minimum", "description"],
        ),
    ];
    let path = format!("{}/postmortem-input.json", super::super::SCHEMA_DIRECTORY);
    let text = render::render(
        &path,
        &schema(),
        &Layout {
            inline,
            shared: &shared,
        },
    )?;
    Ok(super::super::Generated { path, text })
}
