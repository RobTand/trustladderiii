//! Maker reply wire contract (#730 C3).
use super::wire::*;

wire_choice!(BudgetExhaustionKind, "const", { Value => "execution_budget_exhausted" });

wire_choice!(BudgetExhaustionDimension, "enum", {Backstop => "backstop",NoProgress => "no_progress",ContextTokens => "context_tokens",InputTokens => "input_tokens"});

wire_choice!(BudgetExhaustionLastProgressKindChoice, "enum", {Read => "read",Grep => "grep",Glob => "glob",Write => "write",Edit => "edit",RunTests => "run_tests"});

wire_nullable_choice!(
    BudgetExhaustionLastProgressKind,
    BudgetExhaustionLastProgressKindChoice
);

wire_object!(BudgetExhaustionToolTurns,["used","allowed"],["type","properties","required","additionalProperties"],vec![],{
used: Whole<0,{ u64::MAX }>, ["type","minimum"], vec![];
allowed: Whole<0,{ u64::MAX }>, ["type","minimum"], vec![];
});

wire_object!(BudgetExhaustionOperation,["node","generation","kind"],["type","properties","required","additionalProperties"],vec![],{
node: Whole<0,{ u64::MAX }>, ["type","minimum"], vec![];
generation: Whole<0,{ u64::MAX }>, ["type","minimum"], vec![];
kind: Text<1,UNBOUNDED>, ["type","minLength"], vec![];
});

wire_text!(
    BudgetExhaustionPreservedWorkHead,
    0,
    UNBOUNDED,
    "^(?:[0-9a-f]{40}|[0-9a-f]{64})$",
    ""
);

wire_object!(BudgetExhaustionPreservedWork,["head","tree"],["type","properties","required","additionalProperties"],vec![],{
head: BudgetExhaustionPreservedWorkHead, ["type","pattern"], vec![];
tree: BudgetExhaustionPreservedWorkHead, ["type","pattern"], vec![];
});

wire_object!(BudgetExhaustion,["kind","dimension","no_progress_window","no_progress_turns","last_progress_kind","tool_turns","input_tokens","operation","preserved_work","unfinished_obligations"],["type","description","properties","required","additionalProperties"],vec![("description",Doc::atom("Code-owned execution-budget stop, never semantic impossibility or completed proof."))],{
kind: BudgetExhaustionKind, ["const"], vec![];
dimension: BudgetExhaustionDimension, ["enum"], vec![];
#[serde(default,deserialize_with="present",skip_serializing_if="Option::is_none")]
compaction_count: Option<Whole<0,{ u64::MAX }>>, ["type","minimum"], vec![];
#[serde(default,deserialize_with="present",skip_serializing_if="Option::is_none")]
context_fraction: Option<NonNegative>, ["type","minimum"], vec![];
no_progress_window: Whole<0,{ u64::MAX }>, ["type","minimum"], vec![];
no_progress_turns: Whole<0,{ u64::MAX }>, ["type","minimum"], vec![];
last_progress_kind: BudgetExhaustionLastProgressKind, ["enum"], vec![];
tool_turns: BudgetExhaustionToolTurns, ["type","properties","required","additionalProperties"], vec![];
input_tokens: BudgetExhaustionToolTurns, ["type","properties","required","additionalProperties"], vec![];
operation: BudgetExhaustionOperation, ["type","properties","required","additionalProperties"], vec![];
preserved_work: BudgetExhaustionPreservedWork, ["type","properties","required","additionalProperties"], vec![];
unfinished_obligations: List<Text<1,UNBOUNDED>,0,UNBOUNDED,false>, ["type","items"], vec![];
});

wire_union!(CheckingHandoffBaseSha, {Variant1(BudgetExhaustionPreservedWorkHead),Variant2(())});

wire_choice!(CheckingHandoffStatus, "const", { Value => "pending" });

wire_choice!(CheckingHandoffOwner, "const", { Value => "checking" });

wire_choice!(CheckingHandoffChecksItemMode, "const", { Value => "base_subject_head_observer" });

