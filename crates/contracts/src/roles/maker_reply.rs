use crate::bounded::{Text as BoundedText, *};
use crate::render::{Layout, TypeOrder, TypeUse};
use crate::run_binding::{self, BackendTag, Ending, RunBinding};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub use super::writer_reply::{TEXT_ORDER, Text};

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct WorkerWaitReference(String);
impl WorkerWaitReference {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
impl<'de> Deserialize<'de> for WorkerWaitReference {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        if value.strip_prefix("worker:").is_some_and(|key| run_binding::hex(key, 64)) {
            Ok(Self(value))
        } else {
            Err(serde::de::Error::custom("expected worker:<lowercase SHA-256>"))
        }
    }
}
impl JsonSchema for WorkerWaitReference {
    fn inline_schema() -> bool {
        true
    }
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "WorkerWaitReference".into()
    }
    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({"type":"string", "pattern":"^worker:[0-9a-f]{64}$"})
    }
}

fn validate_worker_receipt(reference: &WorkerWaitReference, binding: &RunBinding) -> Result<(), String> {
    binding.validate().map_err(|error| error.to_string())?;
    if binding.intent.owner != "worker-tools"
        || binding.intent.correlation != reference.as_str().replace("worker:", "worker-")
    {
        return Err("native receipt does not match its worker reference".into());
    }
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(inline)]
#[schemars(transform = reply_constant_schema)]
pub enum BudgetExhaustionKind {
    #[serde(rename = "execution_budget_exhausted")]
    Value,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(inline)]
#[schemars(transform = reply_enum_schema)]
pub enum BudgetExhaustionDimension {
    #[serde(rename = "backstop")]
    Backstop,
    #[serde(rename = "no_progress")]
    NoProgress,
    #[serde(rename = "context_tokens")]
    ContextTokens,
    #[serde(rename = "input_tokens")]
    InputTokens,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(inline)]
#[schemars(transform = reply_enum_schema)]
pub enum BudgetExhaustionLastProgressKindValue {
    #[serde(rename = "read")]
    Read,
    #[serde(rename = "grep")]
    Grep,
    #[serde(rename = "glob")]
    Glob,
    #[serde(rename = "write")]
    Write,
    #[serde(rename = "edit")]
    Edit,
    #[serde(rename = "delete")]
    Delete,
    #[serde(rename = "run_tests")]
    RunTests,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(inline)]
pub struct BudgetExhaustionToolTurns {
    pub used: ReplyNumber<0, 0, true, false, true>,
    pub allowed: ReplyNumber<0, 0, true, false, true>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(inline)]
pub struct BudgetExhaustionInputTokens {
    pub used: ReplyNumber<0, 0, true, false, true>,
    pub allowed: ReplyNumber<0, 0, true, false, true>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(inline)]
pub struct BudgetExhaustionOperation {
    pub node: ReplyNumber<0, 0, true, false, true>,
    pub generation: ReplyNumber<0, 0, true, false, true>,
    pub kind: BoundedText<1, UNBOUNDED>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(inline)]
pub struct BudgetExhaustionPreservedWork {
    pub head: ReplyText<6, 0, UNBOUNDED>,
    pub tree: ReplyText<6, 0, UNBOUNDED>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(rename = "budget_exhaustion")]
#[schemars(
    description = "Code-owned execution-budget stop, never semantic impossibility or completed proof."
)]
pub struct BudgetExhaustion {
    pub kind: BudgetExhaustionKind,
    pub dimension: BudgetExhaustionDimension,
    pub no_progress_window: ReplyNumber<0, 0, true, false, true>,
    pub no_progress_turns: ReplyNumber<0, 0, true, false, true>,
    #[serde(deserialize_with = "serde::Deserialize::deserialize")]
    pub last_progress_kind: ReplyNullable<BudgetExhaustionLastProgressKindValue>,
    pub tool_turns: BudgetExhaustionToolTurns,
    pub input_tokens: BudgetExhaustionInputTokens,
    pub operation: BudgetExhaustionOperation,
    pub preserved_work: BudgetExhaustionPreservedWork,
    pub unfinished_obligations: List<BoundedText<1, UNBOUNDED>, 0, UNBOUNDED, false>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "ReplyNumber<0, 0, true, false, true>", default)]
    pub compaction_count: Option<ReplyNumber<0, 0, true, false, true>>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "ReplyNumber<0, 0, true, false, false>", default)]
    pub context_fraction: Option<ReplyNumber<0, 0, true, false, false>>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(inline)]
#[schemars(transform = reply_constant_schema)]
pub enum CheckingHandoffStatus {
    #[serde(rename = "pending")]
    Value,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(inline)]
#[schemars(transform = reply_constant_schema)]
pub enum CheckingHandoffOwner {
    #[serde(rename = "checking")]
    Value,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(inline)]
#[schemars(transform = reply_constant_schema)]
pub enum CheckingHandoffChecksItemMode {
    #[serde(rename = "base_subject_head_observer")]
    Value,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(inline)]
pub struct CheckingHandoffChecksItem {
    pub name: BoundedText<1, UNBOUNDED>,
    pub tests: List<BoundedText<0, UNBOUNDED>, 1, UNBOUNDED, false>,
    pub mode: CheckingHandoffChecksItemMode,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(rename = "checking_handoff")]
pub struct CheckingHandoff {
    #[serde(deserialize_with = "serde::Deserialize::deserialize")]
    pub base_sha: ReplyNullChoice<ReplyText<6, 0, UNBOUNDED>>,
    pub head_sha: ReplyText<6, 0, UNBOUNDED>,
    pub head_tree: ReplyText<6, 0, UNBOUNDED>,
    pub status: CheckingHandoffStatus,
    pub owner: CheckingHandoffOwner,
    pub checks: List<CheckingHandoffChecksItem, 1, UNBOUNDED, false>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(inline)]
#[schemars(transform = reply_constant_schema)]
pub enum GenerationReceiptStatus {
    #[serde(rename = "completed")]
    Value,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(inline)]
#[schemars(transform = reply_enum_schema)]
pub enum GenerationReceiptIdentityImplementation {
    #[serde(rename = "tl3-generator-v3")]
    Tl3GeneratorV3,
    #[serde(rename = "tl3-generator-v4")]
    Tl3GeneratorV4,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(inline)]
pub struct GenerationReceiptIdentityBudget {
    pub cpus: ReplyNumber<1, 16, true, true, true>,
    pub mem_gb: ReplyNumber<1, 64, true, true, true>,
    pub timeout_s: ReplyNumber<1, 600, true, true, true>,
    pub outer_timeout_s: ReplyNumber<61, 660, true, true, true>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(inline)]
pub struct GenerationReceiptIdentity {
    pub implementation: GenerationReceiptIdentityImplementation,
    pub image: BoundedText<0, UNBOUNDED>,
    pub budget: GenerationReceiptIdentityBudget,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(inline)]
pub struct GenerationReceiptBudget {
    pub cpus: ReplyNumber<1, 16, true, true, true>,
    pub mem_gb: ReplyNumber<1, 64, true, true, true>,
    pub timeout_s: ReplyNumber<1, 600, true, true, true>,
    pub outer_timeout_s: ReplyNumber<61, 660, true, true, true>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(inline)]
#[schemars(transform = reply_enum_schema)]
pub enum GenerationReceiptTimingsPreparationOutcome {
    #[serde(rename = "completed")]
    Completed,
    #[serde(rename = "failed")]
    Failed,
    #[serde(rename = "timeout")]
    Timeout,
    #[serde(rename = "skipped")]
    Skipped,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(inline)]
#[schemars(transform = reply_constant_schema)]
pub enum GenerationReceiptTimingsPreparationScope {
    #[serde(rename = "startup_and_command")]
    Value,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(inline)]
