use crate::bounded::{Text as BoundedText, *};
use crate::render::{Layout, TypeOrder, TypeUse};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(rename = "execution")]
#[schemars(description = "Code-owned execution estimate. Does not replace the exact token forecast or grant fresh maker allowance.")]
pub struct Execution {
    pub allowed_tool_turns: ReplyNumber<0, 0, true, false, true>,
    pub finalization_turns: ReplyNumber<0, 0, true, false, true>,
    pub usable_tool_turns: ReplyNumber<0, 0, true, false, true>,
    pub estimated_tool_turns: ReplyNumber<0, 0, true, false, true>,
    pub components: std::collections::BTreeMap<String, ReplyNumber<0, 0, true, false, true>>,
    pub exploration_declared: bool,
    pub fits: bool,
    #[serde(default, deserialize_with = "present", skip_serializing_if = "Option::is_none")]
    #[schemars(with = "List<Execution, 0, UNBOUNDED, false>", default)]
    pub pieces: Option<List<Execution, 0, UNBOUNDED, false>>,
    #[serde(default, deserialize_with = "present", skip_serializing_if = "Option::is_none")]
    #[schemars(with = "ReplyNumber<0, 0, true, false, true>", default)]
    pub aggregate_tool_turns: Option<ReplyNumber<0, 0, true, false, true>>,
    #[serde(default, deserialize_with = "present", skip_serializing_if = "Option::is_none")]
    #[schemars(with = "ReplyNumber<0, 0, true, false, true>", default)]
    pub aggregate_allowed_tool_turns: Option<ReplyNumber<0, 0, true, false, true>>,
}

pub use super::ingest_reply::{Forecast, IngestReplyBriefMemoryFlagsItem as MemoryFlag, IngestReplyBriefMemoryFlagsItemEvidence as SourceQuote};

macro_rules! declaration {
    ($name:ident, $wire:literal, $inner:ty) => {
        #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub $inner);
        impl JsonSchema for $name {
            fn schema_name() -> std::borrow::Cow<'static, str> { $wire.into() }
            fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
                super::inline_declaration_schema::<$inner>()
            }
        }
    };
}
declaration!(Forcing, "forcing", crate::checks::Forcing);
declaration!(Proof, "proof", crate::checks::ProofDeclaration);
declaration!(DeclaredSymptom, "declared_symptom", crate::checks::DeclaredSymptom);

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(inline)]
#[schemars(transform = reply_constant_schema)]
pub enum PlannerReplyLeafKind { #[serde(rename = "leaf")] Value }

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(inline)]
#[schemars(transform = reply_enum_schema)]
pub enum PlannerReplyLeafChecksItemKind {
    #[serde(rename = "demonstrating")]
    Demonstrating,
    #[serde(rename = "regression")]
    Regression,
    #[serde(rename = "behavior-preserving")]
    BehaviorPreserving,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(inline)]
pub struct PlannerReplyLeafChecksItem {
    pub kind: PlannerReplyLeafChecksItemKind,
    #[schemars(description = "Exact pytest function or method selectors, validated by tl3_harness.Check: tests/test_api.py::test_status or tests/test_api.py::TestAPI::test_status. An unparameterized function selects all its cases. Whole files, directories and globs are forbidden. Regression selectors must name existing base definitions.")]
    pub node_ids: List<BoundedText<1, UNBOUNDED>, 0, UNBOUNDED, true>,
    #[schemars(description = "Zero-based positions in inputs.ingest.goals.")]
    pub goal_ids: List<ReplyNumber<0, 0, true, false, true>, 0, UNBOUNDED, true>,
    #[serde(default, deserialize_with = "present", skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Forcing", default)]
    pub forcing: Option<Forcing>,
    #[serde(default, deserialize_with = "present", skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Proof", default)]
    pub proof: Option<Proof>,
    #[serde(default, deserialize_with = "present", skip_serializing_if = "Option::is_none")]
    #[schemars(with = "DeclaredSymptom", default)]
    pub declared_symptom: Option<DeclaredSymptom>,
}