wire_object!(CheckingHandoffChecksItem,["name","tests","mode"],["type","properties","required","additionalProperties"],vec![],{
name: Text<1,UNBOUNDED>, ["type","minLength"], vec![];
tests: List<Text<0,UNBOUNDED>,1,UNBOUNDED,false>, ["type","minItems","items"], vec![];
mode: CheckingHandoffChecksItemMode, ["const"], vec![];
});

wire_object!(CheckingHandoff,["base_sha","head_sha","head_tree","status","owner","checks"],["type","properties","required","additionalProperties"],vec![],{
base_sha: CheckingHandoffBaseSha, ["oneOf"], vec![];
head_sha: BudgetExhaustionPreservedWorkHead, ["type","pattern"], vec![];
head_tree: BudgetExhaustionPreservedWorkHead, ["type","pattern"], vec![];
status: CheckingHandoffStatus, ["const"], vec![];
owner: CheckingHandoffOwner, ["const"], vec![];
checks: List<CheckingHandoffChecksItem,1,UNBOUNDED,false>, ["type","minItems","items"], vec![];
});

wire_text!(
    GenerationReceiptActionKey,
    0,
    UNBOUNDED,
    "^[0-9a-f]{64}$",
    ""
);

wire_choice!(GenerationReceiptStatus, "const", { Value => "completed" });

wire_zero!(GenerationReceiptExitCode);

wire_choice!(GenerationReceiptIdentityImplementation, "enum", {Tl3GeneratorV3 => "tl3-generator-v3",Tl3GeneratorV4 => "tl3-generator-v4"});

wire_object!(GenerationReceiptIdentityBudget,["cpus","mem_gb","timeout_s","outer_timeout_s"],["type","properties","required","additionalProperties"],vec![],{
cpus: Whole<1,16>, ["type","minimum","maximum"], vec![];
mem_gb: Whole<1,64>, ["type","minimum","maximum"], vec![];
timeout_s: Whole<1,600>, ["type","minimum","maximum"], vec![];
outer_timeout_s: Whole<61,660>, ["type","minimum","maximum"], vec![];
});

wire_object!(GenerationReceiptIdentity,["implementation","image","budget"],["type","properties","required","additionalProperties"],vec![],{
implementation: GenerationReceiptIdentityImplementation, ["enum"], vec![];
image: Text<0,UNBOUNDED>, ["type"], vec![];
budget: GenerationReceiptIdentityBudget, ["type","properties","required","additionalProperties"], vec![];
});

wire_choice!(GenerationReceiptTimingsPreparationOutcome, "enum", {Completed => "completed",Failed => "failed",Timeout => "timeout",Skipped => "skipped"});

wire_choice!(GenerationReceiptTimingsPreparationScope, "const", { Value => "startup_and_command" });

wire_object!(GenerationReceiptTimingsPreparation,["seconds","outcome"],["type","properties","required","additionalProperties"],vec![],{
seconds: NonNegative, ["type","minimum"], vec![];
outcome: GenerationReceiptTimingsPreparationOutcome, ["enum"], vec![];
#[serde(default,deserialize_with="present",skip_serializing_if="Option::is_none")]
scope: Option<GenerationReceiptTimingsPreparationScope>, ["const"], vec![];
});

wire_object!(GenerationReceiptTimings,["preparation","container","validation","cleanup","total_seconds"],["type","properties","required","additionalProperties"],vec![],{
preparation: GenerationReceiptTimingsPreparation, ["type","properties","required","additionalProperties"], vec![];
container: GenerationReceiptTimingsPreparation, ["type","properties","required","additionalProperties"], vec![];
validation: GenerationReceiptTimingsPreparation, ["type","properties","required","additionalProperties"], vec![];
cleanup: GenerationReceiptTimingsPreparation, ["type","properties","required","additionalProperties"], vec![];
total_seconds: NonNegative, ["type","minimum"], vec![];
});

