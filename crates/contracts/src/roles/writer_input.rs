//! Typed writer-input wire contract (#819).

use crate::bounded::*;
use crate::render::{self, Layout, SchemaOrder, TypeOrder, TypeUse};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(inline, transform = reply_enum_schema)]
pub enum DefWorkerInputFailureRole {
    #[serde(rename = "ingest")]
    Ingest,
    #[serde(rename = "audit")]
    Audit,
    #[serde(rename = "planner")]
    Planner,
    #[serde(rename = "maker")]
    Maker,
    #[serde(rename = "reviewer")]
    Reviewer,
    #[serde(rename = "classifier")]
    Classifier,
    #[serde(rename = "writer")]
    Writer,
    #[serde(rename = "postmortem")]
    Postmortem,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkerInputFailure {
    pub role: DefWorkerInputFailureRole,
    pub pointer: Text<0, 320>,
    pub rule: Text<1, 48>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "CommitSha")]
    pub maker_head: Option<CommitSha>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(inline, transform = reply_enum_schema)]
pub enum QuestionFactsFindingsSource {
    #[serde(rename = "audit")]
    Audit,
    #[serde(rename = "planner")]
    Planner,
    #[serde(rename = "maker")]
    Maker,
    #[serde(rename = "checks_and_review")]
    ChecksAndReview,
    #[serde(rename = "incident")]
    Incident,
    #[serde(rename = "credit")]
    Credit,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct QuestionFactsFindingsPiecesitem {
    pub brief: String,
    pub files: List<String, 0, UNBOUNDED, false>,
}

/// The deciding worker's findings (#322). The worker redacts them before the model turn.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct QuestionFactsFindings {
    pub source: QuestionFactsFindingsSource,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "String")]
    pub report: Option<String>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "String")]
    pub outcome: Option<String>,

    #[schemars(with = "WriterTextRef")]
    pub decision: crate::roles::writer_reply::Text,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "String")]
    pub reading: Option<String>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "List<String, 0, UNBOUNDED, false>")]
    pub goals: Option<List<String, 0, UNBOUNDED, false>>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "List<String, 0, UNBOUNDED, false>")]
    pub objections: Option<List<String, 0, UNBOUNDED, false>>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "List<QuestionFactsFindingsPiecesitem, 0, UNBOUNDED, false>")]
    pub pieces: Option<List<QuestionFactsFindingsPiecesitem, 0, UNBOUNDED, false>>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct QuestionFacts {
    #[schemars(with = "WriterTextRef")]
    pub issue_title: crate::roles::writer_reply::Text,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "WorkerInputFailureRef")]
    pub worker_input_failure: Option<WorkerInputFailure>,

    #[schemars(with = "WriterTextRef")]
    pub request: crate::roles::writer_reply::Text,

    #[schemars(with = "WriterTextRef")]
    pub obstacle: crate::roles::writer_reply::Text,

    #[schemars(with = "OptionsRef")]
    pub options: crate::roles::writer_reply::Options,
    pub recommended_option: Integer<0, 2>,

    #[schemars(with = "WriterTextRef")]
    pub recommendation_reason: crate::roles::writer_reply::Text,

    #[schemars(with = "WriterTextRef")]
    pub how_to_answer: crate::roles::writer_reply::Text,
    /// The deciding worker's findings (#322). The worker redacts them before the model turn.
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "QuestionFactsFindings")]
    pub findings: Option<QuestionFactsFindings>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NoteFacts {
    #[schemars(with = "WriterTextRef")]
    pub issue_title: crate::roles::writer_reply::Text,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "WorkerInputFailureRef")]
    pub worker_input_failure: Option<WorkerInputFailure>,

    #[schemars(with = "WriterTextRef")]
    pub request: crate::roles::writer_reply::Text,

    #[schemars(with = "WriterTextRef")]
    pub change: crate::roles::writer_reply::Text,

    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "WriterTextRef")]
    pub change_quote: Option<crate::roles::writer_reply::Text>,
    /// Safe code-owned fields. Code appends these after the checked draft.
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "JsonObject")]
    pub publication_diagnostic: Option<JsonObject>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "Natural<1>")]
    pub occurrences: Option<Natural<1>>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(inline, transform = reply_enum_schema)]
pub enum SummaryFactsEvidenceStatus {
    #[serde(rename = "captured")]
    Captured,
    #[serde(rename = "not_captured")]
    NotCaptured,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(inline, transform = reply_enum_schema)]
