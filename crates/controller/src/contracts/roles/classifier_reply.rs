use crate::contracts::bounded::{Text as BoundedText, *};
use crate::contracts::render::{Layout, TypeOrder, TypeUse};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(rename = "decision")]
pub struct Decision {
    pub allowed_scope: List<BoundedText<1, UNBOUNDED>, 0, UNBOUNDED, true>,
    pub restrictions: List<BoundedText<1, UNBOUNDED>, 0, UNBOUNDED, true>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(inline)]
#[schemars(transform = reply_constant_schema)]
pub enum ClassifierReplyChangeRequestKind {
    #[serde(rename = "change_request")]
    Value,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(inline)]
pub struct ClassifierReplyChangeRequest {
    pub kind: ClassifierReplyChangeRequestKind,
    pub new_text: BoundedText<1, UNBOUNDED>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "List<BoundedText<1, UNBOUNDED>, 0, UNBOUNDED, true>", default)]
    pub supersedes: Option<List<BoundedText<1, UNBOUNDED>, 0, UNBOUNDED, true>>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(inline)]
#[schemars(transform = reply_enum_schema)]
pub enum ClassifierReplyValue2Kind {
    #[serde(rename = "stop")]
    Stop,
    #[serde(rename = "restart")]
    Restart,
    #[serde(rename = "pause")]
    Pause,
    #[serde(rename = "resume")]
    Resume,
    #[serde(rename = "remark")]
    Remark,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(inline)]
pub struct ClassifierReplyValue2 {
    pub kind: ClassifierReplyValue2Kind,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(inline)]
#[schemars(transform = reply_constant_schema)]
pub enum ClassifierReplyUnsureKind {
    #[serde(rename = "unsure")]
    Value,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(inline)]
pub struct ClassifierReplyUnsure {
    pub kind: ClassifierReplyUnsureKind,
    pub question: ReplyText<1, 1, 300>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(inline)]
#[schemars(transform = reply_constant_schema)]
pub enum ClassifierReplyComplaintKind {
    #[serde(rename = "complaint")]
    Value,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(inline)]
pub struct ClassifierReplyComplaint {
    pub kind: ClassifierReplyComplaintKind,
    pub summary: ReplyText<1, 1, UNBOUNDED>,
    pub revert: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(inline)]
#[schemars(transform = reply_constant_schema)]
pub enum ClassifierReplyAnswerKind {
    #[serde(rename = "answer")]
    Value,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(inline)]
#[schemars(transform = reply_enum_schema)]
pub enum ClassifierReplyAnswerAnswer {
    #[serde(rename = "continue")]
    Continue,
    #[serde(rename = "stop")]
    Stop,
    #[serde(rename = "retry")]
    Retry,
    #[serde(rename = "mark_handled")]
    MarkHandled,
    #[serde(rename = "unclear")]
    Unclear,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(inline)]
pub struct ClassifierReplyAnswer {
    pub kind: ClassifierReplyAnswerKind,
    pub question_id: BoundedText<1, UNBOUNDED>,
    pub answer: ClassifierReplyAnswerAnswer,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "Decision", default)]
    pub decision: Option<Decision>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(inline)]
#[schemars(transform = reply_constant_schema)]
pub enum ClassifierReplyAnswerChangeRequestKind {
    #[serde(rename = "answer")]
    Value,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(inline)]
#[schemars(transform = reply_constant_schema)]
pub enum ClassifierReplyAnswerChangeRequestAnswer {
    #[serde(rename = "change_request")]
    Value,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(inline)]
pub struct ClassifierReplyAnswerChangeRequest {
    pub kind: ClassifierReplyAnswerChangeRequestKind,
    pub question_id: BoundedText<1, UNBOUNDED>,
    pub answer: ClassifierReplyAnswerChangeRequestAnswer,
    pub new_text: BoundedText<1, UNBOUNDED>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "Decision", default)]
    pub decision: Option<Decision>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "List<BoundedText<1, UNBOUNDED>, 0, UNBOUNDED, true>", default)]
    pub supersedes: Option<List<BoundedText<1, UNBOUNDED>, 0, UNBOUNDED, true>>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(inline)]
#[schemars(transform = reply_constant_schema)]
pub enum ClassifierReplyAnswerTryAgainKind {
    #[serde(rename = "answer")]
    Value,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(inline)]
#[schemars(transform = reply_constant_schema)]
pub enum ClassifierReplyAnswerTryAgainAnswer {
    #[serde(rename = "try_again")]
    Value,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(inline)]
