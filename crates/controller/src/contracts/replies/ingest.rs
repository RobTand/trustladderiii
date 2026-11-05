//! Ingest reply wire contract (#730 C3).
use super::wire::*;

wire_choice!(ForecastTokenizer, "const", { Value => "o200k_base" });

wire_object!(Forecast,["tokenizer","brief_tokens","file_tokens","overhead_tokens","tokens","fit_tokens","context_tokens","fits"],["type","properties","required","additionalProperties","description"],vec![("description",Doc::atom("Appended by the reply check. The model must omit this field."))],{
tokenizer: ForecastTokenizer, ["const"], vec![];
brief_tokens: Whole<0,{ u64::MAX }>, ["type","minimum"], vec![];
file_tokens: BTreeMap<String,Whole<0,{ u64::MAX }>>, ["type","additionalProperties"], vec![];
overhead_tokens: Whole<0,{ u64::MAX }>, ["type","minimum"], vec![];
tokens: Whole<0,{ u64::MAX }>, ["type","minimum"], vec![];
fit_tokens: Whole<1,{ u64::MAX }>, ["type","minimum"], vec![];
context_tokens: Whole<1,{ u64::MAX }>, ["type","minimum"], vec![];
fits: bool, ["type"], vec![];
});

wire_object!(ToolCapability,["tool","executor","runner_features","artifacts","methods","protected_files"],["type","properties","required","additionalProperties"],vec![],{
tool: Text<1,128>, ["type","minLength","maxLength"], vec![];
executor: Text<1,128>, ["type","minLength","maxLength"], vec![];
runner_features: List<Text<1,512>,0,64,true>, ["type","items","uniqueItems","maxItems"], vec![];
artifacts: List<Text<1,512>,0,64,true>, ["type","items","uniqueItems","maxItems"], vec![];
methods: List<Text<1,512>,0,64,true>, ["type","items","uniqueItems","maxItems"], vec![];
protected_files: List<Text<1,512>,0,64,true>, ["type","items","uniqueItems","maxItems"], vec![];
});

wire_object!(EvidenceRequirementProvenanceSource,["source","quote"],["type","properties","required","additionalProperties"],vec![],{
source: Text<1,512>, ["type","minLength","maxLength"], vec![];
quote: Text<1,4096>, ["type","minLength","maxLength"], vec![];
});

wire_object!(
    EvidenceRequirementProvenanceMethod,
    [],
    ["type", "properties", "required", "additionalProperties"],
    vec![],
    {}
);

wire_tagged!(EvidenceRequirementProvenance, "kind", {Source(EvidenceRequirementProvenanceSource) => ("source",0,0),Method(EvidenceRequirementProvenanceMethod) => ("method",0,0)});

wire_object!(EvidenceRequirement,["tool","executor","runner_features","artifacts","methods","protected_files","requirement"],["type","properties","required","additionalProperties"],vec![],{
tool: Text<1,128>, ["type","minLength","maxLength"], vec![];
executor: Text<1,128>, ["type","minLength","maxLength"], vec![];
runner_features: List<Text<1,512>,0,64,true>, ["type","items","uniqueItems","maxItems"], vec![];
artifacts: List<Text<1,512>,0,64,true>, ["type","items","uniqueItems","maxItems"], vec![];
methods: List<Text<1,512>,0,64,true>, ["type","items","uniqueItems","maxItems"], vec![];
protected_files: List<Text<1,512>,0,64,true>, ["type","items","uniqueItems","maxItems"], vec![];
requirement: Text<1,4096>, ["type","minLength","maxLength"], vec![];
#[serde(default,deserialize_with="present",skip_serializing_if="Option::is_none")]
provenance: Option<EvidenceRequirementProvenance>, ["description","oneOf"], vec![("description",Doc::atom("Source obligations are immutable across roles; method choices may be replaced. Omitted provenance is a legacy mandatory requirement."))];
});

