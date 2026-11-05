//! Classifier reply wire contract (#730 C3).
use super::wire::*;

wire_object!(Decision,["allowed_scope","restrictions"],["type","properties","required","additionalProperties"],vec![],{
allowed_scope: List<Text<1,UNBOUNDED>,0,UNBOUNDED,true>, ["type","items","uniqueItems"], vec![];
restrictions: List<Text<1,UNBOUNDED>,0,UNBOUNDED,true>, ["type","items","uniqueItems"], vec![];
});

wire_choice!(ClassifierReplyVariant1Kind, "const", { Value => "change_request" });

wire_object!(ClassifierReplyVariant1,["kind","new_text"],["type","properties","required","additionalProperties"],vec![],{
kind: ClassifierReplyVariant1Kind, ["const"], vec![];
new_text: Text<1,UNBOUNDED>, ["type","minLength"], vec![];
#[serde(default,deserialize_with="present",skip_serializing_if="Option::is_none")]
supersedes: Option<List<Text<1,UNBOUNDED>,0,UNBOUNDED,true>>, ["type","items","uniqueItems"], vec![];
});

wire_choice!(ClassifierReplyVariant2Kind, "enum", {Stop => "stop",Restart => "restart",Pause => "pause",Resume => "resume",Remark => "remark"});

wire_object!(ClassifierReplyVariant2,["kind"],["type","properties","required","additionalProperties"],vec![],{
kind: ClassifierReplyVariant2Kind, ["enum"], vec![];
});

wire_choice!(ClassifierReplyVariant3Kind, "const", { Value => "unsure" });

wire_text!(
    ClassifierReplyVariant3Question,
    1,
    300,
    "\\S",
    "[\\r\\n\\u0085\\u2028\\u2029]"
);

wire_object!(ClassifierReplyVariant3,["kind","question"],["type","properties","required","additionalProperties"],vec![],{
kind: ClassifierReplyVariant3Kind, ["const"], vec![];
question: ClassifierReplyVariant3Question, ["type","minLength","maxLength","pattern","not"], vec![];
});

wire_choice!(ClassifierReplyVariant4Kind, "const", { Value => "complaint" });

wire_text!(
    ClassifierReplyVariant4Summary,
    1,
    UNBOUNDED,
    "\\S",
    "[\\r\\n\\u0085\\u2028\\u2029]"
);

wire_object!(ClassifierReplyVariant4,["kind","summary","revert"],["type","properties","required","additionalProperties"],vec![],{
kind: ClassifierReplyVariant4Kind, ["const"], vec![];
summary: ClassifierReplyVariant4Summary, ["type","minLength","pattern","not"], vec![];
revert: bool, ["type"], vec![];
});

wire_choice!(ClassifierReplyVariant5Kind, "const", { Value => "answer" });

wire_choice!(ClassifierReplyVariant5Answer, "enum", {Continue => "continue",Stop => "stop",Retry => "retry",MarkHandled => "mark_handled",Unclear => "unclear"});

wire_link!(
    ClassifierReplyVariant5Decision,
    super::classifier::Decision,
    "#/$defs/decision"
);

wire_object!(ClassifierReplyVariant5,["kind","question_id","answer"],["type","properties","required","additionalProperties"],vec![],{
kind: ClassifierReplyVariant5Kind, ["const"], vec![];
question_id: Text<1,UNBOUNDED>, ["type","minLength"], vec![];
answer: ClassifierReplyVariant5Answer, ["enum"], vec![];
#[serde(default,deserialize_with="present",skip_serializing_if="Option::is_none")]
decision: Option<ClassifierReplyVariant5Decision>, ["$ref"], vec![];
});

