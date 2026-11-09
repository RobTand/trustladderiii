use crate::bounded::{Text as BoundedText, *};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(rename = "execution")]
#[schemars(
    description = "Code-owned execution estimate. Does not replace the exact token forecast or grant fresh maker allowance."
)]
pub struct Execution {
    pub allowed_tool_turns: ReplyNumber<0, 0, true, false, true>,
    pub finalization_turns: ReplyNumber<0, 0, true, false, true>,
    pub usable_tool_turns: ReplyNumber<0, 0, true, false, true>,
    pub estimated_tool_turns: ReplyNumber<0, 0, true, false, true>,
    pub components: std::collections::BTreeMap<String, ReplyNumber<0, 0, true, false, true>>,
    pub exploration_declared: bool,
    pub fits: bool,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "List<Execution, 0, UNBOUNDED, false>", default)]
    pub pieces: Option<List<Execution, 0, UNBOUNDED, false>>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "ReplyNumber<0, 0, true, false, true>", default)]
    pub aggregate_tool_turns: Option<ReplyNumber<0, 0, true, false, true>>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "ReplyNumber<0, 0, true, false, true>", default)]
    pub aggregate_allowed_tool_turns: Option<ReplyNumber<0, 0, true, false, true>>,
}

pub use super::ingest_reply::{
    Forecast, IngestReplyBriefMemoryFlagsItem as MemoryFlag,
    IngestReplyBriefMemoryFlagsItemEvidence as SourceQuote,
};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(inline)]
#[schemars(transform = reply_constant_schema)]
pub enum PlannerReplyLeafKind {
    #[serde(rename = "leaf")]
    Value,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(inline)]