pub enum SummaryFactsMergeStatus {
    #[serde(rename = "merged")]
    Merged,
    #[serde(rename = "not_recorded")]
    NotRecorded,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(inline, transform = reply_enum_schema)]
pub enum SummaryFactsFindingsSource {
    #[serde(rename = "planner")]
    Planner,
    #[serde(rename = "maker")]
    Maker,
    #[serde(rename = "checks_and_review")]
    ChecksAndReview,
}

/// Evidence behind a stop for TL3 review (Q54). The worker redacts it before the model turn.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SummaryFactsFindings {
    pub source: SummaryFactsFindingsSource,
    pub report: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SummaryFacts {
    #[schemars(with = "WriterTextRef")]
    pub issue_title: crate::roles::writer_reply::Text,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "WorkerInputFailureRef")]
    pub worker_input_failure: Option<WorkerInputFailure>,

    #[schemars(with = "WriterTextRef")]
    pub request: crate::roles::writer_reply::Text,

    #[schemars(with = "WriterTextRef")]
    pub outcome: crate::roles::writer_reply::Text,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "SummaryFactsEvidenceStatus")]
    pub evidence_status: Option<SummaryFactsEvidenceStatus>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "SummaryFactsMergeStatus")]
    pub merge_status: Option<SummaryFactsMergeStatus>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "String")]
    pub recorded_summary: Option<String>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "ReplyNullable<JsonObject>")]
    pub pull_request: Option<Option<JsonObject>>,
    /// Recorded checks, verdicts, remaining failures, artifacts, review, audit, queue and scratch. Missing records mean not captured, never not run.
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "ReplyNullable<JsonObject>")]
    pub evidence: Option<Option<JsonObject>>,

    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "WriterTextRef")]
    pub removed_changes: Option<crate::roles::writer_reply::Text>,
    /// Evidence behind a stop for TL3 review (Q54). The worker redacts it before the model turn.
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "SummaryFactsFindings")]
    pub findings: Option<SummaryFactsFindings>,

    #[schemars(with = "List<WriterTextRef, 0, UNBOUNDED, false>")]
    pub unmet_goals: List<crate::roles::writer_reply::Text, 0, UNBOUNDED, false>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ClarificationFacts {
    #[schemars(with = "WriterTextRef")]
    pub issue_title: crate::roles::writer_reply::Text,

    #[schemars(with = "WriterTextRef")]
    pub request: crate::roles::writer_reply::Text,

    #[schemars(with = "WriterTextRef")]
    pub question: crate::roles::writer_reply::Text,

    #[schemars(with = "WriterTextRef")]
    pub comment_excerpt: crate::roles::writer_reply::Text,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", deny_unknown_fields)]
/// Serialize through `serde_json::to_value` for the producer's sorted keys.
/// Direct enum serialization follows tag and field declaration order and differs.
pub enum WriterInput {
    #[serde(rename = "question")]
    Question {
        facts: QuestionFacts,

        #[serde(
            default,
            deserialize_with = "present",
            skip_serializing_if = "Option::is_none"
        )]
        violations: Option<List<crate::roles::writer_reply::Text, 0, UNBOUNDED, false>>,
    },
    #[serde(rename = "note")]
    Note {
        facts: NoteFacts,

        #[serde(
            default,
            deserialize_with = "present",
            skip_serializing_if = "Option::is_none"
        )]
        violations: Option<List<crate::roles::writer_reply::Text, 0, UNBOUNDED, false>>,
    },
    #[serde(rename = "summary")]
    Summary {
        facts: SummaryFacts,

        #[serde(
            default,
            deserialize_with = "present",
            skip_serializing_if = "Option::is_none"
        )]
        violations: Option<List<crate::roles::writer_reply::Text, 0, UNBOUNDED, false>>,
    },
    #[serde(rename = "clarification")]
    Clarification {
        facts: ClarificationFacts,

        #[serde(
            default,
            deserialize_with = "present",
            skip_serializing_if = "Option::is_none"
        )]
        violations: Option<List<crate::roles::writer_reply::Text, 0, UNBOUNDED, false>>,
    },
}

schema_ref!(WriterTextRef, "writer-reply.json#/$defs/text");
schema_ref!(OptionsRef, "writer-reply.json#/$defs/options");
schema_ref!(WorkerInputFailureRef, "#/$defs/worker_input_failure");