pub struct GenerationReceiptTimingsPreparation {
    pub seconds: ReplyNumber<0, 0, true, false, false>,
    pub outcome: GenerationReceiptTimingsPreparationOutcome,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "GenerationReceiptTimingsPreparationScope", default)]
    pub scope: Option<GenerationReceiptTimingsPreparationScope>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(inline)]
#[schemars(transform = reply_enum_schema)]
pub enum GenerationReceiptTimingsContainerOutcome {
    #[serde(rename = "completed")]
    Completed,
    #[serde(rename = "failed")]
    Failed,
    #[serde(rename = "timeout")]
    Timeout,
    #[serde(rename = "skipped")]
    Skipped,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(inline)]
#[schemars(transform = reply_constant_schema)]
pub enum GenerationReceiptTimingsContainerScope {
    #[serde(rename = "startup_and_command")]
    Value,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(inline)]
pub struct GenerationReceiptTimingsContainer {
    pub seconds: ReplyNumber<0, 0, true, false, false>,
    pub outcome: GenerationReceiptTimingsContainerOutcome,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "GenerationReceiptTimingsContainerScope", default)]
    pub scope: Option<GenerationReceiptTimingsContainerScope>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(inline)]
#[schemars(transform = reply_enum_schema)]
pub enum GenerationReceiptTimingsValidationOutcome {
    #[serde(rename = "completed")]
    Completed,
    #[serde(rename = "failed")]
    Failed,
    #[serde(rename = "timeout")]
    Timeout,
    #[serde(rename = "skipped")]
    Skipped,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(inline)]
#[schemars(transform = reply_constant_schema)]
pub enum GenerationReceiptTimingsValidationScope {
    #[serde(rename = "startup_and_command")]
    Value,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(inline)]
pub struct GenerationReceiptTimingsValidation {
    pub seconds: ReplyNumber<0, 0, true, false, false>,
    pub outcome: GenerationReceiptTimingsValidationOutcome,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "GenerationReceiptTimingsValidationScope", default)]
    pub scope: Option<GenerationReceiptTimingsValidationScope>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(inline)]
#[schemars(transform = reply_enum_schema)]
pub enum GenerationReceiptTimingsCleanupOutcome {
    #[serde(rename = "completed")]
    Completed,
    #[serde(rename = "failed")]
    Failed,
    #[serde(rename = "timeout")]
    Timeout,
    #[serde(rename = "skipped")]
    Skipped,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(inline)]
#[schemars(transform = reply_constant_schema)]
pub enum GenerationReceiptTimingsCleanupScope {
    #[serde(rename = "startup_and_command")]
    Value,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(inline)]
pub struct GenerationReceiptTimingsCleanup {
    pub seconds: ReplyNumber<0, 0, true, false, false>,
    pub outcome: GenerationReceiptTimingsCleanupOutcome,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "GenerationReceiptTimingsCleanupScope", default)]
    pub scope: Option<GenerationReceiptTimingsCleanupScope>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(inline)]
pub struct GenerationReceiptTimings {
    pub preparation: GenerationReceiptTimingsPreparation,
    pub container: GenerationReceiptTimingsContainer,
    pub validation: GenerationReceiptTimingsValidation,
    pub cleanup: GenerationReceiptTimingsCleanup,
    pub total_seconds: ReplyNumber<0, 0, true, false, false>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(inline)]
#[schemars(transform = reply_constant_schema)]
pub enum GenerationReceiptPhase {
    #[serde(rename = "completed")]
    Value,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(inline)]
#[schemars(transform = reply_constant_schema)]
pub enum GenerationReceiptInnerStatus {
    #[serde(rename = "completed")]
    Value,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(inline)]
pub struct GenerationReceiptInner {
    pub status: GenerationReceiptInnerStatus,
    pub exit_code: ReplyZero,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(rename = "generation_receipt")]
pub struct GenerationReceiptFields {
    pub backend: BackendTag,
    pub wait_reference: WorkerWaitReference,
    pub run_binding: RunBinding,
    pub ending: Ending,
    pub artifact_sha256: ReplyText<5, 0, UNBOUNDED>,
    pub receipt_sha256: ReplyText<5, 0, UNBOUNDED>,
    pub output_manifest: ReplyText<5, 0, UNBOUNDED>,
    pub input_tree: ReplyText<6, 0, UNBOUNDED>,
    pub output_tree: ReplyText<6, 0, UNBOUNDED>,
    pub status: GenerationReceiptStatus,
    pub exit_code: ReplyZero,
    pub command: BoundedText<1, UNBOUNDED>,
    pub argv: List<BoundedText<0, UNBOUNDED>, 1, UNBOUNDED, false>,
    pub identity: GenerationReceiptIdentity,
    pub output_count: ReplyNumber<0, 128, true, true, true>,
    pub budget: GenerationReceiptBudget,
    pub timings: GenerationReceiptTimings,
    pub phase: GenerationReceiptPhase,
    pub inner: GenerationReceiptInner,
}

#[derive(Clone, Debug, PartialEq)]
pub struct GenerationReceiptRule;
impl Rule<GenerationReceiptFields> for GenerationReceiptRule {
    fn validate(value: &GenerationReceiptFields) -> Result<(), String> {
        validate_worker_receipt(&value.wait_reference, &value.run_binding)
    }
}
pub type GenerationReceipt = Validated<GenerationReceiptFields, GenerationReceiptRule>;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(inline)]
#[schemars(transform = reply_constant_schema)]
pub enum SelectorIdentitySchema {
    #[serde(rename = "tl3.selectors.v1")]
    Value,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(rename = "selector_identity")]
#[schemars(description = "Exact ordered selector identity; prefix is display text only.")]
pub struct SelectorIdentity {
    pub schema: SelectorIdentitySchema,
    pub count: ReplyNumber<0, 256, true, true, true>,
    pub sha256: ReplyText<5, 0, UNBOUNDED>,
    pub prefix: List<BoundedText<0, 120>, 0, 3, false>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(inline)]
#[schemars(transform = reply_constant_schema)]
pub enum TestRunStatus {
    #[serde(rename = "completed")]
    Value,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(inline)]
pub struct TestRunReceiptsFields {
    pub backend: BackendTag,
    pub wait_reference: WorkerWaitReference,
    pub run_binding: RunBinding,
    pub receipt_sha256: ReplyText<5, 0, UNBOUNDED>,
    pub ending: Ending,
}
#[derive(Clone, Debug, PartialEq)]
pub struct TestRunReceiptRule;
impl Rule<TestRunReceiptsFields> for TestRunReceiptRule {
    fn validate(value: &TestRunReceiptsFields) -> Result<(), String> {
        validate_worker_receipt(&value.wait_reference, &value.run_binding)
    }
}
pub type TestRunReceiptsItem = Validated<TestRunReceiptsFields, TestRunReceiptRule>;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(inline)]
#[schemars(transform = reply_enum_schema)]
pub enum TestRunBaseSemantics {
    #[serde(rename = "untouched_base")]
    UntouchedBase,
    #[serde(rename = "base_subject_head_observer")]
    BaseSubjectHeadObserver,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ReviewerInputJsonExternalProof(pub crate::report::ProofReceipt);
impl JsonSchema for ReviewerInputJsonExternalProof {
    fn inline_schema() -> bool {
        true
    }
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "ReviewerInputJsonExternalProof".into()
    }
    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({"$ref": "reviewer-input.json#/properties/checks/items/properties/proof"})
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(inline)]
#[schemars(transform = reply_constant_schema)]
pub enum TestRunExecutionOutcome {
    #[serde(rename = "test_timeout")]
    Value,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(inline)]
#[schemars(transform = reply_enum_schema)]
pub enum TestRunExecutionPhase {
    #[serde(rename = "test_run")]
    TestRun,
    #[serde(rename = "cleanup")]
    Cleanup,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(inline)]
#[schemars(transform = reply_constant_schema)]
pub enum TestRunExecutionTerminationReason {
    #[serde(rename = "no_progress")]
    Value,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(inline)]
