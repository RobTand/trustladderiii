//! Typed classifier-input wire contract (#819).

use crate::bounded::*;
use crate::render::{self, Layout, SchemaOrder, TypeOrder, TypeUse};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Comment {
    pub author: Text<1, UNBOUNDED>,
    pub body: String,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "Text<1, 1000>")]
    pub delegated_by: Option<Text<1, 1000>>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(inline, transform = reply_enum_schema)]
pub enum StateOpenQuestionKind {
    #[serde(rename = "audit_disagreement")]
    AuditDisagreement,
    #[serde(rename = "needs_splitting")]
    NeedsSplitting,
    #[serde(rename = "cant_meet")]
    CantMeet,
    #[serde(rename = "out_of_credit")]
    OutOfCredit,
    #[serde(rename = "effect_failed")]
    EffectFailed,
    #[serde(rename = "cleanup_cant_finish")]
    CleanupCantFinish,
    #[serde(rename = "unclear_answer")]
    UnclearAnswer,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(inline, transform = reply_enum_schema)]
pub enum StateOpenQuestionAnswersitem {
    #[serde(rename = "continue")]
    Continue,
    #[serde(rename = "change_request")]
    ChangeRequest,
    #[serde(rename = "stop")]
    Stop,
    #[serde(rename = "try_again")]
    TryAgain,
    #[serde(rename = "add_credit")]
    AddCredit,
    #[serde(rename = "retry")]
    Retry,
    #[serde(rename = "mark_handled")]
    MarkHandled,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StateOpenQuestion {
    pub id: Text<1, UNBOUNDED>,
    pub kind: StateOpenQuestionKind,
    pub answers: List<StateOpenQuestionAnswersitem, 1, UNBOUNDED, true>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct State {
    pub phase: Text<1, UNBOUNDED>,
    pub closed: bool,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "bool")]
    pub restartable: Option<bool>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "NullFirst<StateOpenQuestion>")]
    pub open_question: Option<Option<StateOpenQuestion>>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Conversation {
    pub comment_id: String,
    pub comment: String,
    pub question: String,
}

/// Fresh GitHub issue, frozen before restart intake. Its current title and body override contradictory restored decisions.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(transform = crate::render::omit_open_additional_properties)]
pub struct ReconcileIssue {
    pub number: ReplyNumber<0, 0, false, false, true>,
    pub title: String,
    #[serde(deserialize_with = "required_nullable")]
    #[schemars(required, with = "NullableString")]
    pub body: Option<String>,
    pub updated_at: String,
    #[serde(flatten)]
    extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
/// Serialize through `serde_json::to_value` to retain the producer's sorted keys.
/// Direct struct serialization follows declaration order and differs from those bytes.
pub struct ClassifierInput {
    pub comment: Comment,
    pub state: State,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "Conversation")]
    pub conversation: Option<Conversation>,
    /// Controller-recorded decisions applicable to this request version. Source answers define scope and restrictions; they do not grant tools or override protected paths.
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "DecisionContextRef")]
    pub decision_context: Option<crate::decision_context::DecisionContext>,
    /// The complete current question and findings whose option Rob is answering.
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "JsonObject")]
    pub question_context: Option<JsonObject>,
    /// A typed console choice. Copy these fields exactly; assess only its decision details and supersedes.

    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "ClassifierReplyRef")]
    pub selected_answer: Option<crate::roles::classifier_reply::ClassifierReply>,
    /// Fresh GitHub issue, frozen before restart intake. Its current title and body override contradictory restored decisions.
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "ReconcileIssue")]
    pub reconcile_issue: Option<ReconcileIssue>,
}

schema_ref!(DecisionContextRef, "decision-context.json");
schema_ref!(ClassifierReplyRef, "classifier-reply.json");

pub fn schema() -> Value {
    super::input_schema::<ClassifierInput>("classifier-input.json")
}

// Order-only metadata for the published input schema.
impl SchemaOrder for ClassifierInput {
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
                    "comment",
                    "state",
                    "conversation",
                    "decision_context",
                    "question_context",
                    "selected_answer",
                    "reconcile_issue",
                ],
            ),
            ("/properties/decision_context", &["$ref", "description"]),
            ("/properties/question_context", &["type", "description"]),
            ("/properties/selected_answer", &["$ref", "description"]),
        ])
    }
}
impl SchemaOrder for Comment {
    fn order() -> TypeOrder<'static> {
        TypeOrder::of::<Self>(&[
            (
                "",
                &["type", "properties", "required", "additionalProperties"],
            ),
            ("/properties", &["author", "body", "delegated_by"]),
        ])
    }
}
impl SchemaOrder for State {
    fn order() -> TypeOrder<'static> {
        TypeOrder::of::<Self>(&[
            (
                "",
                &["type", "properties", "required", "additionalProperties"],
            ),
            (
                "/properties",
                &["phase", "closed", "restartable", "open_question"],
            ),
        ])
    }
}
impl SchemaOrder for StateOpenQuestion {
    fn order() -> TypeOrder<'static> {
        TypeOrder::of::<Self>(&[
            (
                "",
                &["type", "properties", "required", "additionalProperties"],
            ),
            ("/properties", &["id", "kind", "answers"]),
            (
                "/properties/answers",
                &["type", "items", "minItems", "uniqueItems"],
            ),
        ])
    }
}
impl SchemaOrder for Conversation {
    fn order() -> TypeOrder<'static> {
        TypeOrder::of::<Self>(&[
            (
                "",
                &["type", "properties", "required", "additionalProperties"],
            ),
            ("/properties", &["comment_id", "comment", "question"]),
        ])
    }
}
impl SchemaOrder for ReconcileIssue {
    fn order() -> TypeOrder<'static> {
        TypeOrder::of::<Self>(&[
            ("", &["type", "description", "required", "properties"]),
            ("/properties", &["number", "title", "body", "updated_at"]),
        ])
    }
}
pub fn generated() -> Result<crate::Generated, render::RenderError> {
    let order0 = <ClassifierInput>::order();
    let order1 = <Comment>::order();
    let order2 = <Text<1, UNBOUNDED>>::order();
    let order3 = <Text<1, 1000>>::order();
    let order4 = <State>::order();
    let order5 = <StateOpenQuestion>::order();
    let order6 = <Conversation>::order();
    let order7 = <ReconcileIssue>::order();
    let path = format!("{}/classifier-input.json", crate::SCHEMA_DIRECTORY);
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
                    pointer: "/properties/comment",
                    order: &order1,
                },
                TypeUse {
                    pointer: "/properties/comment/properties/author",
                    order: &order2,
                },
                TypeUse {
                    pointer: "/properties/comment/properties/delegated_by",
                    order: &order3,
                },
                TypeUse {
                    pointer: "/properties/state",
                    order: &order4,
                },
                TypeUse {
                    pointer: "/properties/state/properties/phase",
                    order: &order2,
                },
                TypeUse {
                    pointer: "/properties/state/properties/open_question/oneOf/1",
                    order: &order5,
                },
                TypeUse {
                    pointer: "/properties/state/properties/open_question/oneOf/1/properties/id",
                    order: &order2,
                },
                TypeUse {
                    pointer: "/properties/conversation",
                    order: &order6,
                },
                TypeUse {
                    pointer: "/properties/reconcile_issue",
                    order: &order7,
                },
            ],
        },
    )?;
    Ok(crate::Generated { path, text })
}
