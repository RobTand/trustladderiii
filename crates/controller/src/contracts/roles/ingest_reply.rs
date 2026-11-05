use crate::contracts::bounded::{Text as BoundedText, *};
use crate::contracts::render::{Layout, TypeOrder, TypeUse};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(inline)]
#[schemars(transform = reply_constant_schema)]
pub enum ForecastTokenizer {
    #[serde(rename = "o200k_base")]
    Value,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(rename = "forecast")]
#[schemars(description = "Appended by the reply check. The model must omit this field.")]
pub struct Forecast {
    pub tokenizer: ForecastTokenizer,
    pub brief_tokens: ReplyNumber<0, 0, true, false, true>,
    pub file_tokens: std::collections::BTreeMap<String, ReplyNumber<0, 0, true, false, true>>,
    pub overhead_tokens: ReplyNumber<0, 0, true, false, true>,
    pub tokens: ReplyNumber<0, 0, true, false, true>,
    pub fit_tokens: ReplyNumber<1, 0, true, false, true>,
    pub context_tokens: ReplyNumber<1, 0, true, false, true>,
    pub fits: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(rename = "tool_capability")]
pub struct ToolCapability {
    pub tool: BoundedText<1, 128>,
    pub executor: BoundedText<1, 128>,
    pub runner_features: List<BoundedText<1, 512>, 0, 64, true>,
    pub artifacts: List<BoundedText<1, 512>, 0, 64, true>,
    pub methods: List<BoundedText<1, 512>, 0, 64, true>,
    pub protected_files: List<BoundedText<1, 512>, 0, 64, true>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(inline)]
#[schemars(transform = reply_constant_schema)]
pub enum EvidenceRequirementProvenanceSourceKind {
    #[serde(rename = "source")]
    Value,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(inline)]
pub struct EvidenceRequirementProvenanceSource {
    pub kind: EvidenceRequirementProvenanceSourceKind,
    pub source: BoundedText<1, 512>,
    pub quote: BoundedText<1, 4096>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(inline)]
#[schemars(transform = reply_constant_schema)]
pub enum EvidenceRequirementProvenanceMethodKind {
    #[serde(rename = "method")]
    Value,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(inline)]
pub struct EvidenceRequirementProvenanceMethod {
    pub kind: EvidenceRequirementProvenanceMethodKind,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
#[schemars(inline)]
#[schemars(
    description = "Source obligations are immutable across roles; method choices may be replaced. Omitted provenance is a legacy mandatory requirement."
)]
#[schemars(transform = reply_one_of)]
pub enum EvidenceRequirementProvenance {
    Source(EvidenceRequirementProvenanceSource),
    Method(EvidenceRequirementProvenanceMethod),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(rename = "evidence_requirement")]
pub struct EvidenceRequirement {
    pub tool: BoundedText<1, 128>,
    pub executor: BoundedText<1, 128>,
    pub runner_features: List<BoundedText<1, 512>, 0, 64, true>,
    pub artifacts: List<BoundedText<1, 512>, 0, 64, true>,
    pub methods: List<BoundedText<1, 512>, 0, 64, true>,
    pub protected_files: List<BoundedText<1, 512>, 0, 64, true>,
    pub requirement: BoundedText<1, 4096>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "EvidenceRequirementProvenance", default)]
    #[schemars(
        description = "Source obligations are immutable across roles; method choices may be replaced. Omitted provenance is a legacy mandatory requirement."
    )]
    pub provenance: Option<EvidenceRequirementProvenance>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct EvidenceContract(pub List<EvidenceRequirement, 0, 64, false>);
impl JsonSchema for EvidenceContract {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "evidence_contract".into()
    }
    fn json_schema(g: &mut schemars::SchemaGenerator) -> schemars::Schema {
        <List<EvidenceRequirement, 0, 64, false>>::json_schema(g)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(inline)]