wire_link!(
    EvidenceContractItem,
    super::ingest::EvidenceRequirement,
    "#/$defs/evidence_requirement"
);

pub type EvidenceContract = List<EvidenceContractItem, 0, 64, false>;

wire_choice!(ProofObligationKind, "const", { Value => "assertion" });

wire_choice!(ProofObligationObserver, "enum", {New => "new",Any => "any"});

wire_text!(
    ProofObligationSubjectsItem,
    1,
    1000,
    "^(?!/)(?!.*(?:^|/)\\.\\.(?:/|::|$))[^\\\\\\u0000\\r\\n]+\\.py(?:::[A-Za-z_][A-Za-z_0-9]*)*$",
    ""
);

wire_object!(ProofObligation,["kind","observer","subjects","source","quote"],["type","properties","required","additionalProperties","description"],vec![("description",Doc::atom("A proof obligation explicitly required by the source, with its literal quote. Method choices cannot invent or replace it."))],{
kind: ProofObligationKind, ["const"], vec![];
observer: ProofObligationObserver, ["enum"], vec![];
subjects: List<ProofObligationSubjectsItem,1,16,true>, ["type","minItems","maxItems","uniqueItems","items"], vec![];
source: Text<1,256>, ["type","minLength","maxLength"], vec![];
quote: Text<1,2000>, ["type","minLength","maxLength"], vec![];
});

wire_choice!(IngestReplyBriefGoalsItemVerification, "enum", {Test => "test",Review => "review"});

wire_choice!(IngestReplyBriefGoalsItemStage, "enum", {Maker => "maker",Checking => "checking",Publishing => "publishing"});

wire_text!(
    IngestReplyBriefGoalsItemPublicationBranch,
    1,
    UNBOUNDED,
    "\\S",
    ""
);

wire_text!(
    IngestReplyBriefGoalsItemPublicationBodyLinesItem,
    1,
    512,
    "^[^\\r\\n]+$",
    ""
);

wire_object!(IngestReplyBriefGoalsItemPublication,["non_draft","body_lines","proof"],["type","properties","required","additionalProperties"],vec![],{
#[serde(default,deserialize_with="present",skip_serializing_if="Option::is_none")]
branch: Option<IngestReplyBriefGoalsItemPublicationBranch>, ["type","minLength","pattern"], vec![];
#[serde(default,deserialize_with="present",skip_serializing_if="Option::is_none")]
branch_prefix: Option<IngestReplyBriefGoalsItemPublicationBranch>, ["type","minLength","pattern"], vec![];
non_draft: True, ["const"], vec![];
body_lines: List<IngestReplyBriefGoalsItemPublicationBodyLinesItem,0,UNBOUNDED,true>, ["type","uniqueItems","items"], vec![];
proof: bool, ["type"], vec![];
});

wire_link!(
    IngestReplyBriefGoalsItemProofObligation,
    super::ingest::ProofObligation,
    "#/$defs/proof_obligation"
);

checked_wire_object!(IngestReplyBriefGoalsItem,["description","verification"],["type","properties","required","additionalProperties","allOf"],vec![("allOf",Doc::Array(vec![Doc::object(vec![("if",Doc::object(vec![("required",Doc::Array(vec![Doc::atom("stage")])),("properties",Doc::object(vec![("stage",Doc::object(vec![("const",Doc::atom("publishing"))]))]))])),("then",Doc::object(vec![("required",Doc::Array(vec![Doc::atom("publication")]))])),("else",Doc::object(vec![("not",Doc::object(vec![("required",Doc::Array(vec![Doc::atom("publication")]))]))]))])]))],{
description: Text<1,UNBOUNDED>, ["type","minLength"], vec![];
verification: IngestReplyBriefGoalsItemVerification, ["enum"], vec![];
#[serde(default,deserialize_with="present",skip_serializing_if="Option::is_none")]
stage: Option<IngestReplyBriefGoalsItemStage>, ["enum"], vec![];
#[serde(default,deserialize_with="present",skip_serializing_if="Option::is_none")]
publication: Option<IngestReplyBriefGoalsItemPublication>, ["type","properties","required","additionalProperties"], vec![];
#[serde(default,deserialize_with="present",skip_serializing_if="Option::is_none")]
proof_obligation: Option<IngestReplyBriefGoalsItemProofObligation>, ["$ref"], vec![];
}, |goal:&Self| goal.publication.is_some() == matches!(goal.stage,Some(IngestReplyBriefGoalsItemStage::Publishing)));

