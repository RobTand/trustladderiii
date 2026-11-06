use crate::bounded::{Text as BoundedText, *};
use crate::render::{Layout, TypeOrder, TypeUse};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(rename = "finding")]
pub struct Finding {
    pub problem: BoundedText<1, UNBOUNDED>,
    pub evidence: BoundedText<1, UNBOUNDED>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(inline)]
#[schemars(transform = reply_enum_schema)]
pub enum BlockingFindingCitationKind {
    #[serde(rename = "goal")]
    Goal,
    #[serde(rename = "standing_rule")]
    StandingRule,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(inline)]
pub struct BlockingFindingCitation {
    pub kind: BlockingFindingCitationKind,
    pub id: BoundedText<1, UNBOUNDED>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(rename = "blocking_finding")]
pub struct BlockingFinding {
    pub citation: BlockingFindingCitation,
    pub problem: BoundedText<1, UNBOUNDED>,
    pub evidence: BoundedText<1, UNBOUNDED>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(inline)]
#[schemars(transform = reply_constant_schema)]
pub enum ReviewerReplyApproveVerdict { #[serde(rename = "approve")] Value }

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(inline)]
pub struct ReviewerReplyApprove {
    pub verdict: ReviewerReplyApproveVerdict,
    pub head_sha: ReplyText<6, 0, UNBOUNDED>,
    pub blocking_findings: List<BlockingFinding, 0, 0, false>,
    pub findings: List<Finding, 0, UNBOUNDED, false>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(inline)]
#[schemars(transform = reply_constant_schema)]
pub enum ReviewerReplyChangesVerdict { #[serde(rename = "changes")] Value }

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(inline)]
pub struct ReviewerReplyChanges {
    pub verdict: ReviewerReplyChangesVerdict,
    pub blocking_findings: List<BlockingFinding, 1, 20, false>,
    pub findings: List<Finding, 0, UNBOUNDED, false>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(inline)]
#[schemars(transform = reply_constant_schema)]
pub enum ReviewerReplyOutOfRoomVerdict { #[serde(rename = "out_of_room")] Value }

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(inline)]
pub struct ReviewerReplyOutOfRoom {
    pub verdict: ReviewerReplyOutOfRoomVerdict,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
#[schemars(inline)]
#[schemars(transform = reply_one_of)]
pub enum ReviewerReply {
    Approve(ReviewerReplyApprove),
    Changes(ReviewerReplyChanges),
    OutOfRoom(ReviewerReplyOutOfRoom),
}

pub fn schema_value() -> serde_json::Value {
    reply_schema_value::<ReviewerReply>("reviewer", |g| {
    g.subschema_for::<Finding>();
    g.subschema_for::<BlockingFinding>();
    })
}

pub fn schema_text() -> String {
    let types = [
        TypeOrder::of::<Finding>(FINDING_ORDER),
        TypeOrder::of::<BlockingFinding>(BLOCKING_FINDING_ORDER),
    ];
    let shared = [
        TypeUse { pointer: "/$defs/finding", order: &types[0] },
        TypeUse { pointer: "/$defs/blocking_finding", order: &types[1] },
    ];
    crate::render::render("reviewer-reply.json", &schema_value(), &Layout { inline: ROOT_ORDER, shared: &shared }).expect("published reply schema order is exhaustive")
}

pub const ROOT_ORDER: &[crate::render::ObjectOrder<'static>] = &[
    ("", &["$schema", "$id", "oneOf", "$defs"]),
    ("/oneOf/0", &["type", "properties", "required", "additionalProperties"]),
    ("/oneOf/0/properties", &["verdict", "head_sha", "blocking_findings", "findings"]),
    ("/oneOf/0/properties/verdict", &["const"]),
    ("/oneOf/0/properties/head_sha", &["type", "pattern"]),
    ("/oneOf/0/properties/blocking_findings", &["type", "items", "maxItems"]),
    ("/oneOf/0/properties/blocking_findings/items", &["$ref"]),
    ("/oneOf/0/properties/findings", &["type", "items"]),
    ("/oneOf/0/properties/findings/items", &["$ref"]),
    ("/oneOf/1", &["type", "properties", "required", "additionalProperties"]),
    ("/oneOf/1/properties", &["verdict", "blocking_findings", "findings"]),
    ("/oneOf/1/properties/verdict", &["const"]),
    ("/oneOf/1/properties/blocking_findings", &["type", "items", "minItems", "maxItems"]),
    ("/oneOf/1/properties/blocking_findings/items", &["$ref"]),
    ("/oneOf/1/properties/findings", &["type", "items"]),
    ("/oneOf/1/properties/findings/items", &["$ref"]),
    ("/oneOf/2", &["type", "properties", "required", "additionalProperties"]),
    ("/oneOf/2/properties", &["verdict"]),
    ("/oneOf/2/properties/verdict", &["const"]),
    ("/$defs", &["finding", "blocking_finding"]),
];
pub const FINDING_ORDER: &[crate::render::ObjectOrder<'static>] = &[
    ("", &["type", "properties", "required", "additionalProperties"]),
    ("/properties", &["problem", "evidence"]),
    ("/properties/problem", &["type", "minLength"]),
    ("/properties/evidence", &["type", "minLength"]),
];
pub const BLOCKING_FINDING_ORDER: &[crate::render::ObjectOrder<'static>] = &[
    ("", &["type", "properties", "required", "additionalProperties"]),
    ("/properties", &["citation", "problem", "evidence"]),
    ("/properties/citation", &["type", "properties", "required", "additionalProperties"]),
    ("/properties/citation/properties", &["kind", "id"]),
    ("/properties/citation/properties/kind", &["enum"]),
    ("/properties/citation/properties/id", &["type", "minLength"]),
    ("/properties/problem", &["type", "minLength"]),
    ("/properties/evidence", &["type", "minLength"]),
];