#[schemars(transform = reply_constant_schema)]
pub enum ProofObligationKind {
    #[serde(rename = "assertion")]
    Value,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(inline)]
#[schemars(transform = reply_enum_schema)]
pub enum ProofObligationObserver {
    #[serde(rename = "new")]
    New,
    #[serde(rename = "any")]
    Any,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(rename = "proof_obligation")]
#[schemars(
    description = "A proof obligation explicitly required by the source, with its literal quote. Method choices cannot invent or replace it."
)]
pub struct ProofObligation {
    pub kind: ProofObligationKind,
    pub observer: ProofObligationObserver,
    pub subjects: List<ReplyText<4, 1, 1000>, 1, 16, true>,
    pub source: BoundedText<1, 256>,
    pub quote: BoundedText<1, 2000>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(inline)]
#[schemars(transform = reply_constant_schema)]
pub enum IngestReplyBriefKind {
    #[serde(rename = "brief")]
    Value,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(inline)]
#[schemars(transform = reply_enum_schema)]
pub enum IngestReplyBriefGoalsItemVerification {
    #[serde(rename = "test")]
    Test,
    #[serde(rename = "review")]
    Review,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(inline)]
#[schemars(transform = reply_enum_schema)]
pub enum IngestReplyBriefGoalsItemStage {
    #[serde(rename = "maker")]
    Maker,
    #[serde(rename = "checking")]
    Checking,
    #[serde(rename = "publishing")]
    Publishing,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(inline)]
pub struct IngestReplyBriefGoalsItemPublication {
    pub non_draft: True,
    pub body_lines: List<ReplyText<2, 1, 512>, 0, UNBOUNDED, true>,
    pub proof: bool,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "ReplyText<0, 1, UNBOUNDED>", default)]
    pub branch: Option<ReplyText<0, 1, UNBOUNDED>>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "ReplyText<0, 1, UNBOUNDED>", default)]
    pub branch_prefix: Option<ReplyText<0, 1, UNBOUNDED>>,
}

#[derive(Clone, Debug, PartialEq, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(inline)]
#[schemars(extend("allOf" = [{"if": {"required": ["stage"], "properties": {"stage": {"const": "publishing"}}}, "then": {"required": ["publication"]}, "else": {"not": {"required": ["publication"]}}}]))]
pub struct IngestReplyBriefGoalsItem {
    description: BoundedText<1, UNBOUNDED>,
    verification: IngestReplyBriefGoalsItemVerification,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "IngestReplyBriefGoalsItemStage", default)]
    stage: Option<IngestReplyBriefGoalsItemStage>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "IngestReplyBriefGoalsItemPublication", default)]
    publication: Option<IngestReplyBriefGoalsItemPublication>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "ProofObligation", default)]
    proof_obligation: Option<ProofObligation>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct IngestReplyBriefGoalsItemWire {
    description: BoundedText<1, UNBOUNDED>,
    verification: IngestReplyBriefGoalsItemVerification,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    stage: Option<IngestReplyBriefGoalsItemStage>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    publication: Option<IngestReplyBriefGoalsItemPublication>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    proof_obligation: Option<ProofObligation>,
}

impl<'de> Deserialize<'de> for IngestReplyBriefGoalsItem {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = IngestReplyBriefGoalsItemWire::deserialize(deserializer)?;
        let publishing = value
            .stage
            .as_ref()
            .is_some_and(|stage| matches!(stage, IngestReplyBriefGoalsItemStage::Publishing));
        if publishing != value.publication.is_some() {
            return Err(serde::de::Error::custom(
                "publication is required exactly for publishing goals",
            ));
        }
        Ok(Self {
            description: value.description,
            verification: value.verification,
            stage: value.stage,
            publication: value.publication,
            proof_obligation: value.proof_obligation,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(inline)]
#[schemars(
    description = "A verbatim quote of at most 40 lines from the read-only checkout, at these 1-based lines of this repo-relative file."
)]
pub struct IngestReplyBriefMemoryFlagsItemEvidence {
    pub path: BoundedText<1, 1024>,
    pub start_line: ReplyNumber<1, 0, true, false, true>,
    pub end_line: ReplyNumber<1, 0, true, false, true>,
    pub quote: BoundedText<8, 2000>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(inline)]