pub struct ClassifierReplyAnswerTryAgain {
    pub kind: ClassifierReplyAnswerTryAgainKind,
    pub question_id: BoundedText<1, UNBOUNDED>,
    pub answer: ClassifierReplyAnswerTryAgainAnswer,
    pub guidance: ReplyText<0, 1, UNBOUNDED>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "Decision", default)]
    pub decision: Option<Decision>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(inline)]
#[schemars(transform = reply_constant_schema)]
pub enum ClassifierReplyAnswerAddCreditKind {
    #[serde(rename = "answer")]
    Value,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(inline)]
#[schemars(transform = reply_constant_schema)]
pub enum ClassifierReplyAnswerAddCreditAnswer {
    #[serde(rename = "add_credit")]
    Value,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(inline)]
pub struct ClassifierReplyAnswerAddCredit {
    pub kind: ClassifierReplyAnswerAddCreditKind,
    pub question_id: BoundedText<1, UNBOUNDED>,
    pub answer: ClassifierReplyAnswerAddCreditAnswer,
    pub amount: ReplyNumber<1, 100, true, true, true>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "Decision", default)]
    pub decision: Option<Decision>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(inline)]
#[schemars(transform = reply_constant_schema)]
pub enum ClassifierReplyReconcileDecisionsKind {
    #[serde(rename = "reconcile_decisions")]
    Value,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(inline)]
pub struct ClassifierReplyReconcileDecisionsAssessmentsItem {
    pub id: BoundedText<1, UNBOUNDED>,
    pub contradicted: bool,
    pub reason: BoundedText<1, 2000>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(inline)]
