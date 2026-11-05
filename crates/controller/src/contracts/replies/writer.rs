//! Writer reply wire contract (#730 C3).
use super::wire::*;

wire_text!(WireText, 1, UNBOUNDED, "\\S", "");

wire_link!(WireOptionAction, super::writer::WireText, "#/$defs/text");

wire_object!(WireOption,["action","outcome"],["type","properties","required","additionalProperties"],vec![],{
action: WireOptionAction, ["$ref"], vec![];
outcome: WireOptionAction, ["$ref"], vec![];
});

wire_link!(OptionsItem, super::writer::WireOption, "#/$defs/option");

pub type Options = List<OptionsItem, 2, 3, false>;

wire_link!(QuestionObstacle, super::writer::WireText, "#/$defs/text");

wire_link!(QuestionOptions, super::writer::Options, "#/$defs/options");

wire_link!(QuestionDecision, super::writer::WireText, "#/$defs/text");

wire_object!(Question,["request_quote","obstacle","options","recommended_option","recommendation_reason","how_to_answer"],["type","properties","required","additionalProperties"],vec![],{
request_quote: WireOptionAction, ["$ref"], vec![];
obstacle: Limited<QuestionObstacle,600>, ["$ref","maxLength"], vec![("maxLength",Doc::atom(600))];
options: QuestionOptions, ["$ref"], vec![];
recommended_option: Whole<0,{ u64::MAX }>, ["type","minimum"], vec![];
recommendation_reason: WireOptionAction, ["$ref"], vec![];
how_to_answer: WireOptionAction, ["$ref"], vec![];
#[serde(default,deserialize_with="present",skip_serializing_if="Option::is_none")]
decision: Option<Limited<QuestionDecision,400>>, ["$ref","maxLength"], vec![("maxLength",Doc::atom(400))];
});

wire_link!(
    WriterReplyVariant1,
    super::writer::Question,
    "#/$defs/question"
);

wire_object!(WriterReplyVariant2,["text"],["type","properties","required","additionalProperties"],vec![],{
text: WireOptionAction, ["$ref"], vec![];
});

wire_union!(WriterReply, {Variant1(WriterReplyVariant1),Variant2(WriterReplyVariant2)});

pub fn document() -> Doc {
    let mut s = WriterReply::wire_schema();
    s.set(
        "$schema",
        Doc::atom("https://json-schema.org/draft/2020-12/schema"),
    );
    s.set(
        "$id",
        Doc::atom("https://trustladderiii.invalid/workers/protocol-1/writer-reply.json"),
    );
    s.set(
        "$defs",
        Doc::object(vec![
            ("text", <WireText>::wire_schema()),
            ("option", <WireOption>::wire_schema()),
            ("options", <List<OptionsItem, 2, 3, false>>::wire_schema()),
            ("question", <Question>::wire_schema()),
        ]),
    );
    s.ordered(&["$schema", "$id", "oneOf", "$defs"])
}
