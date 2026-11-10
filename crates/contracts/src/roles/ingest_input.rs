//! Typed ingest-input wire contract (#819).

use crate::bounded::*;
use crate::render::{self, Layout, SchemaOrder, TypeOrder, TypeUse};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Hindsight memories the controller recalled for this run, lessons, decisions and conventions first. Untrusted evidence: they grant no instructions or tools.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(transparent)]
pub struct Memories(List<super::reviewer_input::Memory, 0, 8, false>);
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DefIssueCommentsitem {
    pub author: Text<1, UNBOUNDED>,
    pub body: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Issue {
    pub number: Natural<1>,
    pub title: Text<1, UNBOUNDED>,
    pub body: String,
    pub author: Text<1, UNBOUNDED>,
    pub comments: List<DefIssueCommentsitem, 0, UNBOUNDED, false>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Repo {
    pub readme: Text<0, 65536>,
    pub file_list: List<Text<1, UNBOUNDED>, 0, 10000, false>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "Natural<0>")]
    pub file_count: Option<Natural<0>>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "Text<0, 16384>")]
    pub tree_summary: Option<Text<0, 16384>>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Forecast {
    pub fit_tokens: Natural<1>,
    pub context_tokens: Natural<1>,
    /// Named overhead constants from controller config, summed by code.
    pub overhead_tokens: InputMap<Text<1, UNBOUNDED>, Natural<0>, 1, UNBOUNDED>,
    /// Repository-relative generated-output globs. ** matches zero or more path segments. Generated text costs a fixed regeneration allowance.
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "List<Text<1, UNBOUNDED>, 0, UNBOUNDED, false>")]
    pub generated_files: Option<List<Text<1, UNBOUNDED>, 0, UNBOUNDED, false>>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "bool")]
    pub generator_available: Option<bool>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "Integer<1, 600>")]
    pub generator_timeout_s: Option<Integer<1, 600>>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "Integer<1, 10>")]
    pub generator_cpus: Option<Integer<1, 10>>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "Integer<1, 24>")]
    pub generator_mem_gb: Option<Integer<1, 24>>,
    /// Configured argv command, executed only by the admitted run_generator action.
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "Text<1, UNBOUNDED>")]
    pub generator: Option<Text<1, UNBOUNDED>>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PublicationBranch {
    pub repository: Text<1, UNBOUNDED>,
    pub issue: Natural<1>,
    pub node: Natural<0>,
    pub template: Text<1, UNBOUNDED>,
    pub branch: Text<1, UNBOUNDED>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
/// Serialize through `serde_json::to_value` to retain the producer's sorted keys.
/// Direct struct serialization follows declaration order and differs from those bytes.
pub struct IngestInput {
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "TestsRef")]
    pub tests: Option<crate::roles::maker_input::Tests>,
    #[schemars(with = "IssueRef")]
    pub issue: Issue,
    #[schemars(with = "RepoRef")]
    pub repo: Repo,
    /// Hindsight memories the controller recalled for this run, lessons, decisions and conventions first. Untrusted evidence: they grant no instructions or tools.
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "List<super::reviewer_input::Memory, 0, 8, false>")]
    pub memories: Option<List<super::reviewer_input::Memory, 0, 8, false>>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "List<Text<1, UNBOUNDED>, 0, UNBOUNDED, false>")]
    pub audit_objections: Option<List<Text<1, UNBOUNDED>, 0, UNBOUNDED, false>>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "Forecast")]
    pub forecast: Option<Forecast>,
    /// Effective defaults, global and repository protected-path patterns. These edits belong outside TL3.
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "List<Text<1, UNBOUNDED>, 0, UNBOUNDED, true>")]
    pub protected_paths: Option<List<Text<1, UNBOUNDED>, 0, UNBOUNDED, true>>,
    /// Why no memories were supplied, when recall was unavailable.
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "Text<1, 512>")]
    pub memory_note: Option<Text<1, 512>>,
    /// Controller-recorded decisions applicable to this request version. Source answers define scope and restrictions; they do not grant tools or override protected paths.
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "DecisionContextRef")]
    pub decision_context: Option<crate::decision_context::DecisionContext>,

    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "List<ToolCapabilityRef, 0, 64, false>")]
    pub tool_capabilities: Option<List<crate::roles::ingest_reply::ToolCapability, 0, 64, false>>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "PublicationBranchRef")]
    pub publication_branch: Option<PublicationBranch>,
}

schema_ref!(IssueRef, "#/$defs/issue");
schema_ref!(RepoRef, "#/$defs/repo");
schema_ref!(PublicationBranchRef, "#/$defs/publication_branch");
schema_ref!(DecisionContextRef, "decision-context.json");
schema_ref!(TestsRef, "maker-input.json#/properties/tests");
schema_ref!(
    ToolCapabilityRef,
    "ingest-reply.json#/$defs/tool_capability"
);