pub struct ClassifierReplyReconcileDecisions {
    pub kind: ClassifierReplyReconcileDecisionsKind,
    pub assessments: List<ClassifierReplyReconcileDecisionsAssessmentsItem, 0, 64, false>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
#[schemars(inline)]
#[schemars(transform = reply_one_of)]
pub enum ClassifierReply {
    ChangeRequest(ClassifierReplyChangeRequest),
    Value2(ClassifierReplyValue2),
    Unsure(ClassifierReplyUnsure),
    Complaint(ClassifierReplyComplaint),
    Answer(ClassifierReplyAnswer),
    AnswerChangeRequest(ClassifierReplyAnswerChangeRequest),
    AnswerTryAgain(ClassifierReplyAnswerTryAgain),
    AnswerAddCredit(ClassifierReplyAnswerAddCredit),
    ReconcileDecisions(ClassifierReplyReconcileDecisions),
}

pub fn schema_value() -> serde_json::Value {
    reply_schema_value::<ClassifierReply>("classifier", |g| {
        g.subschema_for::<Decision>();
    })
}

pub fn schema_text() -> String {
    let types = [TypeOrder::of::<Decision>(DECISION_ORDER)];
    let shared = [TypeUse {
        pointer: "/$defs/decision",
        order: &types[0],
    }];
    crate::contracts::render::render(
        "classifier-reply.json",
        &schema_value(),
        &Layout {
            inline: ROOT_ORDER,
            shared: &shared,
        },
    )
    .expect("published reply schema order is exhaustive")
}

pub const ROOT_ORDER: &[crate::contracts::render::ObjectOrder<'static>] = &[
    ("", &["$schema", "$id", "oneOf", "$defs"]),
    (
        "/oneOf/0",
        &["type", "properties", "required", "additionalProperties"],
    ),
    ("/oneOf/0/properties", &["kind", "new_text", "supersedes"]),
    ("/oneOf/0/properties/kind", &["const"]),
    ("/oneOf/0/properties/new_text", &["type", "minLength"]),
    (
        "/oneOf/0/properties/supersedes",
        &["type", "items", "uniqueItems"],
    ),
    (
        "/oneOf/0/properties/supersedes/items",
        &["type", "minLength"],
    ),
    (
        "/oneOf/1",
        &["type", "properties", "required", "additionalProperties"],
    ),
    ("/oneOf/1/properties", &["kind"]),
    ("/oneOf/1/properties/kind", &["enum"]),
    (
        "/oneOf/2",
        &["type", "properties", "required", "additionalProperties"],
    ),
    ("/oneOf/2/properties", &["kind", "question"]),
    ("/oneOf/2/properties/kind", &["const"]),
    (
        "/oneOf/2/properties/question",
        &["type", "minLength", "maxLength", "pattern", "not"],
    ),
    ("/oneOf/2/properties/question/not", &["pattern"]),
    (
        "/oneOf/3",
        &["type", "properties", "required", "additionalProperties"],
    ),
    ("/oneOf/3/properties", &["kind", "summary", "revert"]),
    ("/oneOf/3/properties/kind", &["const"]),
    (
        "/oneOf/3/properties/summary",
        &["type", "minLength", "pattern", "not"],
    ),
    ("/oneOf/3/properties/summary/not", &["pattern"]),
    ("/oneOf/3/properties/revert", &["type"]),
    (
        "/oneOf/4",
        &["type", "properties", "required", "additionalProperties"],
    ),
    (
        "/oneOf/4/properties",
        &["kind", "question_id", "answer", "decision"],
    ),
    ("/oneOf/4/properties/kind", &["const"]),
    ("/oneOf/4/properties/question_id", &["type", "minLength"]),
    ("/oneOf/4/properties/answer", &["enum"]),
    ("/oneOf/4/properties/decision", &["$ref"]),
    (
        "/oneOf/5",
        &["type", "properties", "required", "additionalProperties"],
    ),
    (
        "/oneOf/5/properties",
        &[
            "kind",
            "question_id",
            "answer",
            "new_text",
            "decision",
            "supersedes",
        ],
    ),
    ("/oneOf/5/properties/kind", &["const"]),
    ("/oneOf/5/properties/question_id", &["type", "minLength"]),
    ("/oneOf/5/properties/answer", &["const"]),
    ("/oneOf/5/properties/new_text", &["type", "minLength"]),
    ("/oneOf/5/properties/decision", &["$ref"]),
    (
        "/oneOf/5/properties/supersedes",
        &["type", "items", "uniqueItems"],
    ),
    (
        "/oneOf/5/properties/supersedes/items",
        &["type", "minLength"],
    ),
    (
        "/oneOf/6",
        &["type", "properties", "required", "additionalProperties"],
    ),
    (
        "/oneOf/6/properties",
        &["kind", "question_id", "answer", "guidance", "decision"],
    ),
    ("/oneOf/6/properties/kind", &["const"]),
    ("/oneOf/6/properties/question_id", &["type", "minLength"]),
    ("/oneOf/6/properties/answer", &["const"]),
    (
        "/oneOf/6/properties/guidance",
        &["type", "minLength", "pattern"],
    ),
    ("/oneOf/6/properties/decision", &["$ref"]),
    (
        "/oneOf/7",
        &["type", "properties", "required", "additionalProperties"],
    ),
    (
        "/oneOf/7/properties",
        &["kind", "question_id", "answer", "amount", "decision"],
    ),
    ("/oneOf/7/properties/kind", &["const"]),
    ("/oneOf/7/properties/question_id", &["type", "minLength"]),
    ("/oneOf/7/properties/answer", &["const"]),
    (
        "/oneOf/7/properties/amount",
        &["type", "minimum", "maximum"],
    ),
    ("/oneOf/7/properties/decision", &["$ref"]),
    (
        "/oneOf/8",
        &["type", "properties", "required", "additionalProperties"],
    ),
    ("/oneOf/8/properties", &["kind", "assessments"]),
    ("/oneOf/8/properties/kind", &["const"]),
    (
        "/oneOf/8/properties/assessments",
        &["type", "maxItems", "items"],
    ),
    (
        "/oneOf/8/properties/assessments/items",
        &["type", "properties", "required", "additionalProperties"],
    ),
    (
        "/oneOf/8/properties/assessments/items/properties",
        &["id", "contradicted", "reason"],
    ),
    (
        "/oneOf/8/properties/assessments/items/properties/id",
        &["type", "minLength"],
    ),
    (
        "/oneOf/8/properties/assessments/items/properties/contradicted",
        &["type"],
    ),
    (
        "/oneOf/8/properties/assessments/items/properties/reason",
        &["type", "minLength", "maxLength"],
    ),
    ("/$defs", &["decision"]),
];
pub const DECISION_ORDER: &[crate::contracts::render::ObjectOrder<'static>] = &[
    (
        "",
        &["type", "properties", "required", "additionalProperties"],
    ),
    ("/properties", &["allowed_scope", "restrictions"]),
    (
        "/properties/allowed_scope",
        &["type", "items", "uniqueItems"],
    ),
    ("/properties/allowed_scope/items", &["type", "minLength"]),
    (
        "/properties/restrictions",
        &["type", "items", "uniqueItems"],
    ),
    ("/properties/restrictions/items", &["type", "minLength"]),
];