wire_object!(GenerationReceiptInner,["status","exit_code"],["type","properties","required","additionalProperties"],vec![],{
status: GenerationReceiptStatus, ["const"], vec![];
exit_code: GenerationReceiptExitCode, ["const"], vec![];
});

wire_choice!(GenerationReceiptOuterStatus, "enum", {Executed => "executed",CacheHit => "cache_hit"});

wire_object!(GenerationReceiptOuter,["status","exit_code","elapsed_s"],["type","properties","required","additionalProperties"],vec![],{
status: GenerationReceiptOuterStatus, ["enum"], vec![];
exit_code: GenerationReceiptExitCode, ["const"], vec![];
elapsed_s: Maybe<NonNegative>, ["type","minimum"], vec![];
});

wire_object!(GenerationReceipt,["action_key","artifact_sha256","receipt_sha256","output_manifest","input_tree","output_tree","status","exit_code","command","argv","identity","output_count","budget","timings","phase","inner","outer"],["type","properties","required","additionalProperties"],vec![],{
action_key: GenerationReceiptActionKey, ["type","pattern"], vec![];
artifact_sha256: GenerationReceiptActionKey, ["type","pattern"], vec![];
receipt_sha256: GenerationReceiptActionKey, ["type","pattern"], vec![];
output_manifest: GenerationReceiptActionKey, ["type","pattern"], vec![];
input_tree: BudgetExhaustionPreservedWorkHead, ["type","pattern"], vec![];
output_tree: BudgetExhaustionPreservedWorkHead, ["type","pattern"], vec![];
status: GenerationReceiptStatus, ["const"], vec![];
exit_code: GenerationReceiptExitCode, ["const"], vec![];
command: Text<1,UNBOUNDED>, ["type","minLength"], vec![];
argv: List<Text<0,UNBOUNDED>,1,UNBOUNDED,false>, ["type","minItems","items"], vec![];
identity: GenerationReceiptIdentity, ["type","properties","required","additionalProperties"], vec![];
output_count: Whole<0,128>, ["type","minimum","maximum"], vec![];
budget: GenerationReceiptIdentityBudget, ["type","properties","required","additionalProperties"], vec![];
timings: GenerationReceiptTimings, ["type","properties","required","additionalProperties"], vec![];
phase: GenerationReceiptStatus, ["const"], vec![];
inner: GenerationReceiptInner, ["type","properties","required","additionalProperties"], vec![];
outer: GenerationReceiptOuter, ["type","properties","required","additionalProperties"], vec![];
});

wire_text!(WireText, 1, UNBOUNDED, "\\S", "");

wire_choice!(SelectorIdentitySchema, "const", { Value => "tl3.selectors.v1" });

wire_object!(SelectorIdentity,["schema","count","sha256","prefix"],["type","description","properties","required","additionalProperties"],vec![("description",Doc::atom("Exact ordered selector identity; prefix is display text only."))],{
schema: SelectorIdentitySchema, ["const"], vec![];
count: Whole<0,256>, ["type","minimum","maximum"], vec![];
sha256: GenerationReceiptActionKey, ["type","pattern"], vec![];
prefix: List<Text<0,120>,0,3,false>, ["type","maxItems","items"], vec![];
});

wire_link!(
    TestRunTestsIdentity,
    super::maker::SelectorIdentity,
    "#/$defs/selector_identity"
);

wire_choice!(TestRunBaseSemantics, "enum", {UntouchedBase => "untouched_base",BaseSubjectHeadObserver => "base_subject_head_observer"});

wire_link!(
    TestRunProof,
    super::super::report::ProofReceipt,
    "reviewer-input.json#/properties/checks/items/properties/proof"
);

wire_choice!(TestRunExecutionOutcome, "const", { Value => "test_timeout" });

wire_choice!(TestRunExecutionPhase, "enum", {TestRun => "test_run",Cleanup => "cleanup"});

wire_choice!(TestRunExecutionTerminationReason, "const", { Value => "no_progress" });

