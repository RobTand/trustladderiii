//! Typed audit-input wire contract (#819).

use crate::bounded::*;
use crate::render::{self, Layout, SchemaOrder, TypeOrder, TypeUse};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
/// Serialize through `serde_json::to_value` to retain the producer's sorted keys.
/// Direct struct serialization follows declaration order and differs from those bytes.
pub struct AuditInput {
    #[serde(default, deserialize_with = "present", skip_serializing_if = "Option::is_none")]
    pub tests: Option<crate::roles::maker_input::Tests>,
    #[schemars(with = "IssueRef")]
    pub issue: crate::roles::ingest_input::Issue,
    /// Bounded orientation at the pinned base. Explore source evidence with read-only tools.
    #[schemars(with = "RepoRef")]
    pub repo: crate::roles::ingest_input::Repo,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "MemoriesRef")]
    pub memories: Option<crate::roles::ingest_input::Memories>,

    #[schemars(with = "IngestReplyRef")]
    pub ingest: crate::roles::ingest_reply::IngestReply,
    /// Effective defaults, global and repository protected-path patterns. These edits belong outside TL3.
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "List<Text<1, UNBOUNDED>, 0, UNBOUNDED, true>")]
    pub protected_paths: Option<List<Text<1, UNBOUNDED>, 0, UNBOUNDED, true>>,
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
    pub publication_branch: Option<crate::roles::ingest_input::PublicationBranch>,
}

schema_ref!(IssueRef, "ingest-input.json#/$defs/issue");
schema_ref!(RepoRef, "ingest-input.json#/$defs/repo");
schema_ref!(MemoriesRef, "ingest-input.json#/$defs/memories");
schema_ref!(
    PublicationBranchRef,
    "ingest-input.json#/$defs/publication_branch"
);
schema_ref!(DecisionContextRef, "decision-context.json");
schema_ref!(
    ToolCapabilityRef,
    "ingest-reply.json#/$defs/tool_capability"
);
schema_ref!(IngestReplyRef, "ingest-reply.json");

pub fn schema() -> Value {
    super::input_schema::<AuditInput>("audit-input.json")
}

// Order-only metadata for the published input schema.
impl SchemaOrder for AuditInput {
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
                ],
            ),
            (
                "/properties",
                &[
                    "issue",
                    "repo",
                    "memories",
                    "ingest",
                    "protected_paths",
                    "decision_context",
                    "tool_capabilities",
                    "publication_branch",
                ],
            ),
            ("/properties/repo", &["$ref", "description"]),
            (
                "/properties/protected_paths",
                &["type", "items", "uniqueItems", "description"],
            ),
            ("/properties/decision_context", &["$ref", "description"]),
            (
                "/properties/tool_capabilities",
                &["type", "items", "maxItems"],
            ),
        ])
    }
}
pub fn generated() -> Result<crate::Generated, render::RenderError> {
    let order0 = <AuditInput>::order();
    let order1 = <Text<1, UNBOUNDED>>::order();
    let path = format!("{}/audit-input.json", crate::SCHEMA_DIRECTORY);
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
                    pointer: "/properties/protected_paths/items",
                    order: &order1,
                },
            ],
        },
    )?;
    Ok(crate::Generated { path, text })
}