pub fn schema() -> Value {
    let mut schema = super::input_schema::<WriterInput>("writer-input.json");
    let mut generator = schemars::generate::SchemaSettings::draft2020_12()
        .with(|s| s.inline_subschemas = true)
        .into_generator();
    schema["$defs"]["worker_input_failure"] =
        WorkerInputFailure::json_schema(&mut generator).to_value();
    schema
}

#[derive(JsonSchema)]
#[schemars(inline, transform = reply_enum_schema)]
pub enum WriterKind {
    #[serde(rename = "question")]
    Question,
    #[serde(rename = "note")]
    Note,
    #[serde(rename = "summary")]
    Summary,
    #[serde(rename = "clarification")]
    Clarification,
}

// JsonSchema's tagged-enum output repeats the envelope in every branch. The
// published contract factors it once; the fact schemas still come from each arm.
impl JsonSchema for WriterInput {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "WriterInput".into()
    }
    fn json_schema(g: &mut schemars::SchemaGenerator) -> schemars::Schema {
        let facts = [
            QuestionFacts::json_schema(g),
            NoteFacts::json_schema(g),
            SummaryFacts::json_schema(g),
            ClarificationFacts::json_schema(g),
        ];
        let branches: Vec<_>=["question","note","summary","clarification"].into_iter().zip(facts)
            .map(|(kind,facts)| serde_json::json!({"properties":{"kind":{"const":kind},"facts":facts}})).collect();
        schemars::json_schema!({"type":"object","properties":{"kind":WriterKind::json_schema(g),"facts":JsonObject::json_schema(g),"violations":List::<WriterTextRef,0,UNBOUNDED>::json_schema(g)},"required":["kind","facts"],"additionalProperties":false,"oneOf":branches})
    }
}