wire_object!(TestRunExecution,["outcome","phase","termination_reason","test_started"],["type","properties","required","additionalProperties"],vec![],{
outcome: TestRunExecutionOutcome, ["const"], vec![];
phase: TestRunExecutionPhase, ["enum"], vec![];
termination_reason: TestRunExecutionTerminationReason, ["const"], vec![];
test_started: True, ["const"], vec![];
#[serde(default,deserialize_with="present",skip_serializing_if="Option::is_none")]
test: Option<Text<0,2048>>, ["type","maxLength"], vec![];
});

wire_text!(
    TestRunTree,
    0,
    UNBOUNDED,
    "^(?:[0-9a-f]{40}|[0-9a-f]{64})$",
    ""
);

wire_link!(TestRunTotals, super::maker::Totals, "#/$defs/totals");

wire_object!(TestRunReceiptsItem,["action_key","status","exit_code","receipt_sha256","log_sha256"],["type","properties","required","additionalProperties"],vec![],{
action_key: GenerationReceiptActionKey, ["type","pattern"], vec![];
status: GenerationReceiptOuterStatus, ["enum"], vec![];
exit_code: Whole<{ i64::MIN },{ u64::MAX }>, ["type"], vec![];
#[serde(default,deserialize_with="present",skip_serializing_if="Option::is_none")]
attempt: Option<Whole<1,{ u64::MAX }>>, ["type","minimum"], vec![];
#[serde(default,deserialize_with="present",skip_serializing_if="Option::is_none")]
generation: Option<GenerationReceiptActionKey>, ["type","pattern"], vec![];
receipt_sha256: GenerationReceiptActionKey, ["type","pattern"], vec![];
log_sha256: GenerationReceiptActionKey, ["type","pattern"], vec![];
});

wire_object!(TestRunRunsItem,["run","status","totals"],["type","properties","required","additionalProperties"],vec![],{
run: Whole<1,10>, ["type","minimum","maximum"], vec![];
status: GenerationReceiptStatus, ["const"], vec![];
totals: TestRunTotals, ["$ref"], vec![];
});

wire_object!(TestRun,["tests","base","repeat","parallel","status","tree","fresh","totals","receipts"],["type","description","properties","required","additionalProperties"],vec![("description",Doc::atom("A completed run_tests call with its PrismaBuild receipts (#408)."))],{
tests: List<Text<0,2048>,0,256,false>, ["type","maxItems","items"], vec![];
#[serde(default,deserialize_with="present",skip_serializing_if="Option::is_none")]
tests_identity: Option<TestRunTestsIdentity>, ["$ref"], vec![];
#[serde(default,deserialize_with="present",skip_serializing_if="Option::is_none")]
tests_omitted: Option<Whole<1,256>>, ["type","minimum","maximum"], vec![];
base: bool, ["type"], vec![];
#[serde(default,deserialize_with="present",skip_serializing_if="Option::is_none")]
base_semantics: Option<TestRunBaseSemantics>, ["enum"], vec![];
#[serde(default,deserialize_with="present",skip_serializing_if="Option::is_none")]
proof: Option<TestRunProof>, ["$ref"], vec![];
#[serde(default,deserialize_with="present",skip_serializing_if="Option::is_none")]
execution: Option<TestRunExecution>, ["type","properties","required","additionalProperties"], vec![];
tree: TestRunTree, ["type","pattern","description"], vec![("description",Doc::atom("The git tree of the checkout this run tested."))];
fresh: bool, ["type","description"], vec![("description",Doc::atom("False when PrismaBuild replayed a stored result."))];
repeat: Whole<1,10>, ["type","minimum","maximum"], vec![];
parallel: Parallel, ["enum"], vec![];
status: GenerationReceiptStatus, ["const"], vec![];
totals: TestRunTotals, ["$ref"], vec![];
receipts: List<TestRunReceiptsItem,1,1,false>, ["type","minItems","maxItems","items"], vec![];
#[serde(default,deserialize_with="present",skip_serializing_if="Option::is_none")]
runs: Option<List<TestRunRunsItem,0,10,false>>, ["type","maxItems","items"], vec![];
});