#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(inline)]
pub struct PlannerReplyLeafConventionsItem {
    pub text: BoundedText<1, 1000>,
    #[schemars(description = "A verbatim quote of at most 40 lines from the read-only checkout, at these 1-based lines of this repo-relative file.")]
    pub evidence: SourceQuote,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct DecisionContextJsonExternalDecisionContextJson(pub crate::decision_context::DecisionContext);
impl JsonSchema for DecisionContextJsonExternalDecisionContextJson {
    fn inline_schema() -> bool { true }
    fn schema_name() -> std::borrow::Cow<'static,str> { "DecisionContextJsonExternalDecisionContextJson".into() }
    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema { schemars::json_schema!({"$ref": "decision-context.json"}) }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct IngestExternalEvidenceContract(pub super::ingest_reply::EvidenceContract);
impl JsonSchema for IngestExternalEvidenceContract {
    fn inline_schema() -> bool { true }
    fn schema_name() -> std::borrow::Cow<'static,str> { "IngestExternalEvidenceContract".into() }
    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema { schemars::json_schema!({"$ref": "ingest-reply.json#/$defs/evidence_contract"}) }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(inline)]
pub struct PlannerReplyLeaf {
    pub kind: PlannerReplyLeafKind,
    pub files: List<BoundedText<1, UNBOUNDED>, 0, UNBOUNDED, true>,
    pub checks: List<PlannerReplyLeafChecksItem, 0, UNBOUNDED, false>,
    pub no_design_questions: bool,
    pub lands_on_its_own: bool,
    #[serde(default, deserialize_with = "present", skip_serializing_if = "Option::is_none")]
    #[schemars(with = "ReplyNumber<0, 0, true, false, true>", default)]
    #[schemars(description = "Required for accepted leaves and splits. Estimate orientation reads, searches, globs and recall separately from implementation and proof. Zero must be explicit.")]
    pub estimated_exploration_turns: Option<ReplyNumber<0, 0, true, false, true>>,
    #[serde(default, deserialize_with = "present", skip_serializing_if = "Option::is_none")]
    #[schemars(with = "ReplyNumber<0, 0, true, false, true>", default)]
    #[schemars(description = "Estimate all required tool work, including proof, metadata and final verification. Code may increase this estimate.")]
    pub estimated_tool_turns: Option<ReplyNumber<0, 0, true, false, true>>,
    #[serde(default, deserialize_with = "present", skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Execution", default)]
    #[schemars(description = "Appended by worker code. The model must omit this field.")]
    pub execution: Option<Execution>,
    #[serde(default, deserialize_with = "present", skip_serializing_if = "Option::is_none")]
    #[schemars(with = "List<BoundedText<1, UNBOUNDED>, 0, UNBOUNDED, true>", default)]
    #[schemars(description = "Existing tests, by pytest node ID, whose definitions the change modifies. Omit it when the change edits no existing test.")]
    pub edits_tests: Option<List<BoundedText<1, UNBOUNDED>, 0, UNBOUNDED, true>>,
    #[serde(default, deserialize_with = "present", skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Forecast", default)]
    pub forecast: Option<Forecast>,
    #[serde(default, deserialize_with = "present", skip_serializing_if = "Option::is_none")]
    #[schemars(with = "List<BoundedText<1, UNBOUNDED>, 0, UNBOUNDED, false>", default)]
    #[schemars(description = "Code-owned rule violations. The model must omit this field.")]
    pub violations: Option<List<BoundedText<1, UNBOUNDED>, 0, UNBOUNDED, false>>,
    #[serde(default, deserialize_with = "present", skip_serializing_if = "Option::is_none")]
    #[schemars(with = "List<BoundedText<1, UNBOUNDED>, 0, UNBOUNDED, true>", default)]
    #[schemars(description = "IDs of supplied memories this reply relied on.")]
    pub memory_ids: Option<List<BoundedText<1, UNBOUNDED>, 0, UNBOUNDED, true>>,
    #[serde(default, deserialize_with = "present", skip_serializing_if = "Option::is_none")]
    #[schemars(with = "List<MemoryFlag, 0, 3, false>", default)]
    #[schemars(description = "Cited memories the checkout shows to be wrong, each with the evidence that contradicts it. Code checks the quote before the memory is retired.")]
    pub memory_flags: Option<List<MemoryFlag, 0, 3, false>>,
    #[serde(default, deserialize_with = "present", skip_serializing_if = "Option::is_none")]
    #[schemars(with = "List<PlannerReplyLeafConventionsItem, 0, 3, false>", default)]
    #[schemars(description = "Repository conventions the checkout shows, each with evidence, kept for later requests. Code checks the quote.")]
    pub conventions: Option<List<PlannerReplyLeafConventionsItem, 0, 3, false>>,
    #[serde(default, deserialize_with = "present", skip_serializing_if = "Option::is_none")]
    #[schemars(with = "DecisionContextJsonExternalDecisionContextJson", default)]
    #[schemars(description = "Code supplies the applicable approvals and restrictions from the planner inputs.")]
    pub decision_context: Option<DecisionContextJsonExternalDecisionContextJson>,
    #[serde(default, deserialize_with = "present", skip_serializing_if = "Option::is_none")]
    #[schemars(with = "IngestExternalEvidenceContract", default)]
    pub evidence_contract: Option<IngestExternalEvidenceContract>,
    #[serde(default, deserialize_with = "present", skip_serializing_if = "Option::is_none")]
    #[schemars(with = "List<BoundedText<1, 8192>, 0, 1024, true>", default)]
    #[schemars(description = "Code-owned unsupported evidence requirements.")]
    pub evidence_errors: Option<List<BoundedText<1, 8192>, 0, 1024, true>>,
    #[serde(default, deserialize_with = "present", skip_serializing_if = "Option::is_none")]
    #[schemars(with = "List<JsonObject, 0, 9, false>", default)]
    #[schemars(description = "Code-owned verified planner rehearsal receipts. Independent Checking is still required.")]
    pub forcing_rehearsals: Option<List<JsonObject, 0, 9, false>>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(inline)]
#[schemars(transform = reply_constant_schema)]
pub enum PlannerReplySplitKind { #[serde(rename = "split")] Value }

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
    pub goal_ids: List<ReplyNumber<0, 0, true, false, true>, 0, UNBOUNDED, true>,
    pub files: List<BoundedText<1, UNBOUNDED>, 0, UNBOUNDED, true>,
    #[serde(default, deserialize_with = "present", skip_serializing_if = "Option::is_none")]
    #[schemars(with = "ReplyNumber<0, 0, true, false, true>", default)]
    pub estimated_exploration_turns: Option<ReplyNumber<0, 0, true, false, true>>,
    #[serde(default, deserialize_with = "present", skip_serializing_if = "Option::is_none")]
    #[schemars(with = "ReplyNumber<0, 0, true, false, true>", default)]
    pub estimated_tool_turns: Option<ReplyNumber<0, 0, true, false, true>>,
}







#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(inline)]
pub struct PlannerReplySplit {
    pub kind: PlannerReplySplitKind,
    pub reason: PlannerReplySplitReason,
    pub pieces: List<PlannerReplySplitPiecesItem, 0, UNBOUNDED, false>,
    #[serde(default, deserialize_with = "present", skip_serializing_if = "Option::is_none")]
    #[schemars(with = "ReplyNumber<0, 0, true, false, true>", default)]
    #[schemars(description = "Required for accepted leaves and splits. Estimate orientation reads, searches, globs and recall separately from implementation and proof. Zero must be explicit.")]
    pub estimated_exploration_turns: Option<ReplyNumber<0, 0, true, false, true>>,
    #[serde(default, deserialize_with = "present", skip_serializing_if = "Option::is_none")]
    #[schemars(with = "ReplyNumber<0, 0, true, false, true>", default)]
    #[schemars(description = "Estimate all required tool work, including proof, metadata and final verification. Code may increase this estimate.")]
    pub estimated_tool_turns: Option<ReplyNumber<0, 0, true, false, true>>,
    #[serde(default, deserialize_with = "present", skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Execution", default)]
    #[schemars(description = "Appended by worker code. The model must omit this field.")]
    pub execution: Option<Execution>,
    #[serde(default, deserialize_with = "present", skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Forecast", default)]
    pub forecast: Option<Forecast>,
    #[serde(default, deserialize_with = "present", skip_serializing_if = "Option::is_none")]
    #[schemars(with = "List<BoundedText<1, UNBOUNDED>, 0, UNBOUNDED, false>", default)]
    #[schemars(description = "Code-owned rule violations. The model must omit this field.")]
    pub violations: Option<List<BoundedText<1, UNBOUNDED>, 0, UNBOUNDED, false>>,
    #[serde(default, deserialize_with = "present", skip_serializing_if = "Option::is_none")]
    #[schemars(with = "List<BoundedText<1, UNBOUNDED>, 0, UNBOUNDED, true>", default)]
    #[schemars(description = "IDs of supplied memories this reply relied on.")]
    pub memory_ids: Option<List<BoundedText<1, UNBOUNDED>, 0, UNBOUNDED, true>>,
    #[serde(default, deserialize_with = "present", skip_serializing_if = "Option::is_none")]
    #[schemars(with = "List<MemoryFlag, 0, 3, false>", default)]
    #[schemars(description = "Cited memories the checkout shows to be wrong, each with the evidence that contradicts it. Code checks the quote before the memory is retired.")]
    pub memory_flags: Option<List<MemoryFlag, 0, 3, false>>,
    #[serde(default, deserialize_with = "present", skip_serializing_if = "Option::is_none")]
    #[schemars(with = "List<PlannerReplyLeafConventionsItem, 0, 3, false>", default)]
    #[schemars(description = "Repository conventions the checkout shows, each with evidence, kept for later requests. Code checks the quote.")]
    pub conventions: Option<List<PlannerReplyLeafConventionsItem, 0, 3, false>>,
    #[serde(default, deserialize_with = "present", skip_serializing_if = "Option::is_none")]
    #[schemars(with = "DecisionContextJsonExternalDecisionContextJson", default)]
    #[schemars(description = "Code supplies the applicable approvals and restrictions from the planner inputs.")]
    pub decision_context: Option<DecisionContextJsonExternalDecisionContextJson>,
    #[serde(default, deserialize_with = "present", skip_serializing_if = "Option::is_none")]
    #[schemars(with = "IngestExternalEvidenceContract", default)]
    pub evidence_contract: Option<IngestExternalEvidenceContract>,
    #[serde(default, deserialize_with = "present", skip_serializing_if = "Option::is_none")]
    #[schemars(with = "List<BoundedText<1, 8192>, 0, 1024, true>", default)]
    #[schemars(description = "Code-owned unsupported evidence requirements.")]
    pub evidence_errors: Option<List<BoundedText<1, 8192>, 0, 1024, true>>,
    #[serde(default, deserialize_with = "present", skip_serializing_if = "Option::is_none")]
    #[schemars(with = "List<JsonObject, 0, 9, false>", default)]
    #[schemars(description = "Code-owned verified planner rehearsal receipts. Independent Checking is still required.")]
    pub forcing_rehearsals: Option<List<JsonObject, 0, 9, false>>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(inline)]
#[schemars(transform = reply_constant_schema)]
pub enum PlannerReplyCantMeetKind { #[serde(rename = "cant_meet")] Value }







#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(inline)]
pub struct PlannerReplyCantMeet {
    pub kind: PlannerReplyCantMeetKind,
    pub report: BoundedText<1, UNBOUNDED>,
    pub files: List<BoundedText<1, UNBOUNDED>, 0, UNBOUNDED, true>,
    #[serde(default, deserialize_with = "present", skip_serializing_if = "Option::is_none")]
    #[schemars(with = "ReplyNumber<0, 0, true, false, true>", default)]
    #[schemars(description = "Required for accepted leaves and splits. Estimate orientation reads, searches, globs and recall separately from implementation and proof. Zero must be explicit.")]
    pub estimated_exploration_turns: Option<ReplyNumber<0, 0, true, false, true>>,
    #[serde(default, deserialize_with = "present", skip_serializing_if = "Option::is_none")]
    #[schemars(with = "ReplyNumber<0, 0, true, false, true>", default)]
    #[schemars(description = "Estimate all required tool work, including proof, metadata and final verification. Code may increase this estimate.")]
    pub estimated_tool_turns: Option<ReplyNumber<0, 0, true, false, true>>,
    #[serde(default, deserialize_with = "present", skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Execution", default)]
    #[schemars(description = "Appended by worker code. The model must omit this field.")]
    pub execution: Option<Execution>,
    #[serde(default, deserialize_with = "present", skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Forecast", default)]
    pub forecast: Option<Forecast>,
    #[serde(default, deserialize_with = "present", skip_serializing_if = "Option::is_none")]
    #[schemars(with = "List<BoundedText<1, UNBOUNDED>, 0, UNBOUNDED, false>", default)]
    #[schemars(description = "Code-owned rule violations. The model must omit this field.")]
    pub violations: Option<List<BoundedText<1, UNBOUNDED>, 0, UNBOUNDED, false>>,
    #[serde(default, deserialize_with = "present", skip_serializing_if = "Option::is_none")]
    #[schemars(with = "List<BoundedText<1, UNBOUNDED>, 1, UNBOUNDED, true>", default)]
    #[schemars(description = "Concrete repository-relative paths that MUST change to meet the request's goals. Report only when the whole request cannot land usefully without these edits; never for inspected files, excluded edits, or an optional part. The controller verifies the effective protected-path list.")]
    pub protected_paths: Option<List<BoundedText<1, UNBOUNDED>, 1, UNBOUNDED, true>>,
    #[serde(default, deserialize_with = "present", skip_serializing_if = "Option::is_none")]
    #[schemars(with = "List<BoundedText<1, UNBOUNDED>, 0, UNBOUNDED, true>", default)]
    #[schemars(description = "IDs of supplied memories this reply relied on.")]
    pub memory_ids: Option<List<BoundedText<1, UNBOUNDED>, 0, UNBOUNDED, true>>,
    #[serde(default, deserialize_with = "present", skip_serializing_if = "Option::is_none")]
    #[schemars(with = "List<MemoryFlag, 0, 3, false>", default)]
    #[schemars(description = "Cited memories the checkout shows to be wrong, each with the evidence that contradicts it. Code checks the quote before the memory is retired.")]
    pub memory_flags: Option<List<MemoryFlag, 0, 3, false>>,
    #[serde(default, deserialize_with = "present", skip_serializing_if = "Option::is_none")]
    #[schemars(with = "List<PlannerReplyLeafConventionsItem, 0, 3, false>", default)]
    #[schemars(description = "Repository conventions the checkout shows, each with evidence, kept for later requests. Code checks the quote.")]
    pub conventions: Option<List<PlannerReplyLeafConventionsItem, 0, 3, false>>,
    #[serde(default, deserialize_with = "present", skip_serializing_if = "Option::is_none")]
    #[schemars(with = "DecisionContextJsonExternalDecisionContextJson", default)]
    #[schemars(description = "Code supplies the applicable approvals and restrictions from the planner inputs.")]
    pub decision_context: Option<DecisionContextJsonExternalDecisionContextJson>,
    #[serde(default, deserialize_with = "present", skip_serializing_if = "Option::is_none")]
    #[schemars(with = "IngestExternalEvidenceContract", default)]
    pub evidence_contract: Option<IngestExternalEvidenceContract>,
    #[serde(default, deserialize_with = "present", skip_serializing_if = "Option::is_none")]
    #[schemars(with = "List<BoundedText<1, 8192>, 0, 1024, true>", default)]
    #[schemars(description = "Code-owned unsupported evidence requirements.")]
    pub evidence_errors: Option<List<BoundedText<1, 8192>, 0, 1024, true>>,
    #[serde(default, deserialize_with = "present", skip_serializing_if = "Option::is_none")]
    #[schemars(with = "List<JsonObject, 0, 9, false>", default)]
    #[schemars(description = "Code-owned verified planner rehearsal receipts. Independent Checking is still required.")]
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
    reply_schema_value::<PlannerReply>("planner", |g| {
    g.subschema_for::<Execution>();
    g.subschema_for::<Forecast>();
    g.subschema_for::<Forcing>();
    g.subschema_for::<Proof>();
    g.subschema_for::<DeclaredSymptom>();
    })
}

pub fn schema_text() -> String {
    let types = [
        TypeOrder::of::<Execution>(EXECUTION_ORDER),
        TypeOrder::of::<Forecast>(FORECAST_ORDER),
        TypeOrder::of::<Forcing>(FORCING_ORDER),
        TypeOrder::of::<Proof>(PROOF_ORDER),
        TypeOrder::of::<DeclaredSymptom>(DECLARED_SYMPTOM_ORDER),
    ];
    let shared = [
        TypeUse { pointer: "/$defs/execution", order: &types[0] },
        TypeUse { pointer: "/$defs/forecast", order: &types[1] },
        TypeUse { pointer: "/$defs/forcing", order: &types[2] },
        TypeUse { pointer: "/$defs/proof", order: &types[3] },
        TypeUse { pointer: "/$defs/declared_symptom", order: &types[4] },
    ];
    crate::render::render("planner-reply.json", &schema_value(), &Layout { inline: ROOT_ORDER, shared: &shared }).expect("published reply schema order is exhaustive")
}

pub const ROOT_ORDER: &[crate::render::ObjectOrder<'static>] = &[
    ("", &["$schema", "$id", "$defs", "oneOf"]),
    ("/$defs", &["execution", "forecast", "forcing", "proof", "declared_symptom"]),
    ("/oneOf/0", &["type", "properties", "required", "additionalProperties"]),
    ("/oneOf/0/properties", &["estimated_exploration_turns", "estimated_tool_turns", "execution", "kind", "files", "checks", "edits_tests", "no_design_questions", "lands_on_its_own", "forecast", "violations", "memory_ids", "memory_flags", "conventions", "decision_context", "evidence_contract", "evidence_errors", "forcing_rehearsals"]),
    ("/oneOf/0/properties/estimated_exploration_turns", &["type", "minimum", "description"]),
    ("/oneOf/0/properties/estimated_tool_turns", &["type", "minimum", "description"]),
    ("/oneOf/0/properties/execution", &["$ref", "description"]),
    ("/oneOf/0/properties/kind", &["const"]),
    ("/oneOf/0/properties/files", &["type", "items", "minItems", "uniqueItems"]),
    ("/oneOf/0/properties/files/items", &["type", "minLength"]),
    ("/oneOf/0/properties/checks", &["type", "items", "minItems"]),
    ("/oneOf/0/properties/checks/items", &["type", "properties", "required", "additionalProperties"]),
    ("/oneOf/0/properties/checks/items/properties", &["kind", "node_ids", "goal_ids", "forcing", "proof", "declared_symptom"]),
    ("/oneOf/0/properties/checks/items/properties/kind", &["enum"]),
    ("/oneOf/0/properties/checks/items/properties/node_ids", &["description", "type", "items", "minItems", "uniqueItems"]),
    ("/oneOf/0/properties/checks/items/properties/node_ids/items", &["type", "minLength"]),
    ("/oneOf/0/properties/checks/items/properties/goal_ids", &["type", "items", "minItems", "uniqueItems", "description"]),
    ("/oneOf/0/properties/checks/items/properties/goal_ids/items", &["type", "minimum"]),
    ("/oneOf/0/properties/checks/items/properties/forcing", &["$ref"]),
    ("/oneOf/0/properties/checks/items/properties/proof", &["$ref"]),
    ("/oneOf/0/properties/checks/items/properties/declared_symptom", &["$ref"]),
    ("/oneOf/0/properties/edits_tests", &["type", "items", "minItems", "uniqueItems", "description"]),
    ("/oneOf/0/properties/edits_tests/items", &["type", "minLength"]),
    ("/oneOf/0/properties/no_design_questions", &["type"]),
    ("/oneOf/0/properties/lands_on_its_own", &["type"]),
    ("/oneOf/0/properties/forecast", &["$ref"]),
    ("/oneOf/0/properties/violations", &["type", "items", "description"]),
    ("/oneOf/0/properties/violations/items", &["type", "minLength"]),
    ("/oneOf/0/properties/memory_ids", &["type", "items", "uniqueItems", "description"]),
    ("/oneOf/0/properties/memory_ids/items", &["type", "minLength"]),
    ("/oneOf/0/properties/memory_flags", &["type", "maxItems", "items", "description"]),
    ("/oneOf/0/properties/memory_flags/items", &["type", "properties", "required", "additionalProperties"]),
    ("/oneOf/0/properties/memory_flags/items/properties", &["id", "claim", "reason", "evidence"]),
    ("/oneOf/0/properties/memory_flags/items/properties/id", &["type", "minLength", "maxLength"]),
    ("/oneOf/0/properties/memory_flags/items/properties/claim", &["type", "minLength", "maxLength", "description"]),
    ("/oneOf/0/properties/memory_flags/items/properties/reason", &["type", "minLength", "maxLength"]),
    ("/oneOf/0/properties/memory_flags/items/properties/evidence", &["type", "properties", "required", "additionalProperties", "description"]),
    ("/oneOf/0/properties/memory_flags/items/properties/evidence/properties", &["path", "start_line", "end_line", "quote"]),
    ("/oneOf/0/properties/memory_flags/items/properties/evidence/properties/path", &["type", "minLength", "maxLength"]),
    ("/oneOf/0/properties/memory_flags/items/properties/evidence/properties/start_line", &["type", "minimum"]),
    ("/oneOf/0/properties/memory_flags/items/properties/evidence/properties/end_line", &["type", "minimum"]),
    ("/oneOf/0/properties/memory_flags/items/properties/evidence/properties/quote", &["type", "minLength", "maxLength"]),
    ("/oneOf/0/properties/conventions", &["type", "maxItems", "items", "description"]),
    ("/oneOf/0/properties/conventions/items", &["type", "properties", "required", "additionalProperties"]),
    ("/oneOf/0/properties/conventions/items/properties", &["text", "evidence"]),
    ("/oneOf/0/properties/conventions/items/properties/text", &["type", "minLength", "maxLength"]),
    ("/oneOf/0/properties/conventions/items/properties/evidence", &["type", "properties", "required", "additionalProperties", "description"]),
    ("/oneOf/0/properties/conventions/items/properties/evidence/properties", &["path", "start_line", "end_line", "quote"]),
    ("/oneOf/0/properties/conventions/items/properties/evidence/properties/path", &["type", "minLength", "maxLength"]),
    ("/oneOf/0/properties/conventions/items/properties/evidence/properties/start_line", &["type", "minimum"]),
    ("/oneOf/0/properties/conventions/items/properties/evidence/properties/end_line", &["type", "minimum"]),
    ("/oneOf/0/properties/conventions/items/properties/evidence/properties/quote", &["type", "minLength", "maxLength"]),
    ("/oneOf/0/properties/decision_context", &["$ref", "description"]),
    ("/oneOf/0/properties/evidence_contract", &["$ref"]),
    ("/oneOf/0/properties/evidence_errors", &["type", "items", "uniqueItems", "maxItems", "description"]),
    ("/oneOf/0/properties/evidence_errors/items", &["type", "minLength", "maxLength"]),
    ("/oneOf/0/properties/forcing_rehearsals", &["type", "maxItems", "items", "description"]),
    ("/oneOf/0/properties/forcing_rehearsals/items", &["type"]),
    ("/oneOf/1", &["type", "properties", "required", "additionalProperties"]),
    ("/oneOf/1/properties", &["estimated_exploration_turns", "estimated_tool_turns", "execution", "kind", "reason", "pieces", "forecast", "violations", "memory_ids", "memory_flags", "conventions", "decision_context", "evidence_contract", "evidence_errors", "forcing_rehearsals"]),
    ("/oneOf/1/properties/estimated_exploration_turns", &["type", "minimum", "description"]),
    ("/oneOf/1/properties/estimated_tool_turns", &["type", "minimum", "description"]),
    ("/oneOf/1/properties/execution", &["$ref", "description"]),
    ("/oneOf/1/properties/kind", &["const"]),
    ("/oneOf/1/properties/reason", &["enum"]),
    ("/oneOf/1/properties/pieces", &["type", "items", "minItems"]),
    ("/oneOf/1/properties/pieces/items", &["type", "properties", "required", "additionalProperties"]),
    ("/oneOf/1/properties/pieces/items/properties", &["estimated_exploration_turns", "estimated_tool_turns", "brief", "goal_ids", "files"]),
    ("/oneOf/1/properties/pieces/items/properties/estimated_exploration_turns", &["type", "minimum"]),
    ("/oneOf/1/properties/pieces/items/properties/estimated_tool_turns", &["type", "minimum"]),
    ("/oneOf/1/properties/pieces/items/properties/brief", &["type", "minLength"]),
    ("/oneOf/1/properties/pieces/items/properties/goal_ids", &["type", "items", "minItems", "uniqueItems", "description"]),
    ("/oneOf/1/properties/pieces/items/properties/goal_ids/items", &["type", "minimum"]),
    ("/oneOf/1/properties/pieces/items/properties/files", &["type", "items", "minItems", "uniqueItems"]),
    ("/oneOf/1/properties/pieces/items/properties/files/items", &["type", "minLength"]),
    ("/oneOf/1/properties/forecast", &["$ref"]),
    ("/oneOf/1/properties/violations", &["type", "items", "description"]),
    ("/oneOf/1/properties/violations/items", &["type", "minLength"]),
    ("/oneOf/1/properties/memory_ids", &["type", "items", "uniqueItems", "description"]),
    ("/oneOf/1/properties/memory_ids/items", &["type", "minLength"]),
    ("/oneOf/1/properties/memory_flags", &["type", "maxItems", "items", "description"]),
    ("/oneOf/1/properties/memory_flags/items", &["type", "properties", "required", "additionalProperties"]),
    ("/oneOf/1/properties/memory_flags/items/properties", &["id", "claim", "reason", "evidence"]),
    ("/oneOf/1/properties/memory_flags/items/properties/id", &["type", "minLength", "maxLength"]),
    ("/oneOf/1/properties/memory_flags/items/properties/claim", &["type", "minLength", "maxLength", "description"]),
    ("/oneOf/1/properties/memory_flags/items/properties/reason", &["type", "minLength", "maxLength"]),
    ("/oneOf/1/properties/memory_flags/items/properties/evidence", &["type", "properties", "required", "additionalProperties", "description"]),
    ("/oneOf/1/properties/memory_flags/items/properties/evidence/properties", &["path", "start_line", "end_line", "quote"]),
    ("/oneOf/1/properties/memory_flags/items/properties/evidence/properties/path", &["type", "minLength", "maxLength"]),
    ("/oneOf/1/properties/memory_flags/items/properties/evidence/properties/start_line", &["type", "minimum"]),
    ("/oneOf/1/properties/memory_flags/items/properties/evidence/properties/end_line", &["type", "minimum"]),
    ("/oneOf/1/properties/memory_flags/items/properties/evidence/properties/quote", &["type", "minLength", "maxLength"]),
    ("/oneOf/1/properties/conventions", &["type", "maxItems", "items", "description"]),
    ("/oneOf/1/properties/conventions/items", &["type", "properties", "required", "additionalProperties"]),
    ("/oneOf/1/properties/conventions/items/properties", &["text", "evidence"]),
    ("/oneOf/1/properties/conventions/items/properties/text", &["type", "minLength", "maxLength"]),
    ("/oneOf/1/properties/conventions/items/properties/evidence", &["type", "properties", "required", "additionalProperties", "description"]),
    ("/oneOf/1/properties/conventions/items/properties/evidence/properties", &["path", "start_line", "end_line", "quote"]),
    ("/oneOf/1/properties/conventions/items/properties/evidence/properties/path", &["type", "minLength", "maxLength"]),
    ("/oneOf/1/properties/conventions/items/properties/evidence/properties/start_line", &["type", "minimum"]),
    ("/oneOf/1/properties/conventions/items/properties/evidence/properties/end_line", &["type", "minimum"]),
    ("/oneOf/1/properties/conventions/items/properties/evidence/properties/quote", &["type", "minLength", "maxLength"]),
    ("/oneOf/1/properties/decision_context", &["$ref", "description"]),
    ("/oneOf/1/properties/evidence_contract", &["$ref"]),
    ("/oneOf/1/properties/evidence_errors", &["type", "items", "uniqueItems", "maxItems", "description"]),
    ("/oneOf/1/properties/evidence_errors/items", &["type", "minLength", "maxLength"]),
    ("/oneOf/1/properties/forcing_rehearsals", &["type", "maxItems", "items", "description"]),
    ("/oneOf/1/properties/forcing_rehearsals/items", &["type"]),
    ("/oneOf/2", &["type", "properties", "required", "additionalProperties"]),
    ("/oneOf/2/properties", &["estimated_exploration_turns", "estimated_tool_turns", "execution", "kind", "report", "files", "forecast", "violations", "protected_paths", "memory_ids", "memory_flags", "conventions", "decision_context", "evidence_contract", "evidence_errors", "forcing_rehearsals"]),
    ("/oneOf/2/properties/estimated_exploration_turns", &["type", "minimum", "description"]),
    ("/oneOf/2/properties/estimated_tool_turns", &["type", "minimum", "description"]),
    ("/oneOf/2/properties/execution", &["$ref", "description"]),
    ("/oneOf/2/properties/kind", &["const"]),
    ("/oneOf/2/properties/report", &["type", "minLength"]),
    ("/oneOf/2/properties/files", &["type", "items", "minItems", "uniqueItems"]),
    ("/oneOf/2/properties/files/items", &["type", "minLength"]),
    ("/oneOf/2/properties/forecast", &["$ref"]),
    ("/oneOf/2/properties/violations", &["type", "items", "description"]),
    ("/oneOf/2/properties/violations/items", &["type", "minLength"]),
    ("/oneOf/2/properties/protected_paths", &["type", "items", "minItems", "uniqueItems", "description"]),
    ("/oneOf/2/properties/protected_paths/items", &["type", "minLength"]),
    ("/oneOf/2/properties/memory_ids", &["type", "items", "uniqueItems", "description"]),
    ("/oneOf/2/properties/memory_ids/items", &["type", "minLength"]),
    ("/oneOf/2/properties/memory_flags", &["type", "maxItems", "items", "description"]),
    ("/oneOf/2/properties/memory_flags/items", &["type", "properties", "required", "additionalProperties"]),
    ("/oneOf/2/properties/memory_flags/items/properties", &["id", "claim", "reason", "evidence"]),
    ("/oneOf/2/properties/memory_flags/items/properties/id", &["type", "minLength", "maxLength"]),
    ("/oneOf/2/properties/memory_flags/items/properties/claim", &["type", "minLength", "maxLength", "description"]),
    ("/oneOf/2/properties/memory_flags/items/properties/reason", &["type", "minLength", "maxLength"]),
    ("/oneOf/2/properties/memory_flags/items/properties/evidence", &["type", "properties", "required", "additionalProperties", "description"]),
    ("/oneOf/2/properties/memory_flags/items/properties/evidence/properties", &["path", "start_line", "end_line", "quote"]),
    ("/oneOf/2/properties/memory_flags/items/properties/evidence/properties/path", &["type", "minLength", "maxLength"]),
    ("/oneOf/2/properties/memory_flags/items/properties/evidence/properties/start_line", &["type", "minimum"]),
    ("/oneOf/2/properties/memory_flags/items/properties/evidence/properties/end_line", &["type", "minimum"]),
    ("/oneOf/2/properties/memory_flags/items/properties/evidence/properties/quote", &["type", "minLength", "maxLength"]),
    ("/oneOf/2/properties/conventions", &["type", "maxItems", "items", "description"]),
    ("/oneOf/2/properties/conventions/items", &["type", "properties", "required", "additionalProperties"]),
    ("/oneOf/2/properties/conventions/items/properties", &["text", "evidence"]),
    ("/oneOf/2/properties/conventions/items/properties/text", &["type", "minLength", "maxLength"]),
    ("/oneOf/2/properties/conventions/items/properties/evidence", &["type", "properties", "required", "additionalProperties", "description"]),
    ("/oneOf/2/properties/conventions/items/properties/evidence/properties", &["path", "start_line", "end_line", "quote"]),
    ("/oneOf/2/properties/conventions/items/properties/evidence/properties/path", &["type", "minLength", "maxLength"]),
    ("/oneOf/2/properties/conventions/items/properties/evidence/properties/start_line", &["type", "minimum"]),
    ("/oneOf/2/properties/conventions/items/properties/evidence/properties/end_line", &["type", "minimum"]),
    ("/oneOf/2/properties/conventions/items/properties/evidence/properties/quote", &["type", "minLength", "maxLength"]),
    ("/oneOf/2/properties/decision_context", &["$ref", "description"]),
    ("/oneOf/2/properties/evidence_contract", &["$ref"]),
    ("/oneOf/2/properties/evidence_errors", &["type", "items", "uniqueItems", "maxItems", "description"]),
    ("/oneOf/2/properties/evidence_errors/items", &["type", "minLength", "maxLength"]),
    ("/oneOf/2/properties/forcing_rehearsals", &["type", "maxItems", "items", "description"]),
    ("/oneOf/2/properties/forcing_rehearsals/items", &["type"]),
];
pub const EXECUTION_ORDER: &[crate::render::ObjectOrder<'static>] = &[
    ("", &["type", "properties", "required", "additionalProperties", "description"]),
    ("/properties", &["allowed_tool_turns", "finalization_turns", "usable_tool_turns", "estimated_tool_turns", "components", "exploration_declared", "fits", "pieces", "aggregate_tool_turns", "aggregate_allowed_tool_turns"]),
    ("/properties/allowed_tool_turns", &["type", "minimum"]),
    ("/properties/finalization_turns", &["type", "minimum"]),
    ("/properties/usable_tool_turns", &["type", "minimum"]),
    ("/properties/estimated_tool_turns", &["type", "minimum"]),
    ("/properties/components", &["type", "additionalProperties"]),
    ("/properties/components/additionalProperties", &["type", "minimum"]),
    ("/properties/exploration_declared", &["type"]),
    ("/properties/fits", &["type"]),
    ("/properties/pieces", &["type", "items"]),
    ("/properties/pieces/items", &["$ref"]),
    ("/properties/aggregate_tool_turns", &["type", "minimum"]),
    ("/properties/aggregate_allowed_tool_turns", &["type", "minimum"]),
];
pub const FORECAST_ORDER: &[crate::render::ObjectOrder<'static>] = &[
    ("", &["type", "properties", "required", "additionalProperties", "description"]),
    ("/properties", &["tokenizer", "brief_tokens", "file_tokens", "overhead_tokens", "tokens", "fit_tokens", "context_tokens", "fits"]),
    ("/properties/tokenizer", &["const"]),
    ("/properties/brief_tokens", &["type", "minimum"]),
    ("/properties/file_tokens", &["type", "additionalProperties"]),
    ("/properties/file_tokens/additionalProperties", &["type", "minimum"]),
    ("/properties/overhead_tokens", &["type", "minimum"]),
    ("/properties/tokens", &["type", "minimum"]),
    ("/properties/fit_tokens", &["type", "minimum"]),
    ("/properties/context_tokens", &["type", "minimum"]),
    ("/properties/fits", &["type"]),
];
pub const FORCING_ORDER: &[crate::render::ObjectOrder<'static>] = &[
    ("", &["type", "description", "properties", "required", "additionalProperties"]),
    ("/properties", &["symptom", "environment", "patches"]),
    ("/properties/symptom", &["type", "minLength", "maxLength"]),
    ("/properties/environment", &["type", "maxProperties", "propertyNames", "additionalProperties"]),
    ("/properties/environment/propertyNames", &["pattern"]),
    ("/properties/environment/additionalProperties", &["type", "maxLength"]),
    ("/properties/patches", &["type", "maxItems", "items"]),
    ("/properties/patches/items", &["type", "properties", "required", "minProperties", "maxProperties", "additionalProperties"]),
    ("/properties/patches/items/properties", &["target", "value", "returns", "raises", "delay_seconds"]),
    ("/properties/patches/items/properties/target", &["type", "maxLength", "pattern"]),
    ("/properties/patches/items/properties/value", &[]),
    ("/properties/patches/items/properties/returns", &[]),
    ("/properties/patches/items/properties/raises", &["type", "minLength"]),
    ("/properties/patches/items/properties/delay_seconds", &["type", "exclusiveMinimum", "maximum"]),
];
pub const PROOF_ORDER: &[crate::render::ObjectOrder<'static>] = &[
    ("", &["type", "additionalProperties", "properties", "required"]),
    ("/properties", &["subjects", "observer_support", "red_message"]),
    ("/properties/subjects", &["type", "minItems", "maxItems", "uniqueItems", "items"]),
    ("/properties/subjects/items", &["type", "minLength", "maxLength", "pattern"]),
    ("/properties/observer_support", &["type", "maxItems", "uniqueItems", "items"]),
    ("/properties/observer_support/items", &["type", "minLength", "maxLength", "pattern"]),
    ("/properties/red_message", &["type", "minLength", "maxLength"]),
];
pub const DECLARED_SYMPTOM_ORDER: &[crate::render::ObjectOrder<'static>] = &[
    ("", &["type", "properties", "required", "additionalProperties"]),
    ("/properties", &["node", "exception", "phase", "repeat", "parallel"]),
    ("/properties/node", &["type", "minLength"]),
    ("/properties/exception", &["type", "pattern"]),
    ("/properties/phase", &["enum"]),
    ("/properties/repeat", &["type", "minimum", "maximum"]),
    ("/properties/parallel", &["enum"]),
];