pub struct TestRunExecution {
    pub outcome: TestRunExecutionOutcome,
    pub phase: TestRunExecutionPhase,
    pub termination_reason: TestRunExecutionTerminationReason,
    pub test_started: True,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "BoundedText<0, 2048>", default)]
    pub test: Option<BoundedText<0, 2048>>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(inline)]
#[schemars(transform = reply_constant_schema)]
pub enum TestRunRunsItemStatus {
    #[serde(rename = "completed")]
    Value,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(inline)]
pub struct TestRunRunsItem {
    pub run: ReplyNumber<1, 10, true, true, true>,
    pub status: TestRunRunsItemStatus,
    pub totals: Totals,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(rename = "test_run")]
#[schemars(description = "A completed run_tests call with its native Flux receipt.")]
pub struct TestRun {
    pub tests: List<BoundedText<0, 2048>, 0, 256, false>,
    pub base: bool,
    pub repeat: ReplyNumber<1, 10, true, true, true>,
    pub parallel: Parallel,
    pub status: TestRunStatus,
    #[schemars(description = "The git tree of the checkout this run tested.")]
    pub tree: ReplyText<6, 0, UNBOUNDED>,
    pub totals: Totals,
    pub receipts: List<TestRunReceiptsItem, 1, 1, false>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "SelectorIdentity", default)]
    pub tests_identity: Option<SelectorIdentity>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "ReplyNumber<1, 256, true, true, true>", default)]
    pub tests_omitted: Option<ReplyNumber<1, 256, true, true, true>>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "TestRunBaseSemantics", default)]
    pub base_semantics: Option<TestRunBaseSemantics>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "ReviewerInputJsonExternalProof", default)]
    pub proof: Option<ReviewerInputJsonExternalProof>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "TestRunExecution", default)]
    pub execution: Option<TestRunExecution>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "List<TestRunRunsItem, 0, 10, false>", default)]
    pub runs: Option<List<TestRunRunsItem, 0, 10, false>>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(rename = "totals")]
pub struct Totals {
    pub passed: ReplyNumber<0, 0, true, false, true>,
    pub failed: ReplyNumber<0, 0, true, false, true>,
    pub error: ReplyNumber<0, 0, true, false, true>,
    pub skipped: ReplyNumber<0, 0, true, false, true>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PlannerExternalProof(pub crate::checks::ProofDeclaration);
impl JsonSchema for PlannerExternalProof {
    fn inline_schema() -> bool {
        true
    }
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "PlannerExternalProof".into()
    }
    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({"$ref": "planner-reply.json#/$defs/proof"})
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(inline)]
pub struct ReproductionValue1 {
    pub finding_id: Text,
    pub tests: List<Text, 1, UNBOUNDED, true>,
    pub proof: PlannerExternalProof,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(inline)]
pub struct ReproductionValue2 {
    pub finding_id: Text,
    pub reason: Text,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
#[schemars(rename = "reproduction")]
#[schemars(transform = reply_one_of)]
pub enum Reproduction {
    Value1(ReproductionValue1),
    Value2(ReproductionValue2),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(inline)]
#[schemars(transform = reply_constant_schema)]
pub enum MakerReplyDoneKind {
    #[serde(rename = "done")]
    Value,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(inline)]
#[schemars(transform = reply_constant_schema)]
pub enum MakerReplyDoneSelfCheckItemMetStatus {
    #[serde(rename = "met")]
    Value,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(inline)]
pub struct MakerReplyDoneSelfCheckItemMet {
    pub goal_id: Text,
    pub status: MakerReplyDoneSelfCheckItemMetStatus,
    pub evidence: Text,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(inline)]
#[schemars(transform = reply_constant_schema)]
pub enum MakerReplyDoneSelfCheckItemNotMetStatus {
    #[serde(rename = "not_met")]
    Value,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(inline)]
pub struct MakerReplyDoneSelfCheckItemNotMet {
    pub goal_id: Text,
    pub status: MakerReplyDoneSelfCheckItemNotMetStatus,
    pub why: Text,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(inline)]
#[schemars(transform = reply_constant_schema)]
pub enum MakerReplyDoneSelfCheckItemPendingStatus {
    #[serde(rename = "pending")]
    Value,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(inline)]
pub struct MakerReplyDoneSelfCheckItemPending {
    pub goal_id: Text,
    pub status: MakerReplyDoneSelfCheckItemPendingStatus,
    pub why: Text,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
#[schemars(inline)]
#[schemars(transform = reply_one_of)]
pub enum MakerReplyDoneSelfCheckItem {
    Met(MakerReplyDoneSelfCheckItemMet),
    NotMet(MakerReplyDoneSelfCheckItemNotMet),
    Pending(MakerReplyDoneSelfCheckItemPending),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(inline)]
pub struct MakerReplyDone {
    pub kind: MakerReplyDoneKind,
    pub self_check: List<MakerReplyDoneSelfCheckItem, 1, UNBOUNDED, false>,
    #[schemars(description = "Added by worker code. The model must omit this field.")]
    pub head: ReplyText<6, 0, UNBOUNDED>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "BudgetExhaustion", default)]
    #[schemars(description = "Added by worker code. The model must omit this field.")]
    pub budget_exhaustion: Option<BudgetExhaustion>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "List<Reproduction, 0, UNBOUNDED, false>", default)]
    pub reproductions: Option<List<Reproduction, 0, UNBOUNDED, false>>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "List<TestRun, 1, 10, false>", default)]
    #[schemars(description = "Added by worker code. The model must omit this field.")]
    pub test_evidence: Option<List<TestRun, 1, 10, false>>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "CheckingHandoff", default)]
    #[schemars(
        description = "Added by worker code. Ordinary demonstrating proof remains pending for independent Checking. The model must omit this field."
    )]
    pub checking_handoff: Option<CheckingHandoff>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "GenerationReceipt", default)]
    #[schemars(
        description = "Added by worker code after verified import. The model must omit this field."
    )]
    pub generation_evidence: Option<GenerationReceipt>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(inline)]
#[schemars(transform = reply_constant_schema)]
pub enum MakerReplyNeedsFilesKind {
    #[serde(rename = "needs_files")]
    Value,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(inline)]
pub struct MakerReplyNeedsFilesFilesItem {
    pub path: Text,
    pub why: Text,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(inline)]
pub struct MakerReplyNeedsFiles {
    pub kind: MakerReplyNeedsFilesKind,
    pub files: List<MakerReplyNeedsFilesFilesItem, 1, UNBOUNDED, true>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "BudgetExhaustion", default)]
    #[schemars(description = "Added by worker code. The model must omit this field.")]
    pub budget_exhaustion: Option<BudgetExhaustion>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "bool", default)]
    #[schemars(description = "Added by worker code. The model must omit this field.")]
    pub fits: Option<bool>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(inline)]
#[schemars(transform = reply_constant_schema)]
pub enum MakerReplyCantMeetKind {
    #[serde(rename = "cant_meet")]
    Value,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(inline)]
#[schemars(
    description = "Only when a maker-owned goal cannot be met within the declared files and tools, the plan is defective, or evidence preflight names a truly unsupported requirement. Ordinary controller-owned checks and PR publication are pending downstream work. Not for unfinished steps your own tools can do."
)]
pub struct MakerReplyCantMeet {
    pub kind: MakerReplyCantMeetKind,
    pub report: Text,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "BudgetExhaustion", default)]
    #[schemars(description = "Added by worker code. The model must omit this field.")]
    pub budget_exhaustion: Option<BudgetExhaustion>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "List<BoundedText<1, UNBOUNDED>, 0, UNBOUNDED, false>", default)]
    pub unfinished_obligations: Option<List<BoundedText<1, UNBOUNDED>, 0, UNBOUNDED, false>>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(inline)]
