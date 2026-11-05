//! Postmortem reply wire contract (#730 C3).
use super::wire::*;

pub type WireText = Text<1, 2000>;

wire_choice!(PostmortemCause, "enum", {Tl3 => "tl3",Outside => "outside"});

wire_object!(PostmortemTimelineItem,["at","event"],["type","properties","required","additionalProperties"],vec![],{
at: Text<0,64>, ["type","maxLength"], vec![];
event: Text<1,1000>, ["type","minLength","maxLength"], vec![];
});

wire_choice!(PostmortemProcessFailuresItemKind, "enum", {PromptOrSkillDefect => "prompt or skill defect",CodeDefect => "code defect",ToolFailure => "tool failure",MissingEvidence => "missing evidence"});

wire_link!(
    PostmortemProcessFailuresItemWhat,
    super::postmortem::WireText,
    "#/$defs/text"
);

wire_object!(PostmortemProcessFailuresItem,["phase","kind","what","evidence"],["type","properties","required","additionalProperties"],vec![],{
phase: Text<1,200>, ["type","minLength","maxLength"], vec![];
kind: PostmortemProcessFailuresItemKind, ["enum"], vec![];
what: PostmortemProcessFailuresItemWhat, ["$ref"], vec![];
evidence: Text<0,2000>, ["type","maxLength"], vec![];
});

wire_link!(
    PostmortemRecommendationsItem,
    super::postmortem::Recommendation,
    "#/$defs/recommendation"
);

wire_object!(Postmortem,["summary","cause","went_wrong","timeline","process_failures","recommendations","notes","lessons"],["type","properties","required","additionalProperties"],vec![],{
summary: Text<1,4000>, ["type","minLength","maxLength"], vec![];
cause: PostmortemCause, ["enum"], vec![];
went_wrong: Text<0,8000>, ["type","maxLength"], vec![];
timeline: List<PostmortemTimelineItem,0,60,false>, ["type","maxItems","items"], vec![];
process_failures: List<PostmortemProcessFailuresItem,0,20,false>, ["type","maxItems","items"], vec![];
recommendations: List<PostmortemRecommendationsItem,0,3,false>, ["type","maxItems","items"], vec![];
notes: Text<0,2000>, ["type","maxLength"], vec![];
lessons: List<Text<1,1000>,0,5,false>, ["type","maxItems","items"], vec![];
});

wire_choice!(RecommendationKind, "enum", {Prompt => "prompt",Code => "code",Config => "config",Rule => "rule"});

wire_choice!(RecommendationOperation, "enum", {Add => "add",Remove => "remove",Replace => "replace"});

checked_wire_object!(Recommendation,["phase","kind","target","operation","before","after","evidence","effect"],["type","properties","required","additionalProperties","allOf"],vec![("allOf",Doc::Array(vec![Doc::object(vec![("if",Doc::object(vec![("properties",Doc::object(vec![("operation",Doc::object(vec![("const",Doc::atom("add"))]))]))])),("then",Doc::object(vec![("properties",Doc::object(vec![("before",Doc::object(vec![("maxLength",Doc::atom(0))])),("after",Doc::object(vec![("minLength",Doc::atom(1))]))]))]))]),Doc::object(vec![("if",Doc::object(vec![("properties",Doc::object(vec![("operation",Doc::object(vec![("const",Doc::atom("remove"))]))]))])),("then",Doc::object(vec![("properties",Doc::object(vec![("before",Doc::object(vec![("minLength",Doc::atom(1))])),("after",Doc::object(vec![("maxLength",Doc::atom(0))]))]))]))]),Doc::object(vec![("if",Doc::object(vec![("properties",Doc::object(vec![("operation",Doc::object(vec![("const",Doc::atom("replace"))]))]))])),("then",Doc::object(vec![("properties",Doc::object(vec![("before",Doc::object(vec![("minLength",Doc::atom(1))])),("after",Doc::object(vec![("minLength",Doc::atom(1))]))]))]))])]))],{
phase: Text<1,200>, ["type","minLength","maxLength"], vec![];
kind: RecommendationKind, ["enum"], vec![];
target: Text<1,500>, ["type","minLength","maxLength"], vec![];
operation: RecommendationOperation, ["enum"], vec![];
before: Text<0,8000>, ["type","maxLength"], vec![];
after: Text<0,8000>, ["type","maxLength"], vec![];
evidence: PostmortemProcessFailuresItemWhat, ["$ref"], vec![];
effect: PostmortemProcessFailuresItemWhat, ["$ref"], vec![];
}, |item:&Self| match item.operation {
    RecommendationOperation::Add => item.before.as_str().is_empty() && !item.after.as_str().is_empty(),
    RecommendationOperation::Remove => !item.before.as_str().is_empty() && item.after.as_str().is_empty(),
    RecommendationOperation::Replace => !item.before.as_str().is_empty() && !item.after.as_str().is_empty(),
});