wire_object!(Totals,["passed","failed","error","skipped"],["type","properties","required","additionalProperties"],vec![],{
passed: Whole<0,{ u64::MAX }>, ["type","minimum"], vec![];
failed: Whole<0,{ u64::MAX }>, ["type","minimum"], vec![];
error: Whole<0,{ u64::MAX }>, ["type","minimum"], vec![];
skipped: Whole<0,{ u64::MAX }>, ["type","minimum"], vec![];
});

wire_link!(
    ReproductionVariant1FindingId,
    super::maker::WireText,
    "#/$defs/text"
);

wire_link!(
    ReproductionVariant1Proof,
    super::super::ProofDeclaration,
    "planner-reply.json#/$defs/proof"
);

wire_object!(ReproductionVariant1,["finding_id","tests","proof"],["type","properties","required","additionalProperties"],vec![],{
finding_id: ReproductionVariant1FindingId, ["$ref"], vec![];
tests: List<ReproductionVariant1FindingId,1,UNBOUNDED,true>, ["type","minItems","uniqueItems","items"], vec![];
proof: ReproductionVariant1Proof, ["$ref"], vec![];
});

wire_object!(ReproductionVariant2,["finding_id","reason"],["type","properties","required","additionalProperties"],vec![],{
finding_id: ReproductionVariant1FindingId, ["$ref"], vec![];
reason: ReproductionVariant1FindingId, ["$ref"], vec![];
});

wire_union!(Reproduction, {Variant1(ReproductionVariant1),Variant2(ReproductionVariant2)});

wire_link!(
    MakerReplyDoneBudgetExhaustion,
    super::maker::BudgetExhaustion,
    "#/$defs/budget_exhaustion"
);

wire_text!(
    MakerReplyDoneHead,
    0,
    UNBOUNDED,
    "^(?:[0-9a-f]{40}|[0-9a-f]{64})$",
    ""
);

wire_link!(
    MakerReplyDoneReproductionsItem,
    super::maker::Reproduction,
    "#/$defs/reproduction"
);

wire_link!(
    MakerReplyDoneTestEvidenceItem,
    super::maker::TestRun,
    "#/$defs/test_run"
);

wire_link!(
    MakerReplyDoneCheckingHandoff,
    super::maker::CheckingHandoff,
    "#/$defs/checking_handoff"
);

wire_link!(
    MakerReplyDoneGenerationEvidence,
    super::maker::GenerationReceipt,
    "#/$defs/generation_receipt"
);

wire_object!(MakerReplyDoneSelfCheckItemMet,["goal_id","evidence"],["type","properties","required","additionalProperties"],vec![],{
goal_id: ReproductionVariant1FindingId, ["$ref"], vec![];
evidence: ReproductionVariant1FindingId, ["$ref"], vec![];
});

wire_object!(MakerReplyDoneSelfCheckItemNotMet,["goal_id","why"],["type","properties","required","additionalProperties"],vec![],{
goal_id: ReproductionVariant1FindingId, ["$ref"], vec![];
why: ReproductionVariant1FindingId, ["$ref"], vec![];
});

wire_tagged!(MakerReplyDoneSelfCheckItem, "status", {Met(MakerReplyDoneSelfCheckItemMet) => ("met",1,1),NotMet(MakerReplyDoneSelfCheckItemNotMet) => ("not_met",1,1),Pending(MakerReplyDoneSelfCheckItemNotMet) => ("pending",1,1)});

