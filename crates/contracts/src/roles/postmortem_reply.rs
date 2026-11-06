use crate::bounded::{Text as BoundedText, *};
use crate::render::{Layout, TypeOrder, TypeUse};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Text(pub BoundedText<1, 2000>);
impl JsonSchema for Text {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "text".into()
    }
    fn json_schema(g: &mut schemars::SchemaGenerator) -> schemars::Schema {
        <BoundedText<1, 2000>>::json_schema(g)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(inline)]
#[schemars(transform = reply_enum_schema)]
pub enum PostmortemCause {
    #[serde(rename = "tl3")]
    Tl3,
    #[serde(rename = "outside")]
    Outside,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(inline)]
pub struct PostmortemTimelineItem {
    pub at: BoundedText<0, 64>,
    pub event: BoundedText<1, 1000>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(inline)]
#[schemars(transform = reply_enum_schema)]
pub enum PostmortemProcessFailuresItemKind {
    #[serde(rename = "prompt or skill defect")]
    PromptOrSkillDefect,
    #[serde(rename = "code defect")]
    CodeDefect,
    #[serde(rename = "tool failure")]
    ToolFailure,
    #[serde(rename = "missing evidence")]
    MissingEvidence,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(inline)]
pub struct PostmortemProcessFailuresItem {
    pub phase: BoundedText<1, 200>,
    pub kind: PostmortemProcessFailuresItemKind,
    pub what: Text,
    pub evidence: BoundedText<0, 2000>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(rename = "postmortem")]
pub struct Postmortem {
    pub summary: BoundedText<1, 4000>,
    pub cause: PostmortemCause,
    pub went_wrong: BoundedText<0, 8000>,
    pub timeline: List<PostmortemTimelineItem, 0, 60, false>,
    pub process_failures: List<PostmortemProcessFailuresItem, 0, 20, false>,
    pub recommendations: List<Recommendation, 0, 3, false>,
    pub notes: BoundedText<0, 2000>,
    pub lessons: List<BoundedText<1, 1000>, 0, 5, false>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(inline)]
#[schemars(transform = reply_enum_schema)]
pub enum RecommendationKind {
    #[serde(rename = "prompt")]
    Prompt,
    #[serde(rename = "code")]
    Code,
    #[serde(rename = "config")]
    Config,
    #[serde(rename = "rule")]
    Rule,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(inline)]
#[schemars(transform = reply_enum_schema)]
pub enum RecommendationOperation {
    #[serde(rename = "add")]
    Add,
    #[serde(rename = "remove")]
    Remove,
    #[serde(rename = "replace")]
    Replace,
}