#[schemars(transform = reply_constant_schema)]
pub enum MakerReplyBlockedKind {
    #[serde(rename = "blocked")]
    Value,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(inline)]
#[schemars(transform = reply_constant_schema)]
pub enum MakerReplyBlockedEvidenceValue1Class {
    #[serde(rename = "test_infrastructure")]
    Value,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(inline)]
pub struct MakerReplyBlockedEvidenceValue1 {
    pub wait_reference: WorkerWaitReference,
    #[schemars(description = "Added by worker code. The model must omit this field.")]
    pub class: MakerReplyBlockedEvidenceValue1Class,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(inline)]
#[schemars(description = "Added by worker code. The model must omit this field.")]
#[schemars(transform = reply_enum_schema)]
pub enum MakerReplyBlockedEvidenceValue2Tool {
    #[serde(rename = "list")]
    List,
    #[serde(rename = "read")]
    Read,
    #[serde(rename = "glob")]
    Glob,
    #[serde(rename = "grep")]
    Grep,
    #[serde(rename = "write")]
    Write,
    #[serde(rename = "edit")]
    Edit,
    #[serde(rename = "delete")]
    Delete,
    #[serde(rename = "run_tests")]
    RunTests,
    #[serde(rename = "run_generator")]
    RunGenerator,
    #[serde(rename = "recall")]
    Recall,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(inline)]
#[schemars(description = "Added by worker code. The model must omit this field.")]
#[schemars(transform = reply_enum_schema)]
pub enum MakerReplyBlockedEvidenceValue2Class {
    #[serde(rename = "test_submission")]
    TestSubmission,
    #[serde(rename = "EACCES")]
    Eacces,
    #[serde(rename = "EPERM")]
    Eperm,
    #[serde(rename = "EROFS")]
    Erofs,
    #[serde(rename = "ENOSPC")]
    Enospc,
    #[serde(rename = "EDQUOT")]
    Edquot,
    #[serde(rename = "EIO")]
    Eio,
    #[serde(rename = "EMFILE")]
    Emfile,
    #[serde(rename = "ENFILE")]
    Enfile,
    #[serde(rename = "ENOMEM")]
    Enomem,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(inline)]
pub struct MakerReplyBlockedEvidenceValue2 {
    pub tool_error: ReplyText<3, 0, UNBOUNDED>,
    #[schemars(description = "Added by worker code. The model must omit this field.")]
    pub tool: MakerReplyBlockedEvidenceValue2Tool,
    #[schemars(description = "Added by worker code. The model must omit this field.")]
    pub class: MakerReplyBlockedEvidenceValue2Class,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
#[schemars(inline)]
#[schemars(
    description = "A wait_reference whose run_tests summary had status infrastructure_error, or a tool_error id, recorded in this run."
)]
#[schemars(transform = reply_one_of)]
pub enum MakerReplyBlockedEvidence {
    Value1(MakerReplyBlockedEvidenceValue1),
    Value2(MakerReplyBlockedEvidenceValue2),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(inline)]
pub struct MakerReplyBlocked {
    pub kind: MakerReplyBlockedKind,
    #[schemars(
        description = "A wait_reference whose run_tests summary had status infrastructure_error, or a tool_error id, recorded in this run."
    )]
    pub evidence: MakerReplyBlockedEvidence,
    pub report: Text,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "BudgetExhaustion", default)]
    #[schemars(description = "Added by worker code. The model must omit this field.")]
    pub budget_exhaustion: Option<BudgetExhaustion>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(inline)]
#[schemars(transform = reply_constant_schema)]
pub enum MakerReplyOutOfRoomKind {
    #[serde(rename = "out_of_room")]
    Value,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(inline)]
pub struct MakerReplyOutOfRoom {
    pub kind: MakerReplyOutOfRoomKind,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "BudgetExhaustion", default)]
    #[schemars(description = "Added by worker code. The model must omit this field.")]
    pub budget_exhaustion: Option<BudgetExhaustion>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
#[schemars(inline)]
#[schemars(transform = reply_one_of)]
pub enum MakerReply {
    Done(Box<MakerReplyDone>),
    NeedsFiles(MakerReplyNeedsFiles),
    CantMeet(MakerReplyCantMeet),
    Blocked(MakerReplyBlocked),
    OutOfRoom(MakerReplyOutOfRoom),
}

pub fn schema_value() -> serde_json::Value {
    reply_schema_value::<MakerReply>("maker", |g| {
        g.subschema_for::<BudgetExhaustion>();
        g.subschema_for::<CheckingHandoff>();
        g.subschema_for::<GenerationReceipt>();
        g.subschema_for::<Text>();
        g.subschema_for::<SelectorIdentity>();
        g.subschema_for::<TestRun>();
        g.subschema_for::<Totals>();
        g.subschema_for::<Reproduction>();
    })
}

pub fn schema_text() -> String {
    let types = [
        TypeOrder::of::<BudgetExhaustion>(BUDGET_EXHAUSTION_ORDER),
        TypeOrder::of::<CheckingHandoff>(CHECKING_HANDOFF_ORDER),
        TypeOrder::of::<GenerationReceipt>(GENERATION_RECEIPT_ORDER),
        TypeOrder::of::<Text>(TEXT_ORDER),
        TypeOrder::of::<SelectorIdentity>(SELECTOR_IDENTITY_ORDER),
        TypeOrder::of::<TestRun>(TEST_RUN_ORDER),
        TypeOrder::of::<Totals>(TOTALS_ORDER),
        TypeOrder::of::<Reproduction>(REPRODUCTION_ORDER),
    ];
    let shared = [
        TypeUse {
            pointer: "/$defs/budget_exhaustion",
            order: &types[0],
        },
        TypeUse {
            pointer: "/$defs/checking_handoff",
            order: &types[1],
        },
        TypeUse {
            pointer: "/$defs/generation_receipt",
            order: &types[2],
        },
        TypeUse {
            pointer: "/$defs/text",
            order: &types[3],
        },
        TypeUse {
            pointer: "/$defs/selector_identity",
            order: &types[4],
        },
        TypeUse {
            pointer: "/$defs/test_run",
            order: &types[5],
        },
        TypeUse {
            pointer: "/$defs/totals",
            order: &types[6],
        },
        TypeUse {
            pointer: "/$defs/reproduction",
            order: &types[7],
        },
    ];
    crate::render::render(
        "maker-reply.json",
        &schema_value(),
        &Layout {
            inline: ROOT_ORDER,
            shared: &shared,
        },
    )
    .expect("published reply schema order is exhaustive")
}