wire_object!(MakerReplyDone,["self_check","head"],["type","properties","required","additionalProperties"],vec![],{
#[serde(default,deserialize_with="present",skip_serializing_if="Option::is_none")]
budget_exhaustion: Option<MakerReplyDoneBudgetExhaustion>, ["$ref","description"], vec![("description",Doc::atom("Added by worker code. The model must omit this field."))];
head: MakerReplyDoneHead, ["type","pattern","description"], vec![("description",Doc::atom("Added by worker code. The model must omit this field."))];
#[serde(default,deserialize_with="present",skip_serializing_if="Option::is_none")]
reproductions: Option<List<MakerReplyDoneReproductionsItem,0,UNBOUNDED,false>>, ["type","items"], vec![];
#[serde(default,deserialize_with="present",skip_serializing_if="Option::is_none")]
test_evidence: Option<List<MakerReplyDoneTestEvidenceItem,1,10,false>>, ["type","minItems","maxItems","items","description"], vec![("description",Doc::atom("Added by worker code. The model must omit this field."))];
#[serde(default,deserialize_with="present",skip_serializing_if="Option::is_none")]
checking_handoff: Option<MakerReplyDoneCheckingHandoff>, ["$ref","description"], vec![("description",Doc::atom("Added by worker code. Ordinary demonstrating proof remains pending for independent Checking. The model must omit this field."))];
#[serde(default,deserialize_with="present",skip_serializing_if="Option::is_none")]
generation_evidence: Option<MakerReplyDoneGenerationEvidence>, ["$ref","description"], vec![("description",Doc::atom("Added by worker code after verified import. The model must omit this field."))];
self_check: List<MakerReplyDoneSelfCheckItem,1,UNBOUNDED,false>, ["type","minItems","items"], vec![];
});

wire_object!(MakerReplyNeedsFilesFilesItem,["path","why"],["type","properties","required","additionalProperties"],vec![],{
path: ReproductionVariant1FindingId, ["$ref"], vec![];
why: ReproductionVariant1FindingId, ["$ref"], vec![];
});

wire_object!(MakerReplyNeedsFiles,["files"],["type","properties","required","additionalProperties"],vec![],{
#[serde(default,deserialize_with="present",skip_serializing_if="Option::is_none")]
budget_exhaustion: Option<MakerReplyDoneBudgetExhaustion>, ["$ref","description"], vec![("description",Doc::atom("Added by worker code. The model must omit this field."))];
#[serde(default,deserialize_with="present",skip_serializing_if="Option::is_none")]
fits: Option<bool>, ["type","description"], vec![("description",Doc::atom("Added by worker code. The model must omit this field."))];
files: List<MakerReplyNeedsFilesFilesItem,1,UNBOUNDED,true>, ["type","minItems","uniqueItems","items"], vec![];
});

wire_object!(MakerReplyCantMeet,["report"],["type","description","properties","required","additionalProperties"],vec![("description",Doc::atom("Only when a maker-owned goal cannot be met within the declared files and tools, the plan is defective, or evidence preflight names a truly unsupported requirement. Ordinary controller-owned checks and PR publication are pending downstream work. Not for unfinished steps your own tools can do."))],{
#[serde(default,deserialize_with="present",skip_serializing_if="Option::is_none")]
budget_exhaustion: Option<MakerReplyDoneBudgetExhaustion>, ["$ref","description"], vec![("description",Doc::atom("Added by worker code. The model must omit this field."))];
#[serde(default,deserialize_with="present",skip_serializing_if="Option::is_none")]
unfinished_obligations: Option<List<Text<1,UNBOUNDED>,0,UNBOUNDED,false>>, ["type","items"], vec![];
report: ReproductionVariant1FindingId, ["$ref"], vec![];
});

wire_choice!(MakerReplyBlockedEvidenceVariant1Class, "const", { Value => "test_infrastructure" });

wire_object!(MakerReplyBlockedEvidenceVariant1,["action_key","class"],["type","properties","required","additionalProperties"],vec![],{
action_key: GenerationReceiptActionKey, ["type","pattern"], vec![];
class: MakerReplyBlockedEvidenceVariant1Class, ["const","description"], vec![("description",Doc::atom("Added by worker code. The model must omit this field."))];
});

wire_text!(
    MakerReplyBlockedEvidenceVariant2ToolError,
    0,
    UNBOUNDED,
    "^tool-error-[1-9][0-9]*$",
    ""
);