pub struct PlannerReplyLeafConventionsItem {
    pub text: BoundedText<1, 1000>,
    #[schemars(
        description = "A verbatim quote of at most 40 lines from the read-only checkout, at these 1-based lines of this repo-relative file."
    )]
    pub evidence: SourceQuote,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct DecisionContextJsonExternalDecisionContextJson(
    pub crate::decision_context::DecisionContext,
);
impl JsonSchema for DecisionContextJsonExternalDecisionContextJson {
    fn inline_schema() -> bool {
        true
    }
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "DecisionContextJsonExternalDecisionContextJson".into()
    }
    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({"$ref": "decision-context.json"})
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct IngestExternalEvidenceContract(pub super::ingest_reply::EvidenceContract);
impl JsonSchema for IngestExternalEvidenceContract {
    fn inline_schema() -> bool {
        true
    }
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "IngestExternalEvidenceContract".into()
    }
    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({"$ref": "ingest-reply.json#/$defs/evidence_contract"})
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(inline)]
pub struct PlannerReplyLeaf {
    pub kind: PlannerReplyLeafKind,
    #[schemars(extend("minItems" = 0))]
    pub files: List<BoundedText<1, UNBOUNDED>, 0, UNBOUNDED, true>,
    #[schemars(extend("minItems" = 0))]
    pub checks: List<crate::Check, 0, UNBOUNDED, false>,
    pub no_design_questions: bool,
    pub lands_on_its_own: bool,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "ReplyNumber<0, 0, true, false, true>", default)]
    #[schemars(
        description = "Required for accepted leaves and splits. Estimate orientation reads, searches, globs and recall separately from implementation and proof. Zero must be explicit."
    )]
    pub estimated_exploration_turns: Option<ReplyNumber<0, 0, true, false, true>>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "ReplyNumber<0, 0, true, false, true>", default)]
    #[schemars(
        description = "Estimate all required tool work, including proof, metadata and final verification. Code may increase this estimate."
    )]
    pub estimated_tool_turns: Option<ReplyNumber<0, 0, true, false, true>>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "Execution", default)]
    #[schemars(description = "Appended by worker code. The model must omit this field.")]
    pub execution: Option<Execution>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "List<BoundedText<1, UNBOUNDED>, 0, UNBOUNDED, true>", default)]
    #[schemars(
        description = "Existing tests, by pytest node ID, whose definitions the change modifies. Omit it when the change edits no existing test."
    )]
    #[schemars(extend("minItems" = 0))]
    pub edits_tests: Option<List<BoundedText<1, UNBOUNDED>, 0, UNBOUNDED, true>>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "Forecast", default)]
    pub forecast: Option<Forecast>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "List<BoundedText<1, UNBOUNDED>, 0, UNBOUNDED, false>", default)]
    #[schemars(description = "Code-owned rule violations. The model must omit this field.")]
    pub violations: Option<List<BoundedText<1, UNBOUNDED>, 0, UNBOUNDED, false>>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "List<BoundedText<1, UNBOUNDED>, 0, UNBOUNDED, true>", default)]
    #[schemars(description = "IDs of supplied memories this reply relied on.")]
    pub memory_ids: Option<List<BoundedText<1, UNBOUNDED>, 0, UNBOUNDED, true>>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "List<MemoryFlag, 0, 3, false>", default)]
    #[schemars(
        description = "Cited memories the checkout shows to be wrong, each with the evidence that contradicts it. Code checks the quote before the memory is retired."
    )]
    pub memory_flags: Option<List<MemoryFlag, 0, 3, false>>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "List<PlannerReplyLeafConventionsItem, 0, 3, false>", default)]
    #[schemars(
        description = "Repository conventions the checkout shows, each with evidence, kept for later requests. Code checks the quote."
    )]
    pub conventions: Option<List<PlannerReplyLeafConventionsItem, 0, 3, false>>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "DecisionContextJsonExternalDecisionContextJson", default)]
    #[schemars(
        description = "Code supplies the applicable approvals and restrictions from the planner inputs."
    )]
    pub decision_context: Option<DecisionContextJsonExternalDecisionContextJson>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "IngestExternalEvidenceContract", default)]
    pub evidence_contract: Option<IngestExternalEvidenceContract>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "List<BoundedText<1, 8192>, 0, 1024, true>", default)]
    #[schemars(description = "Code-owned unsupported evidence requirements.")]
    pub evidence_errors: Option<List<BoundedText<1, 8192>, 0, 1024, true>>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "List<JsonObject, 0, 9, false>", default)]
    #[schemars(
        description = "Code-owned verified planner rehearsal receipts. Independent Checking is still required."
    )]
    pub forcing_rehearsals: Option<List<JsonObject, 0, 9, false>>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(inline)]
#[schemars(transform = reply_constant_schema)]
pub enum PlannerReplySplitKind {
    #[serde(rename = "split")]
    Value,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(inline)]
#[schemars(transform = reply_enum_schema)]
pub enum PlannerReplySplitReason {
    #[serde(rename = "does_not_fit")]
    DoesNotFit,
    #[serde(rename = "execution_budget")]
    ExecutionBudget,
    #[serde(rename = "multiple_repos")]
    MultipleRepos,
    #[serde(rename = "out_of_room")]
    OutOfRoom,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(inline)]
pub struct PlannerReplySplitPiecesItem {
    pub brief: BoundedText<1, UNBOUNDED>,
    #[schemars(description = "Zero-based positions in inputs.ingest.goals.")]
    #[schemars(extend("minItems" = 0))]
    pub goal_ids: List<ReplyNumber<0, 0, true, false, true>, 0, UNBOUNDED, true>,
    #[schemars(extend("minItems" = 0))]
    pub files: List<BoundedText<1, UNBOUNDED>, 0, UNBOUNDED, true>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "ReplyNumber<0, 0, true, false, true>", default)]
    pub estimated_exploration_turns: Option<ReplyNumber<0, 0, true, false, true>>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "ReplyNumber<0, 0, true, false, true>", default)]
    pub estimated_tool_turns: Option<ReplyNumber<0, 0, true, false, true>>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(inline)]