pub const ROOT_ORDER: &[crate::render::ObjectOrder<'static>] = &[
    ("", &["$schema", "$id", "oneOf", "$defs"]),
    (
        "/oneOf/0",
        &["type", "properties", "required", "additionalProperties"],
    ),
    (
        "/oneOf/0/properties",
        &[
            "budget_exhaustion",
            "kind",
            "head",
            "reproductions",
            "test_evidence",
            "checking_handoff",
            "generation_evidence",
            "self_check",
        ],
    ),
    (
        "/oneOf/0/properties/budget_exhaustion",
        &["$ref", "description"],
    ),
    ("/oneOf/0/properties/kind", &["const"]),
    (
        "/oneOf/0/properties/head",
        &["type", "pattern", "description"],
    ),
    ("/oneOf/0/properties/reproductions", &["type", "items"]),
    ("/oneOf/0/properties/reproductions/items", &["$ref"]),
    (
        "/oneOf/0/properties/test_evidence",
        &["type", "minItems", "maxItems", "items", "description"],
    ),
    ("/oneOf/0/properties/test_evidence/items", &["$ref"]),
    (
        "/oneOf/0/properties/checking_handoff",
        &["$ref", "description"],
    ),
    (
        "/oneOf/0/properties/generation_evidence",
        &["$ref", "description"],
    ),
    (
        "/oneOf/0/properties/self_check",
        &["type", "minItems", "items"],
    ),
    ("/oneOf/0/properties/self_check/items", &["oneOf"]),
    (
        "/oneOf/0/properties/self_check/items/oneOf/0",
        &["type", "properties", "required", "additionalProperties"],
    ),
    (
        "/oneOf/0/properties/self_check/items/oneOf/0/properties",
        &["goal_id", "status", "evidence"],
    ),
    (
        "/oneOf/0/properties/self_check/items/oneOf/0/properties/goal_id",
        &["$ref"],
    ),
    (
        "/oneOf/0/properties/self_check/items/oneOf/0/properties/status",
        &["const"],
    ),
    (
        "/oneOf/0/properties/self_check/items/oneOf/0/properties/evidence",
        &["$ref"],
    ),
    (
        "/oneOf/0/properties/self_check/items/oneOf/1",
        &["type", "properties", "required", "additionalProperties"],
    ),
    (
        "/oneOf/0/properties/self_check/items/oneOf/1/properties",
        &["goal_id", "status", "why"],
    ),
    (
        "/oneOf/0/properties/self_check/items/oneOf/1/properties/goal_id",
        &["$ref"],
    ),
    (
        "/oneOf/0/properties/self_check/items/oneOf/1/properties/status",
        &["const"],
    ),
    (
        "/oneOf/0/properties/self_check/items/oneOf/1/properties/why",
        &["$ref"],
    ),
    (
        "/oneOf/0/properties/self_check/items/oneOf/2",
        &["type", "properties", "required", "additionalProperties"],
    ),
    (
        "/oneOf/0/properties/self_check/items/oneOf/2/properties",
        &["goal_id", "status", "why"],
    ),
    (
        "/oneOf/0/properties/self_check/items/oneOf/2/properties/goal_id",
        &["$ref"],
    ),
    (
        "/oneOf/0/properties/self_check/items/oneOf/2/properties/status",
        &["const"],
    ),
    (
        "/oneOf/0/properties/self_check/items/oneOf/2/properties/why",
        &["$ref"],
    ),
    (
        "/oneOf/1",
        &["type", "properties", "required", "additionalProperties"],
    ),
    (
        "/oneOf/1/properties",
        &["budget_exhaustion", "kind", "fits", "files"],
    ),
    (
        "/oneOf/1/properties/budget_exhaustion",
        &["$ref", "description"],
    ),
    ("/oneOf/1/properties/kind", &["const"]),
    ("/oneOf/1/properties/fits", &["type", "description"]),
    (
        "/oneOf/1/properties/files",
        &["type", "minItems", "uniqueItems", "items"],
    ),
    (
        "/oneOf/1/properties/files/items",
        &["type", "properties", "required", "additionalProperties"],
    ),
    (
        "/oneOf/1/properties/files/items/properties",
        &["path", "why"],
    ),
    ("/oneOf/1/properties/files/items/properties/path", &["$ref"]),
    ("/oneOf/1/properties/files/items/properties/why", &["$ref"]),
    (
        "/oneOf/2",
        &[
            "type",
            "description",
            "properties",
            "required",
            "additionalProperties",
        ],
    ),
    (
        "/oneOf/2/properties",
        &[
            "budget_exhaustion",
            "unfinished_obligations",
            "kind",
            "report",
        ],
    ),
    (
        "/oneOf/2/properties/budget_exhaustion",
        &["$ref", "description"],
    ),
    (
        "/oneOf/2/properties/unfinished_obligations",
        &["type", "items"],
    ),
    (
        "/oneOf/2/properties/unfinished_obligations/items",
        &["type", "minLength"],
    ),
    ("/oneOf/2/properties/kind", &["const"]),
    ("/oneOf/2/properties/report", &["$ref"]),
    (
        "/oneOf/3",
        &["type", "properties", "required", "additionalProperties"],
    ),
    (
        "/oneOf/3/properties",
        &["budget_exhaustion", "kind", "evidence", "report"],
    ),
    (
        "/oneOf/3/properties/budget_exhaustion",
        &["$ref", "description"],
    ),
    ("/oneOf/3/properties/kind", &["const"]),
    ("/oneOf/3/properties/evidence", &["description", "oneOf"]),
    (
        "/oneOf/3/properties/evidence/oneOf/0",
        &["type", "properties", "required", "additionalProperties"],
    ),
    (
        "/oneOf/3/properties/evidence/oneOf/0/properties",
        &["wait_reference", "class"],
    ),
    (
        "/oneOf/3/properties/evidence/oneOf/0/properties/wait_reference",
        &["type", "pattern"],
    ),
    (
        "/oneOf/3/properties/evidence/oneOf/0/properties/class",
        &["const", "description"],
    ),
    (
        "/oneOf/3/properties/evidence/oneOf/1",
        &["type", "properties", "required", "additionalProperties"],
    ),
    (
        "/oneOf/3/properties/evidence/oneOf/1/properties",
        &["tool_error", "tool", "class"],
    ),
    (
        "/oneOf/3/properties/evidence/oneOf/1/properties/tool_error",
        &["type", "pattern"],
    ),
    (
        "/oneOf/3/properties/evidence/oneOf/1/properties/tool",
        &["enum", "description"],
    ),
    (
        "/oneOf/3/properties/evidence/oneOf/1/properties/class",
        &["enum", "description"],
    ),
    ("/oneOf/3/properties/report", &["$ref"]),
    (
        "/oneOf/4",
        &["type", "properties", "required", "additionalProperties"],
    ),
    ("/oneOf/4/properties", &["budget_exhaustion", "kind"]),
    (
        "/oneOf/4/properties/budget_exhaustion",
        &["$ref", "description"],
    ),
    ("/oneOf/4/properties/kind", &["const"]),
    (
        "/$defs",
        &[
            "budget_exhaustion",
            "checking_handoff",
            "generation_receipt",
            "text",
            "selector_identity",
            "test_run",
            "totals",
            "reproduction",
        ],
    ),
];
pub const BUDGET_EXHAUSTION_ORDER: &[crate::render::ObjectOrder<'static>] = &[
    (
        "",
        &[
            "type",
            "description",
            "properties",
            "required",
            "additionalProperties",
        ],
    ),
    (
        "/properties",
        &[
            "kind",
            "dimension",
            "compaction_count",
            "context_fraction",
            "no_progress_window",
            "no_progress_turns",
            "last_progress_kind",
            "tool_turns",
            "input_tokens",
            "operation",
            "preserved_work",
            "unfinished_obligations",
        ],
    ),
    ("/properties/kind", &["const"]),
    ("/properties/dimension", &["enum"]),
    ("/properties/compaction_count", &["type", "minimum"]),
    ("/properties/context_fraction", &["type", "minimum"]),
    ("/properties/no_progress_window", &["type", "minimum"]),
    ("/properties/no_progress_turns", &["type", "minimum"]),
    ("/properties/last_progress_kind", &["enum"]),
    (
        "/properties/tool_turns",
        &["type", "properties", "required", "additionalProperties"],
    ),
    ("/properties/tool_turns/properties", &["used", "allowed"]),
    (
        "/properties/tool_turns/properties/used",
        &["type", "minimum"],
    ),
    (
        "/properties/tool_turns/properties/allowed",
        &["type", "minimum"],
    ),
    (
        "/properties/input_tokens",
        &["type", "properties", "required", "additionalProperties"],
    ),
    ("/properties/input_tokens/properties", &["used", "allowed"]),
    (
        "/properties/input_tokens/properties/used",
        &["type", "minimum"],
    ),
    (
        "/properties/input_tokens/properties/allowed",
        &["type", "minimum"],
    ),
    (
        "/properties/operation",
        &["type", "properties", "required", "additionalProperties"],
    ),
    (
        "/properties/operation/properties",
        &["node", "generation", "kind"],
    ),
    (
        "/properties/operation/properties/node",
        &["type", "minimum"],
    ),
    (
        "/properties/operation/properties/generation",
        &["type", "minimum"],
    ),
    (
        "/properties/operation/properties/kind",
        &["type", "minLength"],
    ),
    (
        "/properties/preserved_work",
        &["type", "properties", "required", "additionalProperties"],
    ),
    ("/properties/preserved_work/properties", &["head", "tree"]),
    (
        "/properties/preserved_work/properties/head",
        &["type", "pattern"],
    ),
    (
        "/properties/preserved_work/properties/tree",
        &["type", "pattern"],
    ),
    ("/properties/unfinished_obligations", &["type", "items"]),
    (
        "/properties/unfinished_obligations/items",
        &["type", "minLength"],
    ),
];
pub const CHECKING_HANDOFF_ORDER: &[crate::render::ObjectOrder<'static>] = &[
    (
        "",
        &["type", "properties", "required", "additionalProperties"],
    ),
    (
        "/properties",
        &[
            "base_sha",
            "head_sha",
            "head_tree",
            "status",
            "owner",
            "checks",
        ],
    ),
    ("/properties/base_sha", &["oneOf"]),
    ("/properties/base_sha/oneOf/0", &["type", "pattern"]),
    ("/properties/base_sha/oneOf/1", &["type"]),
    ("/properties/head_sha", &["type", "pattern"]),
    ("/properties/head_tree", &["type", "pattern"]),
    ("/properties/status", &["const"]),
    ("/properties/owner", &["const"]),
    ("/properties/checks", &["type", "minItems", "items"]),
    (
        "/properties/checks/items",
        &["type", "properties", "required", "additionalProperties"],
    ),
    (
        "/properties/checks/items/properties",
        &["name", "tests", "mode"],
    ),
    (
        "/properties/checks/items/properties/name",
        &["type", "minLength"],
    ),
    (
        "/properties/checks/items/properties/tests",
        &["type", "minItems", "items"],
    ),
    ("/properties/checks/items/properties/tests/items", &["type"]),
    ("/properties/checks/items/properties/mode", &["const"]),
];
pub const GENERATION_RECEIPT_ORDER: &[crate::render::ObjectOrder<'static>] = &[
    (
        "",
        &["type", "properties", "required", "additionalProperties"],
    ),
    (
        "/properties",
        &[
            "backend",
            "wait_reference",
            "run_binding",
            "ending",
            "artifact_sha256",
            "receipt_sha256",
            "output_manifest",
            "input_tree",
            "output_tree",
            "status",
            "exit_code",
            "command",
            "argv",
            "identity",
            "output_count",
            "budget",
            "timings",
            "phase",
            "inner",
        ],
    ),
    ("/properties/backend", &["type", "enum"]),
    ("/properties/wait_reference", &["type", "pattern"]),
    ("/properties/run_binding", &["$ref"]),
    ("/properties/ending", &["type", "enum"]),
    ("/properties/artifact_sha256", &["type", "pattern"]),
    ("/properties/receipt_sha256", &["type", "pattern"]),
    ("/properties/output_manifest", &["type", "pattern"]),
    ("/properties/input_tree", &["type", "pattern"]),
    ("/properties/output_tree", &["type", "pattern"]),
    ("/properties/status", &["const"]),
    ("/properties/exit_code", &["const"]),
    ("/properties/command", &["type", "minLength"]),
    ("/properties/argv", &["type", "minItems", "items"]),
    ("/properties/argv/items", &["type"]),
    (
        "/properties/identity",
        &["type", "properties", "required", "additionalProperties"],
    ),
    (
        "/properties/identity/properties",
        &["implementation", "image", "budget"],
    ),
    ("/properties/identity/properties/implementation", &["enum"]),
    ("/properties/identity/properties/image", &["type"]),
    (
        "/properties/identity/properties/budget",
        &["type", "properties", "required", "additionalProperties"],
    ),
    (
        "/properties/identity/properties/budget/properties",
        &["cpus", "mem_gb", "timeout_s", "outer_timeout_s"],
    ),
    (
        "/properties/identity/properties/budget/properties/cpus",
        &["type", "minimum", "maximum"],
    ),
    (
        "/properties/identity/properties/budget/properties/mem_gb",
        &["type", "minimum", "maximum"],
    ),
    (
        "/properties/identity/properties/budget/properties/timeout_s",
        &["type", "minimum", "maximum"],
    ),
    (
        "/properties/identity/properties/budget/properties/outer_timeout_s",
        &["type", "minimum", "maximum"],
    ),
    ("/properties/output_count", &["type", "minimum", "maximum"]),
    (
        "/properties/budget",
        &["type", "properties", "required", "additionalProperties"],
    ),
    (
        "/properties/budget/properties",
        &["cpus", "mem_gb", "timeout_s", "outer_timeout_s"],
    ),
    (
        "/properties/budget/properties/cpus",
        &["type", "minimum", "maximum"],
    ),
    (
        "/properties/budget/properties/mem_gb",
        &["type", "minimum", "maximum"],
    ),
    (
        "/properties/budget/properties/timeout_s",
        &["type", "minimum", "maximum"],
    ),
    (
        "/properties/budget/properties/outer_timeout_s",
        &["type", "minimum", "maximum"],
    ),
    (
        "/properties/timings",
        &["type", "properties", "required", "additionalProperties"],
    ),
    (
        "/properties/timings/properties",
        &[
            "preparation",
            "container",
            "validation",
            "cleanup",
            "total_seconds",
        ],
    ),
    (
        "/properties/timings/properties/preparation",
        &["type", "properties", "required", "additionalProperties"],
    ),
    (
        "/properties/timings/properties/preparation/properties",
        &["seconds", "outcome", "scope"],
    ),
    (
        "/properties/timings/properties/preparation/properties/seconds",
        &["type", "minimum"],
    ),
    (
        "/properties/timings/properties/preparation/properties/outcome",
        &["enum"],
    ),
    (
        "/properties/timings/properties/preparation/properties/scope",
        &["const"],
    ),
    (
        "/properties/timings/properties/container",
        &["type", "properties", "required", "additionalProperties"],
    ),
    (
        "/properties/timings/properties/container/properties",
        &["seconds", "outcome", "scope"],
    ),
    (
        "/properties/timings/properties/container/properties/seconds",
        &["type", "minimum"],
    ),
    (
        "/properties/timings/properties/container/properties/outcome",
        &["enum"],
    ),
    (
        "/properties/timings/properties/container/properties/scope",
        &["const"],
    ),
    (
        "/properties/timings/properties/validation",
        &["type", "properties", "required", "additionalProperties"],
    ),
    (
        "/properties/timings/properties/validation/properties",
        &["seconds", "outcome", "scope"],
    ),
    (
        "/properties/timings/properties/validation/properties/seconds",
        &["type", "minimum"],
    ),
    (
        "/properties/timings/properties/validation/properties/outcome",
        &["enum"],
    ),
    (
        "/properties/timings/properties/validation/properties/scope",
        &["const"],
    ),
    (
        "/properties/timings/properties/cleanup",
        &["type", "properties", "required", "additionalProperties"],
    ),
    (
        "/properties/timings/properties/cleanup/properties",
        &["seconds", "outcome", "scope"],
    ),
    (
        "/properties/timings/properties/cleanup/properties/seconds",
        &["type", "minimum"],
    ),
    (
        "/properties/timings/properties/cleanup/properties/outcome",
        &["enum"],
    ),
    (
        "/properties/timings/properties/cleanup/properties/scope",
        &["const"],
    ),
    (
        "/properties/timings/properties/total_seconds",
        &["type", "minimum"],
    ),
    ("/properties/phase", &["const"]),
    (
        "/properties/inner",
        &["type", "properties", "required", "additionalProperties"],
    ),
    ("/properties/inner/properties", &["status", "exit_code"]),
    ("/properties/inner/properties/status", &["const"]),
    ("/properties/inner/properties/exit_code", &["const"]),

];
pub const SELECTOR_IDENTITY_ORDER: &[crate::render::ObjectOrder<'static>] = &[
    (
        "",
        &[
            "type",
            "description",
            "properties",
            "required",
            "additionalProperties",
        ],
    ),
    ("/properties", &["schema", "count", "sha256", "prefix"]),
    ("/properties/schema", &["const"]),
    ("/properties/count", &["type", "minimum", "maximum"]),
    ("/properties/sha256", &["type", "pattern"]),
    ("/properties/prefix", &["type", "maxItems", "items"]),
    ("/properties/prefix/items", &["type", "maxLength"]),
];
pub const TEST_RUN_ORDER: &[crate::render::ObjectOrder<'static>] = &[
    (
        "",
        &[
            "type",
            "description",
            "properties",
            "required",
            "additionalProperties",
        ],
    ),
    (
        "/properties",
        &[
            "tests",
            "tests_identity",
            "tests_omitted",
            "base",
            "base_semantics",
            "proof",
            "execution",
            "tree",
            "repeat",
            "parallel",
            "status",
            "totals",
            "receipts",
            "runs",
        ],
    ),
    ("/properties/tests", &["type", "maxItems", "items"]),
    ("/properties/tests/items", &["type", "maxLength"]),
    ("/properties/tests_identity", &["$ref"]),
    ("/properties/tests_omitted", &["type", "minimum", "maximum"]),
    ("/properties/base", &["type"]),
    ("/properties/base_semantics", &["enum"]),
    ("/properties/proof", &["$ref"]),
    (
        "/properties/execution",
        &["type", "properties", "required", "additionalProperties"],
    ),
    (
        "/properties/execution/properties",
        &[
            "outcome",
            "phase",
            "termination_reason",
            "test_started",
            "test",
        ],
    ),
    ("/properties/execution/properties/outcome", &["const"]),
    ("/properties/execution/properties/phase", &["enum"]),
    (
        "/properties/execution/properties/termination_reason",
        &["const"],
    ),
    ("/properties/execution/properties/test_started", &["const"]),
    (
        "/properties/execution/properties/test",
        &["type", "maxLength"],
    ),
    ("/properties/tree", &["type", "pattern", "description"]),
    ("/properties/repeat", &["type", "minimum", "maximum"]),
    ("/properties/parallel", &["enum"]),
    ("/properties/status", &["const"]),
    ("/properties/totals", &["$ref"]),
    (
        "/properties/receipts",
        &["type", "minItems", "maxItems", "items"],
    ),
    (
        "/properties/receipts/items",
        &["type", "properties", "required", "additionalProperties"],
    ),
    (
        "/properties/receipts/items/properties",
        &["backend", "wait_reference", "run_binding", "receipt_sha256", "ending"],
    ),
    ("/properties/receipts/items/properties/backend", &["type", "enum"]),
    ("/properties/receipts/items/properties/wait_reference", &["type", "pattern"]),
    ("/properties/receipts/items/properties/run_binding", &["$ref"]),
    ("/properties/receipts/items/properties/receipt_sha256", &["type", "pattern"]),
    ("/properties/receipts/items/properties/ending", &["type", "enum"]),
    ("/properties/runs", &["type", "maxItems", "items"]),
    (
        "/properties/runs/items",
        &["type", "properties", "required", "additionalProperties"],
    ),
    (
        "/properties/runs/items/properties",
        &["run", "status", "totals"],
    ),
    (
        "/properties/runs/items/properties/run",
        &["type", "minimum", "maximum"],
    ),
    ("/properties/runs/items/properties/status", &["const"]),
    ("/properties/runs/items/properties/totals", &["$ref"]),
];
pub const TOTALS_ORDER: &[crate::render::ObjectOrder<'static>] = &[
    (
        "",
        &["type", "properties", "required", "additionalProperties"],
    ),
    ("/properties", &["passed", "failed", "error", "skipped"]),
    ("/properties/passed", &["type", "minimum"]),
    ("/properties/failed", &["type", "minimum"]),
    ("/properties/error", &["type", "minimum"]),
    ("/properties/skipped", &["type", "minimum"]),
];
pub const REPRODUCTION_ORDER: &[crate::render::ObjectOrder<'static>] = &[
    ("", &["oneOf"]),
    (
        "/oneOf/0",
        &["type", "properties", "required", "additionalProperties"],
    ),
    ("/oneOf/0/properties", &["finding_id", "tests", "proof"]),
    ("/oneOf/0/properties/finding_id", &["$ref"]),
    (
        "/oneOf/0/properties/tests",
        &["type", "minItems", "uniqueItems", "items"],
    ),
    ("/oneOf/0/properties/tests/items", &["$ref"]),
    ("/oneOf/0/properties/proof", &["$ref"]),
    (
        "/oneOf/1",
        &["type", "properties", "required", "additionalProperties"],
    ),
    ("/oneOf/1/properties", &["finding_id", "reason"]),
    ("/oneOf/1/properties/finding_id", &["$ref"]),
    ("/oneOf/1/properties/reason", &["$ref"]),
];