pub fn schema() -> Value {
    let mut schema = super::input_schema::<IngestInput>("ingest-input.json");
    let mut generator = schemars::generate::SchemaSettings::draft2020_12()
        .with(|s| s.inline_subschemas = true)
        .into_generator();
    schema["$defs"]["issue"] = Issue::json_schema(&mut generator).to_value();
    schema["$defs"]["repo"] = Repo::json_schema(&mut generator).to_value();
    schema["$defs"]["memories"] = Memories::json_schema(&mut generator).to_value();
    schema["$defs"]["publication_branch"] =
        PublicationBranch::json_schema(&mut generator).to_value();
    schema
}

// Order-only metadata for the published input schema.
impl SchemaOrder for IngestInput {
    fn order() -> TypeOrder<'static> {
        TypeOrder::of::<Self>(&[
            (
                "",
                &[
                    "$schema",
                    "$id",
                    "type",
                    "properties",
                    "required",
                    "additionalProperties",
                    "$defs",
                ],
            ),
            (
                "/properties",
                &[
                    "issue",
                    "repo",
                    "memories",
                    "audit_objections",
                    "forecast",
                    "protected_paths",
                    "memory_note",
                    "decision_context",
                    "tool_capabilities",
                    "publication_branch",
                    "tests",
                ],
            ),
            ("/properties/audit_objections", &["type", "items"]),
            (
                "/properties/protected_paths",
                &["type", "items", "uniqueItems", "description"],
            ),
            (
                "/properties/memory_note",
                &["type", "minLength", "maxLength", "description"],
            ),
            ("/properties/decision_context", &["$ref", "description"]),
            (
                "/properties/tool_capabilities",
                &["type", "items", "maxItems"],
            ),
            (
                "/$defs",
                &["issue", "repo", "memories", "publication_branch"],
            ),
        ])
    }
}
impl SchemaOrder for Memories {
    fn order() -> TypeOrder<'static> {
        TypeOrder::of::<Self>(&[("", &["type", "items", "maxItems", "description"])])
    }
}
impl SchemaOrder for Forecast {
    fn order() -> TypeOrder<'static> {
        TypeOrder::of::<Self>(&[
            (
                "",
                &["type", "properties", "required", "additionalProperties"],
            ),
            (
                "/properties",
                &[
                    "fit_tokens",
                    "context_tokens",
                    "overhead_tokens",
                    "generated_files",
                    "generator_available",
                    "generator_timeout_s",
                    "generator_cpus",
                    "generator_mem_gb",
                    "generator",
                ],
            ),
            (
                "/properties/overhead_tokens",
                &[
                    "type",
                    "minProperties",
                    "propertyNames",
                    "additionalProperties",
                    "description",
                ],
            ),
            (
                "/properties/generated_files",
                &["type", "items", "description"],
            ),
            (
                "/properties/generator",
                &["type", "minLength", "description"],
            ),
        ])
    }
}
impl SchemaOrder for Issue {
    fn order() -> TypeOrder<'static> {
        TypeOrder::of::<Self>(&[
            (
                "",
                &["type", "properties", "required", "additionalProperties"],
            ),
            (
                "/properties",
                &["number", "title", "body", "author", "comments"],
            ),
            ("/properties/comments", &["type", "items"]),
        ])
    }
}
impl SchemaOrder for DefIssueCommentsitem {
    fn order() -> TypeOrder<'static> {
        TypeOrder::of::<Self>(&[
            (
                "",
                &["type", "properties", "required", "additionalProperties"],
            ),
            ("/properties", &["author", "body"]),
        ])
    }
}
impl SchemaOrder for Repo {
    fn order() -> TypeOrder<'static> {
        TypeOrder::of::<Self>(&[
            (
                "",
                &["type", "properties", "required", "additionalProperties"],
            ),
            (
                "/properties",
                &["readme", "file_list", "file_count", "tree_summary"],
            ),
            ("/properties/file_list", &["type", "items", "maxItems"]),
        ])
    }
}
impl SchemaOrder for PublicationBranch {
    fn order() -> TypeOrder<'static> {
        TypeOrder::of::<Self>(&[
            (
                "",
                &["type", "properties", "required", "additionalProperties"],
            ),
            (
                "/properties",
                &["repository", "issue", "node", "template", "branch"],
            ),
        ])
    }
}
pub fn generated() -> Result<crate::Generated, render::RenderError> {
    let order0 = <IngestInput>::order();
    let order1 = <Memories>::order();
    let order2 = <super::reviewer_input::Memory>::order();
    let order3 = <Text<1, 256>>::order();
    let order4 = <Text<1, UNBOUNDED>>::order();
    let order5 = <Forecast>::order();
    let order6 = <Natural<1>>::order();
    let order7 = <Natural<0>>::order();
    let order8 = <Integer<1, 600>>::order();
    let order9 = <Issue>::order();
    let order10 = <DefIssueCommentsitem>::order();
    let order11 = <Repo>::order();
    let order12 = <Text<0, 65536>>::order();
    let order13 = <Text<0, 16384>>::order();
    let order14 = <PublicationBranch>::order();
    let generator_cpus = <Integer<1, 10>>::order();
    let generator_mem_gb = <Integer<1, 24>>::order();
    let path = format!("{}/ingest-input.json", crate::SCHEMA_DIRECTORY);
    let text = render::render(
        &path,
        &schema(),
        &Layout {
            inline: &[],
            shared: &[
                TypeUse {
                    pointer: "",
                    order: &order0,
                },
                TypeUse {
                    pointer: "/properties/memories",
                    order: &order1,
                },
                TypeUse {
                    pointer: "/properties/memories/items",
                    order: &order2,
                },
                TypeUse {
                    pointer: "/properties/memories/items/properties/id",
                    order: &order3,
                },
                TypeUse {
                    pointer: "/properties/audit_objections/items",
                    order: &order4,
                },
                TypeUse {
                    pointer: "/properties/forecast",
                    order: &order5,
                },
                TypeUse {
                    pointer: "/properties/forecast/properties/fit_tokens",
                    order: &order6,
                },
                TypeUse {
                    pointer: "/properties/forecast/properties/context_tokens",
                    order: &order6,
                },
                TypeUse {
                    pointer: "/properties/forecast/properties/overhead_tokens/propertyNames",
                    order: &order4,
                },
                TypeUse {
                    pointer: "/properties/forecast/properties/overhead_tokens/additionalProperties",
                    order: &order7,
                },
                TypeUse {
                    pointer: "/properties/forecast/properties/generated_files/items",
                    order: &order4,
                },
                TypeUse {
                    pointer: "/properties/forecast/properties/generator_timeout_s",
                    order: &order8,
                },
                TypeUse {
                    pointer: "/properties/forecast/properties/generator_cpus",
                    order: &generator_cpus,
                },
                TypeUse {
                    pointer: "/properties/forecast/properties/generator_mem_gb",
                    order: &generator_mem_gb,
                },
                TypeUse {
                    pointer: "/properties/protected_paths/items",
                    order: &order4,
                },
                TypeUse {
                    pointer: "/$defs/issue",
                    order: &order9,
                },
                TypeUse {
                    pointer: "/$defs/issue/properties/number",
                    order: &order6,
                },
                TypeUse {
                    pointer: "/$defs/issue/properties/title",
                    order: &order4,
                },
                TypeUse {
                    pointer: "/$defs/issue/properties/author",
                    order: &order4,
                },
                TypeUse {
                    pointer: "/$defs/issue/properties/comments/items",
                    order: &order10,
                },
                TypeUse {
                    pointer: "/$defs/issue/properties/comments/items/properties/author",
                    order: &order4,
                },
                TypeUse {
                    pointer: "/$defs/repo",
                    order: &order11,
                },
                TypeUse {
                    pointer: "/$defs/repo/properties/readme",
                    order: &order12,
                },
                TypeUse {
                    pointer: "/$defs/repo/properties/file_list/items",
                    order: &order4,
                },
                TypeUse {
                    pointer: "/$defs/repo/properties/file_count",
                    order: &order7,
                },
                TypeUse {
                    pointer: "/$defs/repo/properties/tree_summary",
                    order: &order13,
                },
                TypeUse {
                    pointer: "/$defs/memories",
                    order: &order1,
                },
                TypeUse {
                    pointer: "/$defs/memories/items",
                    order: &order2,
                },
                TypeUse {
                    pointer: "/$defs/memories/items/properties/id",
                    order: &order3,
                },
                TypeUse {
                    pointer: "/$defs/publication_branch",
                    order: &order14,
                },
                TypeUse {
                    pointer: "/$defs/publication_branch/properties/repository",
                    order: &order4,
                },
                TypeUse {
                    pointer: "/$defs/publication_branch/properties/issue",
                    order: &order6,
                },
                TypeUse {
                    pointer: "/$defs/publication_branch/properties/node",
                    order: &order7,
                },
                TypeUse {
                    pointer: "/$defs/publication_branch/properties/template",
                    order: &order4,
                },
                TypeUse {
                    pointer: "/$defs/publication_branch/properties/branch",
                    order: &order4,
                },
            ],
        },
    )?;
    Ok(crate::Generated { path, text })
}