pub struct IngestReplyBriefMemoryFlagsItem {
    pub id: BoundedText<1, 256>,
    #[schemars(
        description = "The memory's own words that the evidence contradicts, quoted verbatim from its content."
    )]
    pub claim: BoundedText<8, 1000>,
    pub reason: BoundedText<1, 1000>,
    #[schemars(
        description = "A verbatim quote of at most 40 lines from the read-only checkout, at these 1-based lines of this repo-relative file."
    )]
    pub evidence: IngestReplyBriefMemoryFlagsItemEvidence,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(inline)]
#[schemars(transform = reply_enum_schema)]
pub enum IngestReplyBriefMergePrerequisitesItemKind {
    #[serde(rename = "issue")]
    Issue,
    #[serde(rename = "pull_request")]
    PullRequest,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(inline)]
pub struct IngestReplyBriefMergePrerequisitesItem {
    pub kind: IngestReplyBriefMergePrerequisitesItemKind,
    pub number: ReplyNumber<1, 0, true, false, true>,
    pub source_quote: BoundedText<1, 4000>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(inline)]
pub struct IngestReplyBrief {
    pub kind: IngestReplyBriefKind,
    pub brief: BoundedText<1, 80000>,
    pub goals: List<IngestReplyBriefGoalsItem, 1, UNBOUNDED, false>,
    pub memory_ids: List<BoundedText<1, UNBOUNDED>, 0, UNBOUNDED, true>,
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
    #[schemars(with = "List<BoundedText<1, UNBOUNDED>, 0, UNBOUNDED, true>", default)]
    #[schemars(
        description = "Files in the root scope used by the forecaster. Required when forecast settings are supplied."
    )]
    pub files: Option<List<BoundedText<1, UNBOUNDED>, 0, UNBOUNDED, true>>,
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
    #[schemars(with = "List<IngestReplyBriefMemoryFlagsItem, 0, 3, false>", default)]
    #[schemars(
        description = "Cited memories the checkout shows to be wrong, each with the evidence that contradicts it. Code checks the quote before the memory is retired."
    )]
    pub memory_flags: Option<List<IngestReplyBriefMemoryFlagsItem, 0, 3, false>>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "EvidenceContract", default)]
    pub evidence_contract: Option<EvidenceContract>,
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
    #[schemars(
        with = "List<IngestReplyBriefMergePrerequisitesItem, 0, UNBOUNDED, true>",
        default
    )]
    #[schemars(
        description = "Same-repository merge prerequisites explicitly required by the original issue, each with its verbatim source quote."
    )]
    pub merge_prerequisites:
        Option<List<IngestReplyBriefMergePrerequisitesItem, 0, UNBOUNDED, true>>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(inline)]
#[schemars(transform = reply_constant_schema)]
pub enum IngestReplyEarlyEndKind {
    #[serde(rename = "early_end")]
    Value,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(inline)]
#[schemars(transform = reply_enum_schema)]
pub enum IngestReplyEarlyEndOutcome {
    #[serde(rename = "duplicate")]
    Duplicate,
    #[serde(rename = "already_done")]
    AlreadyDone,
    #[serde(rename = "not_actionable")]
    NotActionable,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(inline)]
pub struct IngestReplyEarlyEnd {
    pub kind: IngestReplyEarlyEndKind,
    pub outcome: IngestReplyEarlyEndOutcome,
    pub reason: BoundedText<1, UNBOUNDED>,
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
    #[schemars(with = "EvidenceContract", default)]
    pub evidence_contract: Option<EvidenceContract>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "List<BoundedText<1, 8192>, 0, 1024, true>", default)]
    #[schemars(description = "Code-owned unsupported evidence requirements.")]
    pub evidence_errors: Option<List<BoundedText<1, 8192>, 0, 1024, true>>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