#[cfg(test)]
mod native_receipt_tests {
    use super::*;
    use serde_json::{Value, json};

    fn receipt() -> Value {
        json!({
            "backend": "flux",
            "wait_reference": format!("worker:{}", "a".repeat(64)),
            "run_binding": {
                "id": {"backend": "flux", "instance": "worker-run",
                    "uri": "local:///fixture", "job": "f2"},
                "intent": {"backend": "flux", "instance": "worker-run",
                    "uri": "local:///fixture", "owner": "worker-tools",
                    "correlation": format!("worker-{}", "a".repeat(64)),
                    "request_digest": "b".repeat(64), "output_dir": "/fixture/output"}
            },
            "receipt_sha256": "c".repeat(64),
            "ending": "passed"
        })
    }

    #[test]
    fn native_receipt_requires_exact_fields_and_binding_closure() {
        let valid = receipt();
        let admitted: TestRunReceiptsItem = serde_json::from_value(valid.clone()).unwrap();
        assert_eq!(serde_json::to_value(admitted).unwrap(), valid);
        for key in ["backend", "wait_reference", "run_binding", "receipt_sha256", "ending"] {
            let mut missing = valid.clone();
            missing.as_object_mut().unwrap().remove(key);
            assert!(serde_json::from_value::<TestRunReceiptsItem>(missing).is_err(), "{key}");
        }
        for key in ["action_key", "status", "exit_code", "log_sha256", "attempt", "generation"] {
            let mut obsolete = valid.clone();
            obsolete[key] = json!(0);
            assert!(serde_json::from_value::<TestRunReceiptsItem>(obsolete).is_err(), "{key}");
        }
        for (pointer, invalid) in [
            ("/backend", json!("prismabuild")),
            ("/wait_reference", json!("a".repeat(64))),
            ("/wait_reference", json!(format!("worker:{}", "A".repeat(64)))),
            ("/receipt_sha256", json!("C".repeat(64))),
            ("/ending", json!("executed")),
            ("/run_binding/id/backend", json!("prismabuild")),
            ("/run_binding/id/instance", json!("other")),
            ("/run_binding/id/uri", json!("local:///other")),
            ("/run_binding/id/job", json!("18446744073709551616")),
            ("/run_binding/intent/owner", json!("other")),
            ("/run_binding/intent/correlation", json!("other")),
            ("/run_binding/intent/request_digest", json!("short")),
            ("/run_binding/intent/output_dir", json!("relative/output")),
        ] {
            let mut malformed = valid.clone();
            *malformed.pointer_mut(pointer).unwrap() = invalid;
            assert!(serde_json::from_value::<TestRunReceiptsItem>(malformed).is_err(), "{pointer}");
        }
    }