wire_link!(
    PostmortemReplyPostmortem,
    super::postmortem::Postmortem,
    "#/$defs/postmortem"
);

wire_object!(PostmortemReplyGradesItem,["dimension","grade","rationale","judge_model","assessment_id"],["type","properties","required","additionalProperties"],vec![],{
dimension: Text<1,100>, ["type","minLength","maxLength"], vec![];
grade: Maybe<Whole<1,5>>, ["type","minimum","maximum"], vec![];
rationale: Text<0,4000>, ["type","maxLength"], vec![];
judge_model: Text<1,200>, ["type","minLength","maxLength"], vec![];
assessment_id: Maybe<Text<0,200>>, ["type","maxLength"], vec![];
});

wire_object!(PostmortemReplyUsage,["traces","input_tokens","output_tokens","cache_read_input_tokens","cache_creation_input_tokens"],["type","properties","required","additionalProperties"],vec![],{
traces: Whole<0,{ u64::MAX }>, ["type","minimum"], vec![];
input_tokens: Whole<0,{ u64::MAX }>, ["type","minimum"], vec![];
output_tokens: Whole<0,{ u64::MAX }>, ["type","minimum"], vec![];
cache_read_input_tokens: Whole<0,{ u64::MAX }>, ["type","minimum"], vec![];
cache_creation_input_tokens: Whole<0,{ u64::MAX }>, ["type","minimum"], vec![];
});

wire_object!(PostmortemReplyModels,["postmortem","grades"],["type","properties","required","additionalProperties"],vec![],{
postmortem: Text<1,200>, ["type","minLength","maxLength"], vec![];
grades: Text<1,200>, ["type","minLength","maxLength"], vec![];
});

wire_object!(PostmortemReply,["postmortem","grades","assessment_ids","usage","models"],["type","properties","required","additionalProperties"],vec![],{
postmortem: PostmortemReplyPostmortem, ["$ref"], vec![];
grades: List<PostmortemReplyGradesItem,0,20,false>, ["type","maxItems","items"], vec![];
assessment_ids: List<Text<1,200>,0,20,false>, ["type","maxItems","items"], vec![];
usage: PostmortemReplyUsage, ["type","properties","required","additionalProperties"], vec![];
models: PostmortemReplyModels, ["type","properties","required","additionalProperties"], vec![];
});

pub fn document() -> Doc {
    let mut s = PostmortemReply::wire_schema();
    s.set(
        "$schema",
        Doc::atom("https://json-schema.org/draft/2020-12/schema"),
    );
    s.set(
        "$id",
        Doc::atom("https://trustladderiii.invalid/workers/protocol-1/postmortem-reply.json"),
    );
    s.set("description",Doc::atom("A judged postmortem (#403). The judge writes `postmortem`; worker code adds the grades, assessment ids, usage and models."));
    s.set(
        "$defs",
        Doc::object(vec![
            ("text", <Text<1, 2000>>::wire_schema()),
            ("postmortem", <Postmortem>::wire_schema()),
            ("recommendation", <Recommendation>::wire_schema()),
        ]),
    );
    s.ordered(&[
        "$schema",
        "$id",
        "description",
        "type",
        "properties",
        "required",
        "additionalProperties",
        "$defs",
    ])
}