#[schemars(inline)]
#[schemars(transform = reply_one_of)]
pub enum IngestReply {
    Brief(Box<IngestReplyBrief>),
    EarlyEnd(IngestReplyEarlyEnd),
}

pub fn schema_value() -> serde_json::Value {
    reply_schema_value::<IngestReply>("ingest", |g| {
        g.subschema_for::<Forecast>();
        g.subschema_for::<ToolCapability>();
        g.subschema_for::<EvidenceRequirement>();
        g.subschema_for::<EvidenceContract>();
        g.subschema_for::<ProofObligation>();
    })
}

pub fn schema_text() -> String {
    let types = [
        TypeOrder::of::<Forecast>(FORECAST_ORDER),
        TypeOrder::of::<ToolCapability>(TOOL_CAPABILITY_ORDER),
        TypeOrder::of::<EvidenceRequirement>(EVIDENCE_REQUIREMENT_ORDER),
        TypeOrder::of::<EvidenceContract>(EVIDENCE_CONTRACT_ORDER),
        TypeOrder::of::<ProofObligation>(PROOF_OBLIGATION_ORDER),
    ];
    let shared = [
        TypeUse {
            pointer: "/$defs/forecast",
            order: &types[0],
        },
        TypeUse {
            pointer: "/$defs/tool_capability",
            order: &types[1],
        },
        TypeUse {
            pointer: "/$defs/evidence_requirement",
            order: &types[2],
        },
        TypeUse {
            pointer: "/$defs/evidence_contract",
            order: &types[3],
        },
        TypeUse {
            pointer: "/$defs/proof_obligation",
            order: &types[4],
        },
    ];
    crate::contracts::render::render(
        "ingest-reply.json",
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
    (
        "/oneOf/0",
        &["type", "properties", "required", "additionalProperties"],
    ),
    (
        "/oneOf/0/properties",
        &[
            "kind",
            "brief",
            "goals",
            "memory_ids",
            "forecast",
            "files",
            "protected_paths",
            "memory_flags",
            "evidence_contract",
            "evidence_errors",
            "merge_prerequisites",
        ],
    ),
    ("/oneOf/0/properties/kind", &["const"]),
    (
        "/oneOf/0/properties/brief",
        &["type", "minLength", "maxLength"],
    ),
    ("/oneOf/0/properties/goals", &["type", "items", "minItems"]),
    (
        "/oneOf/0/properties/goals/items",
        &[
            "type",
            "properties",
            "required",
            "additionalProperties",
            "allOf",
        ],
    ),
    (
        "/oneOf/0/properties/goals/items/properties",
        &[
            "description",
            "verification",
            "stage",
            "publication",
            "proof_obligation",
        ],
    ),
    (
        "/oneOf/0/properties/goals/items/properties/description",
        &["type", "minLength"],
    ),
    (
        "/oneOf/0/properties/goals/items/properties/verification",
        &["enum"],
    ),
    (
        "/oneOf/0/properties/goals/items/properties/stage",
        &["enum"],
    ),
    (
        "/oneOf/0/properties/goals/items/properties/publication",
        &["type", "properties", "required", "additionalProperties"],
    ),
    (
        "/oneOf/0/properties/goals/items/properties/publication/properties",
        &[
            "branch",
            "branch_prefix",
            "non_draft",
            "body_lines",
            "proof",
        ],
    ),
    (
        "/oneOf/0/properties/goals/items/properties/publication/properties/branch",
        &["type", "minLength", "pattern"],
    ),
    (
        "/oneOf/0/properties/goals/items/properties/publication/properties/branch_prefix",
        &["type", "minLength", "pattern"],
    ),
    (
        "/oneOf/0/properties/goals/items/properties/publication/properties/non_draft",
        &["const"],
    ),
    (
        "/oneOf/0/properties/goals/items/properties/publication/properties/body_lines",
        &["type", "uniqueItems", "items"],
    ),
    (
        "/oneOf/0/properties/goals/items/properties/publication/properties/body_lines/items",
        &["type", "minLength", "maxLength", "pattern"],
    ),
    (
        "/oneOf/0/properties/goals/items/properties/publication/properties/proof",
        &["type"],
    ),
    (
        "/oneOf/0/properties/goals/items/properties/proof_obligation",
        &["$ref"],
    ),
    (
        "/oneOf/0/properties/goals/items/allOf/0",
        &["if", "then", "else"],
    ),
    (
        "/oneOf/0/properties/goals/items/allOf/0/if",
        &["required", "properties"],
    ),
    (
        "/oneOf/0/properties/goals/items/allOf/0/if/properties",
        &["stage"],
    ),
    (
        "/oneOf/0/properties/goals/items/allOf/0/if/properties/stage",
        &["const"],
    ),
    (
        "/oneOf/0/properties/goals/items/allOf/0/then",
        &["required"],
    ),
    ("/oneOf/0/properties/goals/items/allOf/0/else", &["not"]),
    (
        "/oneOf/0/properties/goals/items/allOf/0/else/not",
        &["required"],
    ),
    (
        "/oneOf/0/properties/memory_ids",
        &["type", "items", "uniqueItems"],
    ),
    (
        "/oneOf/0/properties/memory_ids/items",
        &["type", "minLength"],
    ),
    ("/oneOf/0/properties/forecast", &["$ref"]),
    (
        "/oneOf/0/properties/files",
        &["type", "items", "uniqueItems", "description"],
    ),
    ("/oneOf/0/properties/files/items", &["type", "minLength"]),
    (
        "/oneOf/0/properties/protected_paths",
        &["type", "items", "minItems", "uniqueItems", "description"],
    ),
    (
        "/oneOf/0/properties/protected_paths/items",
        &["type", "minLength"],
    ),
    (
        "/oneOf/0/properties/memory_flags",
        &["type", "maxItems", "items", "description"],
    ),
    (
        "/oneOf/0/properties/memory_flags/items",
        &["type", "properties", "required", "additionalProperties"],
    ),
    (
        "/oneOf/0/properties/memory_flags/items/properties",
        &["id", "claim", "reason", "evidence"],
    ),
    (
        "/oneOf/0/properties/memory_flags/items/properties/id",
        &["type", "minLength", "maxLength"],
    ),
    (
        "/oneOf/0/properties/memory_flags/items/properties/claim",
        &["type", "minLength", "maxLength", "description"],
    ),
    (
        "/oneOf/0/properties/memory_flags/items/properties/reason",
        &["type", "minLength", "maxLength"],
    ),
    (
        "/oneOf/0/properties/memory_flags/items/properties/evidence",
        &[
            "type",
            "properties",
            "required",
            "additionalProperties",
            "description",
        ],
    ),
    (
        "/oneOf/0/properties/memory_flags/items/properties/evidence/properties",
        &["path", "start_line", "end_line", "quote"],
    ),
    (
        "/oneOf/0/properties/memory_flags/items/properties/evidence/properties/path",
        &["type", "minLength", "maxLength"],
    ),
    (
        "/oneOf/0/properties/memory_flags/items/properties/evidence/properties/start_line",
        &["type", "minimum"],
    ),
    (
        "/oneOf/0/properties/memory_flags/items/properties/evidence/properties/end_line",
        &["type", "minimum"],
    ),
    (
        "/oneOf/0/properties/memory_flags/items/properties/evidence/properties/quote",
        &["type", "minLength", "maxLength"],
    ),
    ("/oneOf/0/properties/evidence_contract", &["$ref"]),
    (
        "/oneOf/0/properties/evidence_errors",
        &["type", "items", "uniqueItems", "maxItems", "description"],
    ),
    (
        "/oneOf/0/properties/evidence_errors/items",
        &["type", "minLength", "maxLength"],
    ),
    (
        "/oneOf/0/properties/merge_prerequisites",
        &["type", "uniqueItems", "items", "description"],
    ),
    (
        "/oneOf/0/properties/merge_prerequisites/items",
        &["type", "properties", "required", "additionalProperties"],
    ),
    (
        "/oneOf/0/properties/merge_prerequisites/items/properties",
        &["kind", "number", "source_quote"],
    ),
    (
        "/oneOf/0/properties/merge_prerequisites/items/properties/kind",
        &["enum"],
    ),
    (
        "/oneOf/0/properties/merge_prerequisites/items/properties/number",
        &["type", "minimum"],
    ),
    (
        "/oneOf/0/properties/merge_prerequisites/items/properties/source_quote",
        &["type", "minLength", "maxLength"],
    ),
    (
        "/oneOf/1",
        &["type", "properties", "required", "additionalProperties"],
    ),
    (
        "/oneOf/1/properties",
        &[
            "kind",
            "outcome",
            "reason",
            "protected_paths",
            "evidence_contract",
            "evidence_errors",
        ],
    ),
    ("/oneOf/1/properties/kind", &["const"]),
    ("/oneOf/1/properties/outcome", &["enum"]),
    ("/oneOf/1/properties/reason", &["type", "minLength"]),
    (
        "/oneOf/1/properties/protected_paths",
        &["type", "items", "minItems", "uniqueItems", "description"],
    ),
    (
        "/oneOf/1/properties/protected_paths/items",
        &["type", "minLength"],
    ),
    ("/oneOf/1/properties/evidence_contract", &["$ref"]),
    (
        "/oneOf/1/properties/evidence_errors",
        &["type", "items", "uniqueItems", "maxItems", "description"],
    ),
    (
        "/oneOf/1/properties/evidence_errors/items",
        &["type", "minLength", "maxLength"],
    ),
    (
        "/$defs",
        &[
            "forecast",
            "tool_capability",
            "evidence_requirement",
            "evidence_contract",
            "proof_obligation",
        ],
    ),
];
pub const FORECAST_ORDER: &[crate::contracts::render::ObjectOrder<'static>] = &[
    (
        "",
        &[
            "type",
            "properties",
            "required",
            "additionalProperties",
            "description",
        ],
    ),
    (
        "/properties",
        &[
            "tokenizer",
            "brief_tokens",
            "file_tokens",
            "overhead_tokens",
            "tokens",
            "fit_tokens",
            "context_tokens",
            "fits",
        ],
    ),
    ("/properties/tokenizer", &["const"]),
    ("/properties/brief_tokens", &["type", "minimum"]),
    ("/properties/file_tokens", &["type", "additionalProperties"]),
    (
        "/properties/file_tokens/additionalProperties",
        &["type", "minimum"],
    ),
    ("/properties/overhead_tokens", &["type", "minimum"]),
    ("/properties/tokens", &["type", "minimum"]),
    ("/properties/fit_tokens", &["type", "minimum"]),
    ("/properties/context_tokens", &["type", "minimum"]),
    ("/properties/fits", &["type"]),
];
pub const TOOL_CAPABILITY_ORDER: &[crate::contracts::render::ObjectOrder<'static>] = &[
    (
        "",
        &["type", "properties", "required", "additionalProperties"],
    ),
    (
        "/properties",
        &[
            "tool",
            "executor",
            "runner_features",
            "artifacts",
            "methods",
            "protected_files",
        ],
    ),
    ("/properties/tool", &["type", "minLength", "maxLength"]),
    ("/properties/executor", &["type", "minLength", "maxLength"]),
    (
        "/properties/runner_features",
        &["type", "items", "uniqueItems", "maxItems"],
    ),
    (
        "/properties/runner_features/items",
        &["type", "minLength", "maxLength"],
    ),
    (
        "/properties/artifacts",
        &["type", "items", "uniqueItems", "maxItems"],
    ),
    (
        "/properties/artifacts/items",
        &["type", "minLength", "maxLength"],
    ),
    (
        "/properties/methods",
        &["type", "items", "uniqueItems", "maxItems"],
    ),
    (
        "/properties/methods/items",
        &["type", "minLength", "maxLength"],
    ),
    (
        "/properties/protected_files",
        &["type", "items", "uniqueItems", "maxItems"],
    ),
    (
        "/properties/protected_files/items",
        &["type", "minLength", "maxLength"],
    ),
];
pub const EVIDENCE_REQUIREMENT_ORDER: &[crate::contracts::render::ObjectOrder<'static>] = &[
    (
        "",
        &["type", "properties", "required", "additionalProperties"],
    ),
    (
        "/properties",
        &[
            "tool",
            "executor",
            "runner_features",
            "artifacts",
            "methods",
            "protected_files",
            "requirement",
            "provenance",
        ],
    ),
    ("/properties/tool", &["type", "minLength", "maxLength"]),
    ("/properties/executor", &["type", "minLength", "maxLength"]),
    (
        "/properties/runner_features",
        &["type", "items", "uniqueItems", "maxItems"],
    ),
    (
        "/properties/runner_features/items",
        &["type", "minLength", "maxLength"],
    ),
    (
        "/properties/artifacts",
        &["type", "items", "uniqueItems", "maxItems"],
    ),
    (
        "/properties/artifacts/items",
        &["type", "minLength", "maxLength"],
    ),
    (
        "/properties/methods",
        &["type", "items", "uniqueItems", "maxItems"],
    ),
    (
        "/properties/methods/items",
        &["type", "minLength", "maxLength"],
    ),
    (
        "/properties/protected_files",
        &["type", "items", "uniqueItems", "maxItems"],
    ),
    (
        "/properties/protected_files/items",
        &["type", "minLength", "maxLength"],
    ),
    (
        "/properties/requirement",
        &["type", "minLength", "maxLength"],
    ),
    ("/properties/provenance", &["description", "oneOf"]),
    (
        "/properties/provenance/oneOf/0",
        &["type", "properties", "required", "additionalProperties"],
    ),
    (
        "/properties/provenance/oneOf/0/properties",
        &["kind", "source", "quote"],
    ),
    ("/properties/provenance/oneOf/0/properties/kind", &["const"]),
    (
        "/properties/provenance/oneOf/0/properties/source",
        &["type", "minLength", "maxLength"],
    ),
    (
        "/properties/provenance/oneOf/0/properties/quote",
        &["type", "minLength", "maxLength"],
    ),
    (
        "/properties/provenance/oneOf/1",
        &["type", "properties", "required", "additionalProperties"],
    ),
    ("/properties/provenance/oneOf/1/properties", &["kind"]),
    ("/properties/provenance/oneOf/1/properties/kind", &["const"]),
];
pub const EVIDENCE_CONTRACT_ORDER: &[crate::contracts::render::ObjectOrder<'static>] =
    &[("", &["type", "items", "maxItems"]), ("/items", &["$ref"])];
pub const PROOF_OBLIGATION_ORDER: &[crate::contracts::render::ObjectOrder<'static>] = &[
    (
        "",
        &[
            "type",
            "properties",
            "required",
            "additionalProperties",
            "description",
        ],
    ),
    (
        "/properties",
        &["kind", "observer", "subjects", "source", "quote"],
    ),
    ("/properties/kind", &["const"]),
    ("/properties/observer", &["enum"]),
    (
        "/properties/subjects",
        &["type", "minItems", "maxItems", "uniqueItems", "items"],
    ),
    (
        "/properties/subjects/items",
        &["type", "minLength", "maxLength", "pattern"],
    ),
    ("/properties/source", &["type", "minLength", "maxLength"]),
    ("/properties/quote", &["type", "minLength", "maxLength"]),
];