    #[test]
    fn blocked_native_reference_rejects_old_keys_and_malformed_references() {
        let valid = json!({"wait_reference": format!("worker:{}", "a".repeat(64)),
            "class": "test_infrastructure"});
        assert!(serde_json::from_value::<MakerReplyBlockedEvidenceValue1>(valid.clone()).is_ok());
        for reference in ["a".repeat(64), format!("worker:{}", "A".repeat(64)), "worker:short".into()] {
            let mut invalid = valid.clone();
            invalid["wait_reference"] = json!(reference);
            assert!(serde_json::from_value::<MakerReplyBlockedEvidenceValue1>(invalid).is_err());
        }
        let obsolete = json!({"action_key": "a".repeat(64), "class": "test_infrastructure"});
        assert!(serde_json::from_value::<MakerReplyBlockedEvidenceValue1>(obsolete).is_err());
    }

    fn generation() -> Value {
        let mut value = receipt();
        let object = value.as_object_mut().unwrap();
        let budget = json!({"cpus": 1, "mem_gb": 1, "timeout_s": 30, "outer_timeout_s": 90});
        for (key, field) in [
            ("artifact_sha256", json!("d".repeat(64))),
            ("output_manifest", json!("e".repeat(64))),
            ("input_tree", json!("1".repeat(40))),
            ("output_tree", json!("2".repeat(40))),
            ("status", json!("completed")),
            ("exit_code", json!(0)),
            ("command", json!("python3 tools/gen.py")),
            ("argv", json!(["python3", "tools/gen.py"])),
            ("identity", json!({"implementation": "tl3-generator-v4",
                "image": format!("sha256:{}", "b".repeat(64)), "budget": budget.clone()})),
            ("output_count", json!(1)),
            ("budget", budget),
            ("timings", json!({
                "preparation": {"seconds": 0.1, "outcome": "completed"},
                "container": {"seconds": 1.0, "outcome": "completed", "scope": "startup_and_command"},
                "validation": {"seconds": 0.1, "outcome": "completed"},
                "cleanup": {"seconds": 0.1, "outcome": "completed"},
                "total_seconds": 1.3})),
            ("phase", json!("completed")),
            ("inner", json!({"status": "completed", "exit_code": 0})),
        ] {
            object.insert(key.into(), field);
        }
        value
    }