wire_link!(
    IngestReplyBriefForecast,
    super::ingest::Forecast,
    "#/$defs/forecast"
);

wire_object!(IngestReplyBriefMemoryFlagsItemEvidence,["path","start_line","end_line","quote"],["type","properties","required","additionalProperties","description"],vec![("description",Doc::atom("A verbatim quote of at most 40 lines from the read-only checkout, at these 1-based lines of this repo-relative file."))],{
path: Text<1,1024>, ["type","minLength","maxLength"], vec![];
start_line: Whole<1,{ u64::MAX }>, ["type","minimum"], vec![];
end_line: Whole<1,{ u64::MAX }>, ["type","minimum"], vec![];
quote: Text<8,2000>, ["type","minLength","maxLength"], vec![];
});

wire_object!(IngestReplyBriefMemoryFlagsItem,["id","claim","reason","evidence"],["type","properties","required","additionalProperties"],vec![],{
id: Text<1,256>, ["type","minLength","maxLength"], vec![];
claim: Text<8,1000>, ["type","minLength","maxLength","description"], vec![("description",Doc::atom("The memory's own words that the evidence contradicts, quoted verbatim from its content."))];
reason: Text<1,1000>, ["type","minLength","maxLength"], vec![];
evidence: IngestReplyBriefMemoryFlagsItemEvidence, ["type","properties","required","additionalProperties","description"], vec![("description",Doc::atom("A verbatim quote of at most 40 lines from the read-only checkout, at these 1-based lines of this repo-relative file."))];
});

wire_link!(
    IngestReplyBriefEvidenceContract,
    super::ingest::EvidenceContract,
    "#/$defs/evidence_contract"
);

wire_choice!(IngestReplyBriefMergePrerequisitesItemKind, "enum", {Issue => "issue",PullRequest => "pull_request"});

wire_object!(IngestReplyBriefMergePrerequisitesItem,["kind","number","source_quote"],["type","properties","required","additionalProperties"],vec![],{
kind: IngestReplyBriefMergePrerequisitesItemKind, ["enum"], vec![];
number: Whole<1,{ u64::MAX }>, ["type","minimum"], vec![];
source_quote: Text<1,4000>, ["type","minLength","maxLength"], vec![];
});

