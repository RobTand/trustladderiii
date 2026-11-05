//! Controller-recorded decisions, generated without changing decision-context.json (#819).

use super::bounded::{Integer, List, Natural, Text, UNBOUNDED, present, required_nullable};
use super::render::{self, Layout, SchemaOrder, TypeOrder, TypeUse};
use schemars::{JsonSchema, Schema, SchemaGenerator, generate::SchemaSettings, json_schema};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::borrow::Cow;

pub type NonEmpty = Text<1, UNBOUNDED>;
pub type Scope = List<NonEmpty, 0, UNBOUNDED>;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SchemaVersion(Integer<1, 1>);

impl Default for SchemaVersion {
    fn default() -> Self {
        Self(Integer::new(1).expect("the protocol version is one"))
    }
}

impl JsonSchema for SchemaVersion {
    fn inline_schema() -> bool {
        true
    }
    fn schema_name() -> Cow<'static, str> {
        "DecisionContextVersion".into()
    }
    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({"const": 1})
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Decision {
    pub id: NonEmpty,
    #[serde(deserialize_with = "required_nullable")]
    #[schemars(required)]
    pub source_comment: Option<String>,
    pub question: serde_json::Map<String, Value>,
    pub answer: NonEmpty,
    pub allowed_scope: Scope,
    pub restrictions: Scope,
    pub origin_request_version: Natural<0>,
    #[serde(deserialize_with = "required_nullable")]
    #[schemars(required)]
    pub superseded_by: Option<String>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "NonEmpty")]
    pub supersession_reason: Option<NonEmpty>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DecisionContext {
    pub schema_version: SchemaVersion,
    pub request_version: Natural<0>,
    pub decisions: List<Decision, 0, 64>,
}

impl SchemaOrder for Decision {
    fn order() -> TypeOrder<'static> {
        TypeOrder::of::<Self>(&[
            (
                "",
                &["type", "properties", "required", "additionalProperties"],
            ),
            (
                "/properties",
                &[
                    "id",
                    "source_comment",
                    "question",
                    "answer",
                    "allowed_scope",
                    "restrictions",
                    "origin_request_version",
                    "superseded_by",
                    "supersession_reason",
                ],
            ),
        ])
    }
}

impl SchemaOrder for Scope {
    fn order() -> TypeOrder<'static> {
        TypeOrder::of::<Self>(&[("", &["type", "items"])])
    }
}

/// The root adds only the standard document identifiers to the derived type schema.
pub fn schema() -> Value {
    let mut generator = SchemaSettings::draft2020_12()
        .with(|settings| settings.inline_subschemas = true)
        .into_generator();
    let mut schema = DecisionContext::json_schema(&mut generator).to_value();
    schema["$schema"] = "https://json-schema.org/draft/2020-12/schema".into();
    schema["$id"] = format!("{}decision-context.json", super::SCHEMA_BASE).into();
    schema
}

pub fn generated() -> Result<super::Generated, render::RenderError> {
    let decision = Decision::order();
    let scope = Scope::order();
    let text = NonEmpty::order();
    let natural = Natural::<0>::order();
    let shared = [
        TypeUse {
            pointer: "/properties/request_version",
            order: &natural,
        },
        TypeUse {
            pointer: "/properties/decisions/items",
            order: &decision,
        },
        TypeUse {
            pointer: "/properties/decisions/items/properties/id",
            order: &text,
        },
        TypeUse {
            pointer: "/properties/decisions/items/properties/answer",
            order: &text,
        },
        TypeUse {
            pointer: "/properties/decisions/items/properties/allowed_scope",
            order: &scope,
        },
        TypeUse {
            pointer: "/properties/decisions/items/properties/allowed_scope/items",
            order: &text,
        },
        TypeUse {
            pointer: "/properties/decisions/items/properties/restrictions",
            order: &scope,
        },
        TypeUse {
            pointer: "/properties/decisions/items/properties/restrictions/items",
            order: &text,
        },
        TypeUse {
            pointer: "/properties/decisions/items/properties/origin_request_version",
            order: &natural,
        },
        TypeUse {
            pointer: "/properties/decisions/items/properties/supersession_reason",
            order: &text,
        },
    ];
    let layout = Layout {
        inline: &[
            (
                "",
                &[
                    "$schema",
                    "$id",
                    "type",
                    "properties",
                    "required",
                    "additionalProperties",
                ],
            ),
            (
                "/properties",
                &["schema_version", "request_version", "decisions"],
            ),
            ("/properties/decisions", &["type", "items", "maxItems"]),
        ],
        shared: &shared,
    };
    let path = format!("{}/decision-context.json", super::SCHEMA_DIRECTORY);
    let text = render::render(&path, &schema(), &layout)?;
    Ok(super::Generated { path, text })
}