    /// The authentic FX-GEN-PREP failed artifact (worker-artifact.json sha256 b37f9a34…,
    /// source 80ddef3e) carried with its native failed terminal.
    fn fx_failed_artifact() -> Value {
        json!({
            "status": "failed", "exit_code": null,
            "message": "Generator action infrastructure failed.",
            "command": "python3 tools/gen.py", "argv": ["python3", "tools/gen.py"],
            "phase": "preparation",
            "budget": {"cpus": 1, "mem_gb": 1, "timeout_s": 30, "outer_timeout_s": 90},
            "identity": {"implementation": "tl3-generator-v4",
                "image": format!("sha256:{}", "b".repeat(64)),
                "budget": {"cpus": 1, "mem_gb": 1, "timeout_s": 30, "outer_timeout_s": 90}},
            "timings": {
                "preparation": {"seconds": 0.039531893911771476, "outcome": "failed"},
                "container": {"seconds": 0.0, "outcome": "skipped", "scope": "startup_and_command"},
                "validation": {"seconds": 0.0, "outcome": "skipped"},
                "cleanup": {"seconds": 0.004997374024242163, "outcome": "completed"},
                "total_seconds": 0.048817435977980494},
            "cause": {"operation": "input_manifest", "type": "PermissionError",
                "errno": 13, "detail": "Permission denied"}
        })
    }

    fn done(evidence: Value) -> Value {
        json!({"kind": "done", "head": "3".repeat(40),
            "self_check": [{"goal_id": "build", "status": "met", "evidence": "Model work finished."}],
            "generation_evidence": evidence})
    }

    #[test]
    fn completed_generation_receipt_is_native_and_requires_a_passed_ending() {
        let valid = generation();
        let admitted: GenerationReceipt = serde_json::from_value(valid.clone()).unwrap();
        assert_eq!(serde_json::to_value(admitted).unwrap(), valid);
        assert!(serde_json::from_value::<MakerReply>(done(valid.clone())).is_ok());
        for obsolete in ["action_key", "outer"] {
            let mut stale = valid.clone();
            stale[obsolete] = json!({"status": "executed", "exit_code": 0, "elapsed_s": null});
            assert!(serde_json::from_value::<GenerationReceipt>(stale).is_err(), "{obsolete}");
        }
        // A completed artifact can never stand behind a non-passing native terminal.
        for ending in ["failed", "refused", "infra-lost", "cancelled"] {
            let mut mismatched = valid.clone();
            mismatched["ending"] = json!(ending);
            assert!(serde_json::from_value::<GenerationReceipt>(mismatched.clone()).is_err(), "{ending}");
            assert!(serde_json::from_value::<MakerReply>(done(mismatched)).is_err(), "{ending}");
        }
        let mut foreign = valid.clone();
        foreign["run_binding"]["intent"]["correlation"] = json!(format!("worker-{}", "f".repeat(64)));
        assert!(serde_json::from_value::<GenerationReceipt>(foreign).is_err());
    }

    #[test]
    fn typed_failed_generator_preparation_never_becomes_completed_generation_evidence() {
        let failed = fx_failed_artifact();
        // Every authentic failure fact alone keeps the completed-only receipt closed: no
        // field is widened, nulled or zero-filled to admit the failure.
        for (pointer, authentic) in [
            ("/ending", json!("failed")),
            ("/status", failed["status"].clone()),
            ("/exit_code", failed["exit_code"].clone()),
            ("/phase", failed["phase"].clone()),
            ("/inner", json!({"status": "failed", "exit_code": null})),
        ] {
            let mut projected = generation();
            *projected.pointer_mut(pointer).unwrap() = authentic;
            assert!(serde_json::from_value::<GenerationReceipt>(projected).is_err(), "{pointer}");
        }
        for (key, extra) in [("cause", failed["cause"].clone()), ("message", failed["message"].clone())] {
            let mut projected = generation();
            projected[key] = extra;
            assert!(serde_json::from_value::<GenerationReceipt>(projected).is_err(), "{key}");
        }
        // The whole authentic shape: no outputs, failed terminal, wholesale projection.
        let mut projected = generation();
        for key in ["output_manifest", "output_tree", "output_count"] {
            projected.as_object_mut().unwrap().remove(key);
        }
        for (key, value) in failed.as_object().unwrap() {
            projected[key.as_str()] = value.clone();
        }
        projected["ending"] = json!("failed");
        projected["inner"] = json!({"status": "failed", "exit_code": null});
        assert!(serde_json::from_value::<GenerationReceipt>(projected.clone()).is_err());
        assert!(serde_json::from_value::<MakerReply>(done(projected)).is_err());
    }

    #[test]
    fn typed_failed_generator_routes_through_cant_meet_without_head_or_generation_evidence() {
        let report = format!(
            "Generation failed before final head: Generator action infrastructure failed. \
             {{\"ending\": \"failed\", \"phase\": \"preparation\", \"status\": \"failed\", \
             \"wait_reference\": \"worker:{}\"}}",
            "a".repeat(64)
        );
        let cant_meet = json!({"kind": "cant_meet", "report": report});
        let admitted: MakerReply = serde_json::from_value(cant_meet.clone()).unwrap();
        assert!(matches!(admitted, MakerReply::CantMeet(_)));
        assert_eq!(serde_json::to_value(admitted).unwrap(), cant_meet);
        // cant_meet never carries a final head or any generation evidence, completed or not.
        for (key, value) in [
            ("head", json!("3".repeat(40))),
            ("generation_evidence", generation()),
            ("generation_evidence", fx_failed_artifact()),
            ("generation_failure", fx_failed_artifact()),
        ] {
            let mut widened = cant_meet.clone();
            widened[key] = value;
            assert!(serde_json::from_value::<MakerReply>(widened).is_err(), "{key}");
        }
    }
}