wire_choice!(MakerReplyBlockedEvidenceVariant2Tool, "enum", {List => "list",Read => "read",Glob => "glob",Grep => "grep",Write => "write",Edit => "edit",RunTests => "run_tests",RunGenerator => "run_generator",Recall => "recall"});

wire_choice!(MakerReplyBlockedEvidenceVariant2Class, "enum", {TestSubmission => "test_submission",EACCES => "EACCES",EPERM => "EPERM",EROFS => "EROFS",ENOSPC => "ENOSPC",EDQUOT => "EDQUOT",EIO => "EIO",EMFILE => "EMFILE",ENFILE => "ENFILE",ENOMEM => "ENOMEM"});

wire_object!(MakerReplyBlockedEvidenceVariant2,["tool_error","tool","class"],["type","properties","required","additionalProperties"],vec![],{
tool_error: MakerReplyBlockedEvidenceVariant2ToolError, ["type","pattern"], vec![];
tool: MakerReplyBlockedEvidenceVariant2Tool, ["enum","description"], vec![("description",Doc::atom("Added by worker code. The model must omit this field."))];
class: MakerReplyBlockedEvidenceVariant2Class, ["enum","description"], vec![("description",Doc::atom("Added by worker code. The model must omit this field."))];
});

wire_union!(MakerReplyBlockedEvidence, {Variant1(MakerReplyBlockedEvidenceVariant1),Variant2(MakerReplyBlockedEvidenceVariant2)});

wire_object!(MakerReplyBlocked,["evidence","report"],["type","properties","required","additionalProperties"],vec![],{
#[serde(default,deserialize_with="present",skip_serializing_if="Option::is_none")]
budget_exhaustion: Option<MakerReplyDoneBudgetExhaustion>, ["$ref","description"], vec![("description",Doc::atom("Added by worker code. The model must omit this field."))];
evidence: MakerReplyBlockedEvidence, ["description","oneOf"], vec![("description",Doc::atom("An action_key whose run_tests summary had status infrastructure_error, or a tool_error id, recorded in this run."))];
report: ReproductionVariant1FindingId, ["$ref"], vec![];
});

wire_object!(MakerReplyOutOfRoom,[],["type","properties","required","additionalProperties"],vec![],{
#[serde(default,deserialize_with="present",skip_serializing_if="Option::is_none")]
budget_exhaustion: Option<MakerReplyDoneBudgetExhaustion>, ["$ref","description"], vec![("description",Doc::atom("Added by worker code. The model must omit this field."))];
});

wire_tagged!(MakerReply, "kind", {Done(MakerReplyDone) => ("done",1,0),NeedsFiles(MakerReplyNeedsFiles) => ("needs_files",1,0),CantMeet(MakerReplyCantMeet) => ("cant_meet",2,0),Blocked(MakerReplyBlocked) => ("blocked",1,0),OutOfRoom(MakerReplyOutOfRoom) => ("out_of_room",1,0)});

pub fn document() -> Doc {
    let mut s = MakerReply::wire_schema();
    s.set(
        "$schema",
        Doc::atom("https://json-schema.org/draft/2020-12/schema"),
    );
    s.set(
        "$id",
        Doc::atom("https://trustladderiii.invalid/workers/protocol-1/maker-reply.json"),
    );
    s.set(
        "$defs",
        Doc::object(vec![
            ("budget_exhaustion", <BudgetExhaustion>::wire_schema()),
            ("checking_handoff", <CheckingHandoff>::wire_schema()),
            ("generation_receipt", <GenerationReceipt>::wire_schema()),
            ("text", <WireText>::wire_schema()),
            ("selector_identity", <SelectorIdentity>::wire_schema()),
            ("test_run", <TestRun>::wire_schema()),
            ("totals", <Totals>::wire_schema()),
            ("reproduction", <Reproduction>::wire_schema()),
        ]),
    );
    s.ordered(&["$schema", "$id", "oneOf", "$defs"])
}
