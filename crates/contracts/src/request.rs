//! Typed request wire contract (#819).

use super::bounded::{
    AbsolutePath, JsonObject, ModelUrl, Natural, NullableType, RecordSchemaVersion, Text,
    UNBOUNDED, WireRole, present, required_nullable,
};
use super::render::{self, Layout, SchemaOrder, TypeOrder, TypeUse};
use schemars::{JsonSchema, generate::SchemaSettings};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Protocol(pub RecordSchemaVersion);

impl JsonSchema for Protocol {
    fn inline_schema() -> bool {
        true
    }
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "WorkerProtocol".into()
    }
    fn json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        let mut schema = RecordSchemaVersion::json_schema(generator);
        schema.insert("type".into(), "integer".into());
        schema
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Operation {
    pub node: Natural<0>,
    pub generation: Natural<0>,
    pub kind: Text<1, UNBOUNDED>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Model {
    pub base_url: ModelUrl,
    pub name: Text<1, UNBOUNDED>,
    #[serde(deserialize_with = "required_nullable")]
    #[schemars(with = "NullableType<Text<1, UNBOUNDED>>")]
    pub effort: Option<Text<1, UNBOUNDED>>,
    pub budget_tokens: Natural<1>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Prompt {
    pub version: Natural<1>,
    pub text: Text<1, UNBOUNDED>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Memory {
    pub summary_file: AbsolutePath,
    pub repo: String,
    pub issue: Natural<1>,
    pub request: String,
    pub epoch: Natural<0>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkerRequest {
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "Option<JsonObject>")]
    pub mlflow: Option<Option<JsonObject>>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "JsonObject")]
    pub trace: Option<JsonObject>,
    pub protocol: Protocol,
    pub operation: Operation,
    pub role: WireRole,
    pub model: Model,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "Option<AbsolutePath>")]
    pub hindsight_key_file: Option<Option<AbsolutePath>>,
    pub key_file: AbsolutePath,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "Natural<1>")]
    pub test_cpus: Option<Natural<1>>,
    /// The agent worker silence allowance in seconds; the maker retro stays inside half of it.
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "Natural<1>")]
    pub silence_secs: Option<Natural<1>>,
    /// The agent worker no-progress allowance in seconds; the maker retro stays inside half of it.
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "Natural<1>")]
    pub no_progress_secs: Option<Natural<1>>,
    pub inputs: JsonObject,
    pub worktree: AbsolutePath,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "AbsolutePath")]
    pub session_file: Option<AbsolutePath>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "Natural<0>")]
    pub last_accepted_generation: Option<Natural<0>>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "bool")]
    pub resume_session: Option<bool>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "Prompt")]
    pub prompt: Option<Prompt>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "Memory")]
    pub memory: Option<Memory>,
}

impl SchemaOrder for Protocol {
    fn order() -> TypeOrder<'static> {
        TypeOrder::of::<Self>(&[("", &["const", "type"])])
    }
}

impl SchemaOrder for Operation {
    fn order() -> TypeOrder<'static> {
        TypeOrder::of::<Self>(&[
            (
                "",
                &["type", "properties", "required", "additionalProperties"],
            ),
            ("/properties", &["node", "generation", "kind"]),
        ])
    }
}

impl SchemaOrder for Model {
    fn order() -> TypeOrder<'static> {
        TypeOrder::of::<Self>(&[
            (
                "",
                &["type", "properties", "required", "additionalProperties"],
            ),
            (
                "/properties",
                &["base_url", "name", "effort", "budget_tokens"],
            ),
        ])
    }
}

impl SchemaOrder for Prompt {
    fn order() -> TypeOrder<'static> {
        TypeOrder::of::<Self>(&[
            (
                "",
                &["type", "properties", "required", "additionalProperties"],
            ),
            ("/properties", &["version", "text"]),
        ])
    }
}

impl SchemaOrder for Memory {
    fn order() -> TypeOrder<'static> {
        TypeOrder::of::<Self>(&[
            (
                "",
                &["type", "properties", "required", "additionalProperties"],
            ),
            (
                "/properties",
                &["summary_file", "repo", "issue", "request", "epoch"],
            ),
        ])
    }
}