wire_object!(IngestReplyBrief,["brief","goals","memory_ids"],["type","properties","required","additionalProperties"],vec![],{
brief: Text<1,80000>, ["type","minLength","maxLength"], vec![];
goals: List<IngestReplyBriefGoalsItem,1,UNBOUNDED,false>, ["type","items","minItems"], vec![];
memory_ids: List<Text<1,UNBOUNDED>,0,UNBOUNDED,true>, ["type","items","uniqueItems"], vec![];
#[serde(default,deserialize_with="present",skip_serializing_if="Option::is_none")]
forecast: Option<IngestReplyBriefForecast>, ["$ref"], vec![];
#[serde(default,deserialize_with="present",skip_serializing_if="Option::is_none")]
files: Option<List<Text<1,UNBOUNDED>,0,UNBOUNDED,true>>, ["type","items","uniqueItems","description"], vec![("description",Doc::atom("Files in the root scope used by the forecaster. Required when forecast settings are supplied."))];
#[serde(default,deserialize_with="present",skip_serializing_if="Option::is_none")]
protected_paths: Option<List<Text<1,UNBOUNDED>,1,UNBOUNDED,true>>, ["type","items","minItems","uniqueItems","description"], vec![("description",Doc::atom("Concrete repository-relative paths that MUST change to meet the request's goals. Report only when the whole request cannot land usefully without these edits; never for inspected files, excluded edits, or an optional part. The controller verifies the effective protected-path list."))];
#[serde(default,deserialize_with="present",skip_serializing_if="Option::is_none")]
memory_flags: Option<List<IngestReplyBriefMemoryFlagsItem,0,3,false>>, ["type","maxItems","items","description"], vec![("description",Doc::atom("Cited memories the checkout shows to be wrong, each with the evidence that contradicts it. Code checks the quote before the memory is retired."))];
#[serde(default,deserialize_with="present",skip_serializing_if="Option::is_none")]
evidence_contract: Option<IngestReplyBriefEvidenceContract>, ["$ref"], vec![];
#[serde(default,deserialize_with="present",skip_serializing_if="Option::is_none")]
evidence_errors: Option<List<Text<1,8192>,0,1024,true>>, ["type","items","uniqueItems","maxItems","description"], vec![("description",Doc::atom("Code-owned unsupported evidence requirements."))];
#[serde(default,deserialize_with="present",skip_serializing_if="Option::is_none")]
merge_prerequisites: Option<List<IngestReplyBriefMergePrerequisitesItem,0,UNBOUNDED,true>>, ["type","uniqueItems","items","description"], vec![("description",Doc::atom("Same-repository merge prerequisites explicitly required by the original issue, each with its verbatim source quote."))];
});

wire_choice!(IngestReplyEarlyEndOutcome, "enum", {Duplicate => "duplicate",AlreadyDone => "already_done",NotActionable => "not_actionable"});

wire_object!(IngestReplyEarlyEnd,["outcome","reason"],["type","properties","required","additionalProperties"],vec![],{
outcome: IngestReplyEarlyEndOutcome, ["enum"], vec![];
reason: Text<1,UNBOUNDED>, ["type","minLength"], vec![];
#[serde(default,deserialize_with="present",skip_serializing_if="Option::is_none")]
protected_paths: Option<List<Text<1,UNBOUNDED>,1,UNBOUNDED,true>>, ["type","items","minItems","uniqueItems","description"], vec![("description",Doc::atom("Concrete repository-relative paths that MUST change to meet the request's goals. Report only when the whole request cannot land usefully without these edits; never for inspected files, excluded edits, or an optional part. The controller verifies the effective protected-path list."))];
#[serde(default,deserialize_with="present",skip_serializing_if="Option::is_none")]
evidence_contract: Option<IngestReplyBriefEvidenceContract>, ["$ref"], vec![];
#[serde(default,deserialize_with="present",skip_serializing_if="Option::is_none")]
evidence_errors: Option<List<Text<1,8192>,0,1024,true>>, ["type","items","uniqueItems","maxItems","description"], vec![("description",Doc::atom("Code-owned unsupported evidence requirements."))];
});

wire_tagged!(IngestReply, "kind", {Brief(IngestReplyBrief) => ("brief",0,0),EarlyEnd(IngestReplyEarlyEnd) => ("early_end",0,0)});

pub fn document() -> Doc {
    let mut s = IngestReply::wire_schema();
    s.set(
        "$schema",
        Doc::atom("https://json-schema.org/draft/2020-12/schema"),
    );
    s.set(
        "$id",
        Doc::atom("https://trustladderiii.invalid/workers/protocol-1/ingest-reply.json"),
    );
    s.set(
        "$defs",
        Doc::object(vec![
            ("forecast", <Forecast>::wire_schema()),
            ("tool_capability", <ToolCapability>::wire_schema()),
            ("evidence_requirement", <EvidenceRequirement>::wire_schema()),
            (
                "evidence_contract",
                <List<EvidenceContractItem, 0, 64, false>>::wire_schema(),
            ),
            ("proof_obligation", <ProofObligation>::wire_schema()),
        ]),
    );
    s.ordered(&["$schema", "$id", "oneOf", "$defs"])
}