pub struct PlannerReplySplit {
    pub kind: PlannerReplySplitKind,
    pub reason: PlannerReplySplitReason,
    #[schemars(extend("minItems" = 0))]
    pub pieces: List<PlannerReplySplitPiecesItem, 0, UNBOUNDED, false>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "ReplyNumber<0, 0, true, false, true>", default)]
    #[schemars(
        description = "Required for accepted leaves and splits. Estimate orientation reads, searches, globs and recall separately from implementation and proof. Zero must be explicit."
    )]
    pub estimated_exploration_turns: Option<ReplyNumber<0, 0, true, false, true>>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "ReplyNumber<0, 0, true, false, true>", default)]
    #[schemars(
        description = "Estimate all required tool work, including proof, metadata and final verification. Code may increase this estimate."
    )]
    pub estimated_tool_turns: Option<ReplyNumber<0, 0, true, false, true>>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "Execution", default)]
    #[schemars(description = "Appended by worker code. The model must omit this field.")]
    pub execution: Option<Execution>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "Forecast", default)]
    pub forecast: Option<Forecast>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "List<BoundedText<1, UNBOUNDED>, 0, UNBOUNDED, false>", default)]
    #[schemars(description = "Code-owned rule violations. The model must omit this field.")]
    pub violations: Option<List<BoundedText<1, UNBOUNDED>, 0, UNBOUNDED, false>>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "List<BoundedText<1, UNBOUNDED>, 0, UNBOUNDED, true>", default)]
    #[schemars(description = "IDs of supplied memories this reply relied on.")]
    pub memory_ids: Option<List<BoundedText<1, UNBOUNDED>, 0, UNBOUNDED, true>>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "List<MemoryFlag, 0, 3, false>", default)]
    #[schemars(
        description = "Cited memories the checkout shows to be wrong, each with the evidence that contradicts it. Code checks the quote before the memory is retired."
    )]
    pub memory_flags: Option<List<MemoryFlag, 0, 3, false>>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "List<PlannerReplyLeafConventionsItem, 0, 3, false>", default)]
    #[schemars(
        description = "Repository conventions the checkout shows, each with evidence, kept for later requests. Code checks the quote."
    )]
    pub conventions: Option<List<PlannerReplyLeafConventionsItem, 0, 3, false>>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "DecisionContextJsonExternalDecisionContextJson", default)]
    #[schemars(
        description = "Code supplies the applicable approvals and restrictions from the planner inputs."
    )]
    pub decision_context: Option<DecisionContextJsonExternalDecisionContextJson>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "IngestExternalEvidenceContract", default)]
    pub evidence_contract: Option<IngestExternalEvidenceContract>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "List<BoundedText<1, 8192>, 0, 1024, true>", default)]
    #[schemars(description = "Code-owned unsupported evidence requirements.")]
    pub evidence_errors: Option<List<BoundedText<1, 8192>, 0, 1024, true>>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "List<JsonObject, 0, 9, false>", default)]
    #[schemars(
        description = "Code-owned verified planner rehearsal receipts. Independent Checking is still required."
    )]
    pub forcing_rehearsals: Option<List<JsonObject, 0, 9, false>>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(inline)]
#[schemars(transform = reply_constant_schema)]
pub enum PlannerReplyCantMeetKind {
    #[serde(rename = "cant_meet")]
    Value,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(inline)]