// Order-only metadata for the published input schema.
impl SchemaOrder for WriterInput {
    fn order() -> TypeOrder<'static> {
        TypeOrder::of::<Self>(&[
            (
                "",
                &[
                    "$schema",
                    "$id",
                    "type",
                    "properties",
                    "required",
                    "additionalProperties",
                    "oneOf",
                    "$defs",
                ],
            ),
            ("/properties", &["kind", "facts", "violations"]),
            ("/properties/violations", &["type", "items"]),
            ("/oneOf/0/properties", &["kind", "facts"]),
            ("/oneOf/1/properties", &["kind", "facts"]),
            ("/oneOf/2/properties", &["kind", "facts"]),
            ("/oneOf/3/properties", &["kind", "facts"]),
        ])
    }
}
impl SchemaOrder for QuestionFacts {
    fn order() -> TypeOrder<'static> {
        TypeOrder::of::<Self>(&[
            (
                "",
                &["type", "properties", "required", "additionalProperties"],
            ),
            (
                "/properties",
                &[
                    "issue_title",
                    "worker_input_failure",
                    "request",
                    "obstacle",
                    "options",
                    "recommended_option",
                    "recommendation_reason",
                    "how_to_answer",
                    "findings",
                ],
            ),
        ])
    }
}
impl SchemaOrder for QuestionFactsFindings {
    fn order() -> TypeOrder<'static> {
        TypeOrder::of::<Self>(&[
            (
                "",
                &[
                    "description",
                    "type",
                    "properties",
                    "required",
                    "additionalProperties",
                ],
            ),
            (
                "/properties",
                &[
                    "source",
                    "report",
                    "outcome",
                    "decision",
                    "reading",
                    "goals",
                    "objections",
                    "pieces",
                ],
            ),
            ("/properties/goals", &["type", "items"]),
            ("/properties/objections", &["type", "items"]),
            ("/properties/pieces", &["type", "items"]),
        ])
    }
}
impl SchemaOrder for QuestionFactsFindingsPiecesitem {
    fn order() -> TypeOrder<'static> {
        TypeOrder::of::<Self>(&[
            (
                "",
                &["type", "properties", "required", "additionalProperties"],
            ),
            ("/properties", &["brief", "files"]),
            ("/properties/files", &["type", "items"]),
        ])
    }
}
impl SchemaOrder for NoteFacts {
    fn order() -> TypeOrder<'static> {
        TypeOrder::of::<Self>(&[
            (
                "",
                &["type", "properties", "required", "additionalProperties"],
            ),
            (
                "/properties",
                &[
                    "issue_title",
                    "worker_input_failure",
                    "request",
                    "change",
                    "change_quote",
                    "publication_diagnostic",
                    "occurrences",
                ],
            ),
            (
                "/properties/publication_diagnostic",
                &["type", "description"],
            ),
        ])
    }
}
impl SchemaOrder for SummaryFacts {
    fn order() -> TypeOrder<'static> {
        TypeOrder::of::<Self>(&[
            (
                "",
                &["type", "properties", "required", "additionalProperties"],
            ),
            (
                "/properties",
                &[
                    "issue_title",
                    "worker_input_failure",
                    "request",
                    "outcome",
                    "evidence_status",
                    "merge_status",
                    "recorded_summary",
                    "pull_request",
                    "evidence",
                    "removed_changes",
                    "findings",
                    "unmet_goals",
                ],
            ),
            ("/properties/evidence", &["type", "description"]),
            ("/properties/unmet_goals", &["type", "items"]),
        ])
    }
}
impl SchemaOrder for SummaryFactsFindings {
    fn order() -> TypeOrder<'static> {
        TypeOrder::of::<Self>(&[
            (
                "",
                &[
                    "description",
                    "type",
                    "properties",
                    "required",
                    "additionalProperties",
                ],
            ),
            ("/properties", &["source", "report"]),
        ])
    }
}
impl SchemaOrder for ClarificationFacts {
    fn order() -> TypeOrder<'static> {
        TypeOrder::of::<Self>(&[
            (
                "",
                &["type", "properties", "required", "additionalProperties"],
            ),
            (
                "/properties",
                &["issue_title", "request", "question", "comment_excerpt"],
            ),
        ])
    }
}
impl SchemaOrder for WorkerInputFailure {
    fn order() -> TypeOrder<'static> {
        TypeOrder::of::<Self>(&[
            (
                "",
                &["type", "properties", "required", "additionalProperties"],
            ),
            ("/properties", &["role", "pointer", "rule", "maker_head"]),
            ("/properties/maker_head", &["type", "pattern"]),
        ])
    }
}
pub fn generated() -> Result<crate::Generated, render::RenderError> {
    let order0 = <WriterInput>::order();
    let order1 = <QuestionFacts>::order();
    let order2 = <Integer<0, 2>>::order();
    let order3 = <QuestionFactsFindings>::order();
    let order4 = <QuestionFactsFindingsPiecesitem>::order();
    let order5 = <NoteFacts>::order();
    let order6 = <Natural<1>>::order();
    let order7 = <SummaryFacts>::order();
    let order8 = <SummaryFactsFindings>::order();
    let order9 = <ClarificationFacts>::order();
    let order10 = <WorkerInputFailure>::order();
    let order11 = <Text<0, 320>>::order();
    let order12 = <Text<1, 48>>::order();
    let path = format!("{}/writer-input.json", crate::SCHEMA_DIRECTORY);
    let text = render::render(
        &path,
        &schema(),
        &Layout {
            inline: &[],
            shared: &[
                TypeUse {
                    pointer: "",
                    order: &order0,
                },
                TypeUse {
                    pointer: "/oneOf/0/properties/facts",
                    order: &order1,
                },
                TypeUse {
                    pointer: "/oneOf/0/properties/facts/properties/recommended_option",
                    order: &order2,
                },
                TypeUse {
                    pointer: "/oneOf/0/properties/facts/properties/findings",
                    order: &order3,
                },
                TypeUse {
                    pointer: "/oneOf/0/properties/facts/properties/findings/properties/pieces/items",
                    order: &order4,
                },
                TypeUse {
                    pointer: "/oneOf/1/properties/facts",
                    order: &order5,
                },
                TypeUse {
                    pointer: "/oneOf/1/properties/facts/properties/occurrences",
                    order: &order6,
                },
                TypeUse {
                    pointer: "/oneOf/2/properties/facts",
                    order: &order7,
                },
                TypeUse {
                    pointer: "/oneOf/2/properties/facts/properties/findings",
                    order: &order8,
                },
                TypeUse {
                    pointer: "/oneOf/3/properties/facts",
                    order: &order9,
                },
                TypeUse {
                    pointer: "/$defs/worker_input_failure",
                    order: &order10,
                },
                TypeUse {
                    pointer: "/$defs/worker_input_failure/properties/pointer",
                    order: &order11,
                },
                TypeUse {
                    pointer: "/$defs/worker_input_failure/properties/rule",
                    order: &order12,
                },
            ],
        },
    )?;
    Ok(crate::Generated { path, text })
}
