//! Reviewer reply wire contract (#730 C3).
use super::wire::*;

wire_object!(Finding,["problem","evidence"],["type","properties","required","additionalProperties"],vec![],{
problem: Text<1,UNBOUNDED>, ["type","minLength"], vec![];
evidence: Text<1,UNBOUNDED>, ["type","minLength"], vec![];
});

wire_choice!(BlockingFindingCitationKind, "enum", {Goal => "goal",StandingRule => "standing_rule"});

wire_object!(BlockingFindingCitation,["kind","id"],["type","properties","required","additionalProperties"],vec![],{
kind: BlockingFindingCitationKind, ["enum"], vec![];
id: Text<1,UNBOUNDED>, ["type","minLength"], vec![];
});

wire_object!(BlockingFinding,["citation","problem","evidence"],["type","properties","required","additionalProperties"],vec![],{
citation: BlockingFindingCitation, ["type","properties","required","additionalProperties"], vec![];
problem: Text<1,UNBOUNDED>, ["type","minLength"], vec![];
evidence: Text<1,UNBOUNDED>, ["type","minLength"], vec![];
});

wire_text!(
    ReviewerReplyApproveHeadSha,
    0,
    UNBOUNDED,
    "^(?:[0-9a-f]{40}|[0-9a-f]{64})$",
    ""
);

wire_link!(
    ReviewerReplyApproveBlockingFindingsItem,
    super::reviewer::BlockingFinding,
    "#/$defs/blocking_finding"
);

wire_link!(
    ReviewerReplyApproveFindingsItem,
    super::reviewer::Finding,
    "#/$defs/finding"
);

wire_object!(ReviewerReplyApprove,["head_sha","blocking_findings","findings"],["type","properties","required","additionalProperties"],vec![],{
head_sha: ReviewerReplyApproveHeadSha, ["type","pattern"], vec![];
blocking_findings: List<ReviewerReplyApproveBlockingFindingsItem,0,0,false>, ["type","items","maxItems"], vec![];
findings: List<ReviewerReplyApproveFindingsItem,0,UNBOUNDED,false>, ["type","items"], vec![];
});

wire_object!(ReviewerReplyChanges,["blocking_findings","findings"],["type","properties","required","additionalProperties"],vec![],{
blocking_findings: List<ReviewerReplyApproveBlockingFindingsItem,1,20,false>, ["type","items","minItems","maxItems"], vec![];
findings: List<ReviewerReplyApproveFindingsItem,0,UNBOUNDED,false>, ["type","items"], vec![];
});

wire_object!(
    ReviewerReplyOutOfRoom,
    [],
    ["type", "properties", "required", "additionalProperties"],
    vec![],
    {}
);

wire_tagged!(ReviewerReply, "verdict", {Approve(ReviewerReplyApprove) => ("approve",0,0),Changes(ReviewerReplyChanges) => ("changes",0,0),OutOfRoom(ReviewerReplyOutOfRoom) => ("out_of_room",0,0)});

pub fn document() -> Doc {
    let mut s = ReviewerReply::wire_schema();
    s.set(
        "$schema",
        Doc::atom("https://json-schema.org/draft/2020-12/schema"),
    );
    s.set(
        "$id",
        Doc::atom("https://trustladderiii.invalid/workers/protocol-1/reviewer-reply.json"),
    );
    s.set(
        "$defs",
        Doc::object(vec![
            ("finding", <Finding>::wire_schema()),
            ("blocking_finding", <BlockingFinding>::wire_schema()),
        ]),
    );
    s.ordered(&["$schema", "$id", "oneOf", "$defs"])
}