pub struct PlannerReplyCantMeet {
    pub kind: PlannerReplyCantMeetKind,
    pub report: BoundedText<1, UNBOUNDED>,
    #[schemars(extend("minItems" = 0))]
    pub files: List<BoundedText<1, UNBOUNDED>, 0, UNBOUNDED, true>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "ReplyNumber<0, 0, true, false, true>", default)]
    #[schemars(
        description = "Required for accepted leaves and splits. Estimate orientation reads, searches, globs and recall separately from implementation and proof. Zero must be explicit."
    )]
    pub estimated_exploration_turns: Option<ReplyNumber<0, 0, true, false, true>>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "ReplyNumber<0, 0, true, false, true>", default)]
    #[schemars(
        description = "Estimate all required tool work, including proof, metadata and final verification. Code may increase this estimate."
    )]
    pub estimated_tool_turns: Option<ReplyNumber<0, 0, true, false, true>>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "Execution", default)]
    #[schemars(description = "Appended by worker code. The model must omit this field.")]
    pub execution: Option<Execution>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "Forecast", default)]
    pub forecast: Option<Forecast>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "List<BoundedText<1, UNBOUNDED>, 0, UNBOUNDED, false>", default)]
    #[schemars(description = "Code-owned rule violations. The model must omit this field.")]
    pub violations: Option<List<BoundedText<1, UNBOUNDED>, 0, UNBOUNDED, false>>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "List<BoundedText<1, UNBOUNDED>, 1, UNBOUNDED, true>", default)]
    #[schemars(
        description = "Concrete repository-relative paths that MUST change to meet the request's goals. Report only when the whole request cannot land usefully without these edits; never for inspected files, excluded edits, or an optional part. The controller verifies the effective protected-path list."
    )]
    pub protected_paths: Option<List<BoundedText<1, UNBOUNDED>, 1, UNBOUNDED, true>>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "List<BoundedText<1, UNBOUNDED>, 0, UNBOUNDED, true>", default)]
    #[schemars(description = "IDs of supplied memories this reply relied on.")]
    pub memory_ids: Option<List<BoundedText<1, UNBOUNDED>, 0, UNBOUNDED, true>>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "List<MemoryFlag, 0, 3, false>", default)]
    #[schemars(
        description = "Cited memories the checkout shows to be wrong, each with the evidence that contradicts it. Code checks the quote before the memory is retired."
    )]
    pub memory_flags: Option<List<MemoryFlag, 0, 3, false>>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "List<PlannerReplyLeafConventionsItem, 0, 3, false>", default)]
    #[schemars(
        description = "Repository conventions the checkout shows, each with evidence, kept for later requests. Code checks the quote."
    )]
    pub conventions: Option<List<PlannerReplyLeafConventionsItem, 0, 3, false>>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "DecisionContextJsonExternalDecisionContextJson", default)]
    #[schemars(
        description = "Code supplies the applicable approvals and restrictions from the planner inputs."
    )]
    pub decision_context: Option<DecisionContextJsonExternalDecisionContextJson>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "IngestExternalEvidenceContract", default)]
    pub evidence_contract: Option<IngestExternalEvidenceContract>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "List<BoundedText<1, 8192>, 0, 1024, true>", default)]
    #[schemars(description = "Code-owned unsupported evidence requirements.")]
    pub evidence_errors: Option<List<BoundedText<1, 8192>, 0, 1024, true>>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "List<JsonObject, 0, 9, false>", default)]
    #[schemars(
        description = "Code-owned verified planner rehearsal receipts. Independent Checking is still required."
    )]
    pub forcing_rehearsals: Option<List<JsonObject, 0, 9, false>>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
#[schemars(inline)]
#[schemars(transform = reply_one_of)]
pub enum PlannerReply {
    Leaf(Box<PlannerReplyLeaf>),
    Split(Box<PlannerReplySplit>),
    CantMeet(Box<PlannerReplyCantMeet>),
}

pub fn schema_value() -> serde_json::Value {
    let mut schema = reply_schema_value::<PlannerReply>("planner", |_| {});
    let kind = &mut schema["$defs"]["Check"]["properties"]["kind"];
    *kind = serde_json::json!({"enum": [crate::CheckKind::Demonstrating, crate::CheckKind::Regression, crate::CheckKind::BehaviorPreserving]});
    schema
}
pub fn schema_text() -> String {
    serde_json::to_string_pretty(&schema_value()).expect("planner schema serializes") + "\n"
}