wire_object!(ClassifierReplyVariant6,["kind","question_id","answer","new_text"],["type","properties","required","additionalProperties"],vec![],{
kind: ClassifierReplyVariant5Kind, ["const"], vec![];
question_id: Text<1,UNBOUNDED>, ["type","minLength"], vec![];
answer: ClassifierReplyVariant1Kind, ["const"], vec![];
new_text: Text<1,UNBOUNDED>, ["type","minLength"], vec![];
#[serde(default,deserialize_with="present",skip_serializing_if="Option::is_none")]
decision: Option<ClassifierReplyVariant5Decision>, ["$ref"], vec![];
#[serde(default,deserialize_with="present",skip_serializing_if="Option::is_none")]
supersedes: Option<List<Text<1,UNBOUNDED>,0,UNBOUNDED,true>>, ["type","items","uniqueItems"], vec![];
});

wire_choice!(ClassifierReplyVariant7Answer, "const", { Value => "try_again" });

wire_text!(ClassifierReplyVariant7Guidance, 1, UNBOUNDED, "\\S", "");

wire_object!(ClassifierReplyVariant7,["kind","question_id","answer","guidance"],["type","properties","required","additionalProperties"],vec![],{
kind: ClassifierReplyVariant5Kind, ["const"], vec![];
question_id: Text<1,UNBOUNDED>, ["type","minLength"], vec![];
answer: ClassifierReplyVariant7Answer, ["const"], vec![];
guidance: ClassifierReplyVariant7Guidance, ["type","minLength","pattern"], vec![];
#[serde(default,deserialize_with="present",skip_serializing_if="Option::is_none")]
decision: Option<ClassifierReplyVariant5Decision>, ["$ref"], vec![];
});

wire_choice!(ClassifierReplyVariant8Answer, "const", { Value => "add_credit" });

wire_object!(ClassifierReplyVariant8,["kind","question_id","answer","amount"],["type","properties","required","additionalProperties"],vec![],{
kind: ClassifierReplyVariant5Kind, ["const"], vec![];
question_id: Text<1,UNBOUNDED>, ["type","minLength"], vec![];
answer: ClassifierReplyVariant8Answer, ["const"], vec![];
amount: Whole<1,100>, ["type","minimum","maximum"], vec![];
#[serde(default,deserialize_with="present",skip_serializing_if="Option::is_none")]
decision: Option<ClassifierReplyVariant5Decision>, ["$ref"], vec![];
});

wire_choice!(ClassifierReplyVariant9Kind, "const", { Value => "reconcile_decisions" });

wire_object!(ClassifierReplyVariant9AssessmentsItem,["id","contradicted","reason"],["type","properties","required","additionalProperties"],vec![],{
id: Text<1,UNBOUNDED>, ["type","minLength"], vec![];
contradicted: bool, ["type"], vec![];
reason: Text<1,2000>, ["type","minLength","maxLength"], vec![];
});

wire_object!(ClassifierReplyVariant9,["kind","assessments"],["type","properties","required","additionalProperties"],vec![],{
kind: ClassifierReplyVariant9Kind, ["const"], vec![];
assessments: List<ClassifierReplyVariant9AssessmentsItem,0,64,false>, ["type","maxItems","items"], vec![];
});

wire_union!(ClassifierReply, {Variant1(ClassifierReplyVariant1),Variant2(ClassifierReplyVariant2),Variant3(ClassifierReplyVariant3),Variant4(ClassifierReplyVariant4),Variant5(ClassifierReplyVariant5),Variant6(ClassifierReplyVariant6),Variant7(ClassifierReplyVariant7),Variant8(ClassifierReplyVariant8),Variant9(ClassifierReplyVariant9)});

pub fn document() -> Doc {
    let mut s = ClassifierReply::wire_schema();
    s.set(
        "$schema",
        Doc::atom("https://json-schema.org/draft/2020-12/schema"),
    );
    s.set(
        "$id",
        Doc::atom("https://trustladderiii.invalid/workers/protocol-1/classifier-reply.json"),
    );
    s.set(
        "$defs",
        Doc::object(vec![("decision", <Decision>::wire_schema())]),
    );
    s.ordered(&["$schema", "$id", "oneOf", "$defs"])
}
