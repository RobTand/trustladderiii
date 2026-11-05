use crate::contracts::bounded::*;
use crate::contracts::render::{Layout, TypeOrder, TypeUse};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Text(pub ReplyText<0, 1, UNBOUNDED>);
impl JsonSchema for Text {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "text".into()
    }
    fn json_schema(g: &mut schemars::SchemaGenerator) -> schemars::Schema {
        <ReplyText<0, 1, UNBOUNDED>>::json_schema(g)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(rename = "option")]
pub struct QuestionOption {
    pub action: Text,
    pub outcome: Text,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Options(pub List<QuestionOption, 2, 3, false>);
impl JsonSchema for Options {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "options".into()
    }
    fn json_schema(g: &mut schemars::SchemaGenerator) -> schemars::Schema {
        <List<QuestionOption, 2, 3, false>>::json_schema(g)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(rename = "question")]
pub struct Question {
    pub request_quote: Text,
    pub obstacle: ReplyMaxText<Text, 600>,
    pub options: Options,
    pub recommended_option: ReplyNumber<0, 0, true, false, true>,
    pub recommendation_reason: Text,
    pub how_to_answer: Text,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "ReplyMaxText<Text, 400>", default)]
    pub decision: Option<ReplyMaxText<Text, 400>>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(inline)]
pub struct WriterReplyValue2 {
    pub text: Text,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
#[schemars(inline)]
#[schemars(transform = reply_one_of)]
pub enum WriterReply {
    Question(Question),
    Value2(WriterReplyValue2),
}

pub fn schema_value() -> serde_json::Value {
    reply_schema_value::<WriterReply>("writer", |g| {
        g.subschema_for::<Text>();
        g.subschema_for::<QuestionOption>();
        g.subschema_for::<Options>();
        g.subschema_for::<Question>();
    })
}

pub fn schema_text() -> String {
    let types = [
        TypeOrder::of::<Text>(TEXT_ORDER),
        TypeOrder::of::<QuestionOption>(OPTION_ORDER),
        TypeOrder::of::<Options>(OPTIONS_ORDER),
        TypeOrder::of::<Question>(QUESTION_ORDER),
    ];
    let shared = [
        TypeUse {
            pointer: "/$defs/text",
            order: &types[0],
        },
        TypeUse {
            pointer: "/$defs/option",
            order: &types[1],
        },
        TypeUse {
            pointer: "/$defs/options",
            order: &types[2],
        },
        TypeUse {
            pointer: "/$defs/question",
            order: &types[3],
        },
    ];
    crate::contracts::render::render(
        "writer-reply.json",
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
    ("/oneOf/0", &["$ref"]),
    (
        "/oneOf/1",
        &["type", "properties", "required", "additionalProperties"],
    ),
    ("/oneOf/1/properties", &["text"]),
    ("/oneOf/1/properties/text", &["$ref"]),
    ("/$defs", &["text", "option", "options", "question"]),
];
pub const TEXT_ORDER: &[crate::contracts::render::ObjectOrder<'static>] =
    &[("", &["type", "minLength", "pattern"])];
pub const OPTION_ORDER: &[crate::contracts::render::ObjectOrder<'static>] = &[
    (
        "",
        &["type", "properties", "required", "additionalProperties"],
    ),
    ("/properties", &["action", "outcome"]),
    ("/properties/action", &["$ref"]),
    ("/properties/outcome", &["$ref"]),
];
pub const OPTIONS_ORDER: &[crate::contracts::render::ObjectOrder<'static>] = &[
    ("", &["type", "items", "minItems", "maxItems"]),
    ("/items", &["$ref"]),
];
pub const QUESTION_ORDER: &[crate::contracts::render::ObjectOrder<'static>] = &[
    (
        "",
        &["type", "properties", "required", "additionalProperties"],
    ),
    (
        "/properties",
        &[
            "request_quote",
            "obstacle",
            "options",
            "recommended_option",
            "recommendation_reason",
            "how_to_answer",
            "decision",
        ],
    ),
    ("/properties/request_quote", &["$ref"]),
    ("/properties/obstacle", &["$ref", "maxLength"]),
    ("/properties/options", &["$ref"]),
    ("/properties/recommended_option", &["type", "minimum"]),
    ("/properties/recommendation_reason", &["$ref"]),
    ("/properties/how_to_answer", &["$ref"]),
    ("/properties/decision", &["$ref", "maxLength"]),
];