pub fn schema() -> Value {
    let mut generator = SchemaSettings::draft2020_12()
        .with(|s| s.inline_subschemas = true)
        .into_generator();
    let mut schema = WorkerRequest::json_schema(&mut generator).to_value();
    schema["$schema"] = "https://json-schema.org/draft/2020-12/schema".into();
    schema["$id"] = format!("{}request.json", super::SCHEMA_BASE).into();
    schema
}

pub fn generated() -> Result<super::Generated, render::RenderError> {
    let protocol = Protocol::order();
    let operation = Operation::order();
    let model = Model::order();
    let prompt = Prompt::order();
    let memory = Memory::order();
    let text = Text::<1, UNBOUNDED>::order();
    let nullable_text = NullableType::<Text<1, UNBOUNDED>>::order();
    let natural0 = Natural::<0>::order();
    let natural1 = Natural::<1>::order();
    let path_order = AbsolutePath::order();
    let nullable_path = NullableType::<AbsolutePath>::order();
    let url = ModelUrl::order();
    let role = WireRole::order();
    let shared = [
        TypeUse {
            pointer: "/properties/protocol",
            order: &protocol,
        },
        TypeUse {
            pointer: "/properties/operation",
            order: &operation,
        },
        TypeUse {
            pointer: "/properties/model",
            order: &model,
        },
        TypeUse {
            pointer: "/properties/prompt",
            order: &prompt,
        },
        TypeUse {
            pointer: "/properties/memory",
            order: &memory,
        },
        TypeUse {
            pointer: "/properties/operation/properties/node",
            order: &natural0,
        },
        TypeUse {
            pointer: "/properties/operation/properties/generation",
            order: &natural0,
        },
        TypeUse {
            pointer: "/properties/last_accepted_generation",
            order: &natural0,
        },
        TypeUse {
            pointer: "/properties/memory/properties/epoch",
            order: &natural0,
        },
        TypeUse {
            pointer: "/properties/model/properties/budget_tokens",
            order: &natural1,
        },
        TypeUse {
            pointer: "/properties/test_cpus",
            order: &natural1,
        },
        TypeUse {
            pointer: "/properties/prompt/properties/version",
            order: &natural1,
        },
        TypeUse {
            pointer: "/properties/memory/properties/issue",
            order: &natural1,
        },
        TypeUse {
            pointer: "/properties/operation/properties/kind",
            order: &text,
        },
        TypeUse {
            pointer: "/properties/model/properties/name",
            order: &text,
        },
        TypeUse {
            pointer: "/properties/prompt/properties/text",
            order: &text,
        },
        TypeUse {
            pointer: "/properties/key_file",
            order: &path_order,
        },
        TypeUse {
            pointer: "/properties/worktree",
            order: &path_order,
        },
        TypeUse {
            pointer: "/properties/session_file",
            order: &path_order,
        },
        TypeUse {
            pointer: "/properties/memory/properties/summary_file",
            order: &path_order,
        },
        TypeUse {
            pointer: "/properties/hindsight_key_file",
            order: &nullable_path,
        },
        TypeUse {
            pointer: "/properties/model/properties/effort",
            order: &nullable_text,
        },
        TypeUse {
            pointer: "/properties/model/properties/base_url",
            order: &url,
        },
        TypeUse {
            pointer: "/properties/role",
            order: &role,
        },
    ];
    let inline: &[render::ObjectOrder<'_>] = &[
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
            &[
                "mlflow",
                "trace",
                "protocol",
                "operation",
                "role",
                "model",
                "hindsight_key_file",
                "key_file",
                "test_cpus",
                "silence_secs",
                "no_progress_secs",
                "inputs",
                "worktree",
                "session_file",
                "last_accepted_generation",
                "resume_session",
                "prompt",
                "memory",
            ],
        ),
        (
            "/properties/silence_secs",
            &["type", "minimum", "description"],
        ),
        (
            "/properties/no_progress_secs",
            &["type", "minimum", "description"],
        ),
    ];
    let path = format!("{}/request.json", super::SCHEMA_DIRECTORY);
    let text = render::render(
        &path,
        &schema(),
        &Layout {
            inline,
            shared: &shared,
        },
    )?;
    Ok(super::Generated { path, text })
}