#[derive(Clone, Debug, PartialEq, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(rename = "recommendation")]
#[schemars(extend("allOf" = [{"if": {"properties": {"operation": {"const": "add"}}}, "then": {"properties": {"before": {"maxLength": 0}, "after": {"minLength": 1}}}}, {"if": {"properties": {"operation": {"const": "remove"}}}, "then": {"properties": {"before": {"minLength": 1}, "after": {"maxLength": 0}}}}, {"if": {"properties": {"operation": {"const": "replace"}}}, "then": {"properties": {"before": {"minLength": 1}, "after": {"minLength": 1}}}}]))]
pub struct Recommendation {
    phase: BoundedText<1, 200>,
    kind: RecommendationKind,
    target: BoundedText<1, 500>,
    operation: RecommendationOperation,
    before: BoundedText<0, 8000>,
    after: BoundedText<0, 8000>,
    evidence: Text,
    effect: Text,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RecommendationWire {
    phase: BoundedText<1, 200>,
    kind: RecommendationKind,
    target: BoundedText<1, 500>,
    operation: RecommendationOperation,
    before: BoundedText<0, 8000>,
    after: BoundedText<0, 8000>,
    evidence: Text,
    effect: Text,
}

impl<'de> Deserialize<'de> for Recommendation {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = RecommendationWire::deserialize(deserializer)?;
        let valid = match value.operation {
            RecommendationOperation::Add => {
                value.before.as_str().is_empty() && !value.after.as_str().is_empty()
            }
            RecommendationOperation::Remove => {
                !value.before.as_str().is_empty() && value.after.as_str().is_empty()
            }
            RecommendationOperation::Replace => {
                !value.before.as_str().is_empty() && !value.after.as_str().is_empty()
            }
        };
        if !valid {
            return Err(serde::de::Error::custom(
                "recommendation text does not match its operation",
            ));
        }
        Ok(Self {
            phase: value.phase,
            kind: value.kind,
            target: value.target,
            operation: value.operation,
            before: value.before,
            after: value.after,
            evidence: value.evidence,
            effect: value.effect,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(inline)]
pub struct PostmortemReplyGradesItem {
    pub dimension: BoundedText<1, 100>,
    pub grade: ReplyNullable<ReplyNumber<1, 5, true, true, true>>,
    pub rationale: BoundedText<0, 4000>,
    pub judge_model: BoundedText<1, 200>,
    pub assessment_id: ReplyNullable<BoundedText<0, 200>>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(inline)]
pub struct PostmortemReplyUsage {
    pub traces: ReplyNumber<0, 0, true, false, true>,
    pub input_tokens: ReplyNumber<0, 0, true, false, true>,
    pub output_tokens: ReplyNumber<0, 0, true, false, true>,
    pub cache_read_input_tokens: ReplyNumber<0, 0, true, false, true>,
    pub cache_creation_input_tokens: ReplyNumber<0, 0, true, false, true>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(inline)]
pub struct PostmortemReplyModels {
    pub postmortem: BoundedText<1, 200>,
    pub grades: BoundedText<1, 200>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(inline)]
#[schemars(
    description = "A judged postmortem (#403). The judge writes `postmortem`; worker code adds the grades, assessment ids, usage and models."
)]
pub struct PostmortemReply {
    pub postmortem: Postmortem,
    pub grades: List<PostmortemReplyGradesItem, 0, 20, false>,
    pub assessment_ids: List<BoundedText<1, 200>, 0, 20, false>,
    pub usage: PostmortemReplyUsage,
    pub models: PostmortemReplyModels,
}

pub fn schema_value() -> serde_json::Value {
    reply_schema_value::<PostmortemReply>("postmortem", |g| {
        g.subschema_for::<Text>();
        g.subschema_for::<Postmortem>();
        g.subschema_for::<Recommendation>();
    })
}

pub fn schema_text() -> String {
    let types = [
        TypeOrder::of::<Text>(TEXT_ORDER),
        TypeOrder::of::<Postmortem>(POSTMORTEM_ORDER),
        TypeOrder::of::<Recommendation>(RECOMMENDATION_ORDER),
    ];
    let shared = [
        TypeUse {
            pointer: "/$defs/text",
            order: &types[0],
        },
        TypeUse {
            pointer: "/$defs/postmortem",
            order: &types[1],
        },
        TypeUse {
            pointer: "/$defs/recommendation",
            order: &types[2],
        },
    ];
    crate::render::render(
        "postmortem-reply.json",
        &schema_value(),
        &Layout {
            inline: ROOT_ORDER,
            shared: &shared,
        },
    )
    .expect("published reply schema order is exhaustive")
}

pub const ROOT_ORDER: &[crate::render::ObjectOrder<'static>] = &[
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
            "$defs",
        ],
    ),
    (
        "/properties",
        &["postmortem", "grades", "assessment_ids", "usage", "models"],
    ),
    ("/properties/postmortem", &["$ref"]),
    ("/properties/grades", &["type", "maxItems", "items"]),
    (
        "/properties/grades/items",
        &["type", "properties", "required", "additionalProperties"],
    ),
    (
        "/properties/grades/items/properties",
        &[
            "dimension",
            "grade",
            "rationale",
            "judge_model",
            "assessment_id",
        ],
    ),
    (
        "/properties/grades/items/properties/dimension",
        &["type", "minLength", "maxLength"],
    ),
    (
        "/properties/grades/items/properties/grade",
        &["type", "minimum", "maximum"],
    ),
    (
        "/properties/grades/items/properties/rationale",
        &["type", "maxLength"],
    ),
    (
        "/properties/grades/items/properties/judge_model",
        &["type", "minLength", "maxLength"],
    ),
    (
        "/properties/grades/items/properties/assessment_id",
        &["type", "maxLength"],
    ),
    ("/properties/assessment_ids", &["type", "maxItems", "items"]),
    (
        "/properties/assessment_ids/items",
        &["type", "minLength", "maxLength"],
    ),
    (
        "/properties/usage",
        &["type", "properties", "required", "additionalProperties"],
    ),
    (
        "/properties/usage/properties",
        &[
            "traces",
            "input_tokens",
            "output_tokens",
            "cache_read_input_tokens",
            "cache_creation_input_tokens",
        ],
    ),
    ("/properties/usage/properties/traces", &["type", "minimum"]),
    (
        "/properties/usage/properties/input_tokens",
        &["type", "minimum"],
    ),
    (
        "/properties/usage/properties/output_tokens",
        &["type", "minimum"],
    ),
    (
        "/properties/usage/properties/cache_read_input_tokens",
        &["type", "minimum"],
    ),
    (
        "/properties/usage/properties/cache_creation_input_tokens",
        &["type", "minimum"],
    ),
    (
        "/properties/models",
        &["type", "properties", "required", "additionalProperties"],
    ),
    ("/properties/models/properties", &["postmortem", "grades"]),
    (
        "/properties/models/properties/postmortem",
        &["type", "minLength", "maxLength"],
    ),
    (
        "/properties/models/properties/grades",
        &["type", "minLength", "maxLength"],
    ),
    ("/$defs", &["text", "postmortem", "recommendation"]),
];
pub const TEXT_ORDER: &[crate::render::ObjectOrder<'static>] =
    &[("", &["type", "minLength", "maxLength"])];
pub const POSTMORTEM_ORDER: &[crate::render::ObjectOrder<'static>] = &[
    (
        "",
        &["type", "properties", "required", "additionalProperties"],
    ),
    (
        "/properties",
        &[
            "summary",
            "cause",
            "went_wrong",
            "timeline",
            "process_failures",
            "recommendations",
            "notes",
            "lessons",
        ],
    ),
    ("/properties/summary", &["type", "minLength", "maxLength"]),
    ("/properties/cause", &["enum"]),
    ("/properties/went_wrong", &["type", "maxLength"]),
    ("/properties/timeline", &["type", "maxItems", "items"]),
    (
        "/properties/timeline/items",
        &["type", "properties", "required", "additionalProperties"],
    ),
    ("/properties/timeline/items/properties", &["at", "event"]),
    (
        "/properties/timeline/items/properties/at",
        &["type", "maxLength"],
    ),
    (
        "/properties/timeline/items/properties/event",
        &["type", "minLength", "maxLength"],
    ),
    (
        "/properties/process_failures",
        &["type", "maxItems", "items"],
    ),
    (
        "/properties/process_failures/items",
        &["type", "properties", "required", "additionalProperties"],
    ),
    (
        "/properties/process_failures/items/properties",
        &["phase", "kind", "what", "evidence"],
    ),
    (
        "/properties/process_failures/items/properties/phase",
        &["type", "minLength", "maxLength"],
    ),
    (
        "/properties/process_failures/items/properties/kind",
        &["enum"],
    ),
    (
        "/properties/process_failures/items/properties/what",
        &["$ref"],
    ),
    (
        "/properties/process_failures/items/properties/evidence",
        &["type", "maxLength"],
    ),
    (
        "/properties/recommendations",
        &["type", "maxItems", "items"],
    ),
    ("/properties/recommendations/items", &["$ref"]),
    ("/properties/notes", &["type", "maxLength"]),
    ("/properties/lessons", &["type", "maxItems", "items"]),
    (
        "/properties/lessons/items",
        &["type", "minLength", "maxLength"],
    ),
];
pub const RECOMMENDATION_ORDER: &[crate::render::ObjectOrder<'static>] = &[
    (
        "",
        &[
            "type",
            "properties",
            "required",
            "additionalProperties",
            "allOf",
        ],
    ),
    (
        "/properties",
        &[
            "phase",
            "kind",
            "target",
            "operation",
            "before",
            "after",
            "evidence",
            "effect",
        ],
    ),
    ("/properties/phase", &["type", "minLength", "maxLength"]),
    ("/properties/kind", &["enum"]),
    ("/properties/target", &["type", "minLength", "maxLength"]),
    ("/properties/operation", &["enum"]),
    ("/properties/before", &["type", "maxLength"]),
    ("/properties/after", &["type", "maxLength"]),
    ("/properties/evidence", &["$ref"]),
    ("/properties/effect", &["$ref"]),
    ("/allOf/0", &["if", "then"]),
    ("/allOf/0/if", &["properties"]),
    ("/allOf/0/if/properties", &["operation"]),
    ("/allOf/0/if/properties/operation", &["const"]),
    ("/allOf/0/then", &["properties"]),
    ("/allOf/0/then/properties", &["before", "after"]),
    ("/allOf/0/then/properties/before", &["maxLength"]),
    ("/allOf/0/then/properties/after", &["minLength"]),
    ("/allOf/1", &["if", "then"]),
    ("/allOf/1/if", &["properties"]),
    ("/allOf/1/if/properties", &["operation"]),
    ("/allOf/1/if/properties/operation", &["const"]),
    ("/allOf/1/then", &["properties"]),
    ("/allOf/1/then/properties", &["before", "after"]),
    ("/allOf/1/then/properties/before", &["minLength"]),
    ("/allOf/1/then/properties/after", &["maxLength"]),
    ("/allOf/2", &["if", "then"]),
    ("/allOf/2/if", &["properties"]),
    ("/allOf/2/if/properties", &["operation"]),
    ("/allOf/2/if/properties/operation", &["const"]),
    ("/allOf/2/then", &["properties"]),
    ("/allOf/2/then/properties", &["before", "after"]),
    ("/allOf/2/then/properties/before", &["minLength"]),
    ("/allOf/2/then/properties/after", &["minLength"]),
];
