//! Typed maker-input wire contract (#819).

use crate::bounded::*;
use crate::render;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub type WireText = ReplyText<0, 1, UNBOUNDED>;
pub type Strings = List<WireText, 1, UNBOUNDED, true>;
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(inline, transform = reply_enum_schema)]
pub enum GoalsitemVerification {
    #[serde(rename = "test")]
    Test,
    #[serde(rename = "review")]
    Review,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(inline, transform = reply_enum_schema)]
pub enum GoalsitemStage {
    #[serde(rename = "maker")]
    Maker,
    #[serde(rename = "checking")]
    Checking,
    #[serde(rename = "publishing")]
    Publishing,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GoalsitemPublication {
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "ReplyText<0, 1, UNBOUNDED>")]
    pub branch: Option<ReplyText<0, 1, UNBOUNDED>>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "ReplyText<0, 1, UNBOUNDED>")]
    pub branch_prefix: Option<ReplyText<0, 1, UNBOUNDED>>,
    pub non_draft: True,
    pub body_lines: List<ReplyText<2, 1, 512>, 0, UNBOUNDED, true>,
    pub proof: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(extend("allOf" = serde_json::json!([{"if": {"required": ["stage"], "properties": {"stage": {"const": "publishing"}}}, "then": {"required": ["publication"]}, "else": {"not": {"required": ["publication"]}}}])))]
pub struct GoalsitemFields {
    #[schemars(with = "TextRef")]
    pub id: WireText,
    #[schemars(with = "TextRef")]
    pub description: WireText,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "GoalsitemVerification")]
    pub verification: Option<GoalsitemVerification>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "GoalsitemStage")]
    pub stage: Option<GoalsitemStage>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "GoalsitemPublication")]
    pub publication: Option<GoalsitemPublication>,

    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "ProofObligationRef")]
    pub proof_obligation: Option<crate::roles::ingest_reply::ProofObligation>,
}
pub type Goalsitem = Validated<GoalsitemFields, PublicationGoal>;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Plan {
    #[schemars(with = "StringsRef")]
    pub files: Strings,
    pub checks: List<crate::Check, 1, UNBOUNDED, false>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Round(Integer<0, 1>);
impl Round {
    pub fn new(value: u64) -> Result<Self, String> {
        Integer::new(value).map(Self)
    }
    pub fn get(&self) -> u64 {
        self.0.get()
    }
}
impl JsonSchema for Round {
    fn inline_schema() -> bool {
        true
    }
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "MakerRound".into()
    }
    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({"type":"integer","enum":[0,1]})
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(transform = crate::render::omit_open_additional_properties)]
pub struct CurrentReviewFindingsitem {
    #[schemars(with = "TextRef")]
    pub id: WireText,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "CommitSha")]
    pub reviewed_sha: Option<CommitSha>,
    #[serde(flatten)]
    extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(inline, transform = reply_enum_schema)]
pub enum FeedbackitemKind {
    #[serde(rename = "failed_check")]
    FailedCheck,
    #[serde(rename = "review_finding")]
    ReviewFinding,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FeedbackitemFailedTestsitem {
    #[schemars(with = "TextRef")]
    pub test: WireText,
    #[schemars(with = "TextRef", extend("maxLength" = 65536))]
    pub message: ReplyText<0, 1, { crate::report::PROOF_MESSAGE_CHARS }>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FeedbackitemCiChecksitem {
    #[schemars(with = "TextRef")]
    pub name: WireText,
    #[schemars(with = "TextRef", extend("maxLength" = 1000))]
    pub summary: ReplyText<0, 1, 1000>,
    #[serde(deserialize_with = "required_nullable")]
    #[schemars(required, with = "NullableType<Text<0, 4096>>")]
    pub url: Option<Text<0, 4096>>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(extend("dependentRequired" = serde_json::json!({"id": ["reviewed_sha"], "reviewed_sha": ["id"]})))]
pub struct FeedbackitemFields {
    pub kind: FeedbackitemKind,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "TextRef")]
    pub id: Option<WireText>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "CommitSha")]
    pub reviewed_sha: Option<CommitSha>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "Natural<1>")]
    pub resolved_round: Option<Natural<1>>,
    #[schemars(with = "TextRef", extend("maxLength" = 16000))]
    pub report: ReplyText<0, 1, 16000>,
    #[schemars(with = "List<TextRef, 0, UNBOUNDED, true>")]
    pub goal_ids: List<WireText, 0, UNBOUNDED, true>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "List<FeedbackitemFailedTestsitem, 0, 20, false>")]
    pub failed_tests: Option<List<FeedbackitemFailedTestsitem, 0, 20, false>>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "List<OutsidePathSchema, 1, 100, false>")]
    pub outside_paths: Option<List<ReplyText<0, 1, 4096>, 1, 100, false>>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "Natural<0>")]
    pub outside_paths_total: Option<Natural<0>>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "List<FeedbackitemCiChecksitem, 0, 20, false>")]
    pub ci_checks: Option<List<FeedbackitemCiChecksitem, 0, 20, false>>,
}
pub type Feedbackitem = Validated<FeedbackitemFields, FeedbackIdentity>;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Tests {
    pub image: ImageDigest,
    #[schemars(extend("minLength" = 1))]
    pub tools_path: AbsolutePath,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "Natural<1>")]
    pub cpus: Option<Natural<1>>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "Natural<1>")]
    pub mem_gb: Option<Natural<1>>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "Natural<1>")]
    pub quiet_seconds: Option<Natural<1>>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Hindsight {
    pub endpoint: HttpPrefix,
    #[schemars(with = "TextRef")]
    pub bank: WireText,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(extend("if" = serde_json::json!({"properties": {"round": {"const": 1}}})))]
#[schemars(extend("then" = serde_json::json!({"required": ["feedback"]})))]
pub struct MakerInputFields {
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "DecisionContextRef")]
    pub decision_context: Option<crate::decision_context::DecisionContext>,
    #[schemars(with = "TextRef")]
    pub brief: WireText,
    pub goals: List<Goalsitem, 1, UNBOUNDED, false>,
    pub plan: Plan,
    pub footprint: crate::footprint::Footprint,
    pub attempt: Integer<1, 3>,
    pub round: Round,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "CommitSha")]
    pub check_base_sha: Option<CommitSha>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "CommitSha")]
    pub reviewed_sha: Option<CommitSha>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "List<CurrentReviewFindingsitem, 0, UNBOUNDED, false>")]
    pub current_review_findings: Option<List<CurrentReviewFindingsitem, 0, UNBOUNDED, false>>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "List<Feedbackitem, 1, UNBOUNDED, false>")]
    pub feedback: Option<List<Feedbackitem, 1, UNBOUNDED, false>>,
    pub tests: Tests,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "Hindsight")]
    pub hindsight: Option<Hindsight>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "List<Text<1, UNBOUNDED>, 0, UNBOUNDED, false>")]
    pub protected_paths: Option<List<Text<1, UNBOUNDED>, 0, UNBOUNDED, false>>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "ForecastRef")]
    pub forecast: Option<crate::roles::planner_input::Forecast>,
    pub context_size: Natural<1>,
}
/// Serialize through `serde_json::to_value` for the producer's sorted keys.
/// Direct serialization follows field declaration order and differs.
pub type MakerInput = Validated<MakerInputFields, FixFeedback>;

impl Rule<GoalsitemFields> for PublicationGoal {
    fn validate(value: &GoalsitemFields) -> Result<(), String> {
        let publishing = matches!(value.stage, Some(GoalsitemStage::Publishing));
        if publishing == value.publication.is_some() {
            Ok(())
        } else {
            Err("publication is required exactly for publishing goals".into())
        }
    }
}
impl Rule<FeedbackitemFields> for FeedbackIdentity {
    fn validate(value: &FeedbackitemFields) -> Result<(), String> {
        if value.id.is_some() == value.reviewed_sha.is_some() {
            Ok(())
        } else {
            Err("feedback id and reviewed_sha must be supplied together".into())
        }
    }
}
impl Rule<MakerInputFields> for FixFeedback {
    fn validate(value: &MakerInputFields) -> Result<(), String> {
        if value.round.get() != 1 || value.feedback.is_some() {
            Ok(())
        } else {
            Err("round 1 requires feedback".into())
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct PublicationGoal;

#[derive(Clone, Debug, PartialEq)]
pub struct FeedbackIdentity;

#[derive(Clone, Debug, PartialEq)]
pub struct FixFeedback;

schema_ref!(DecisionContextRef, "decision-context.json");
schema_ref!(TextRef, "#/$defs/text");
schema_ref!(StringsRef, "#/$defs/strings");
schema_ref!(ForecastRef, "planner-input.json#/properties/forecast");
schema_ref!(
    ProofObligationRef,
    "ingest-reply.json#/$defs/proof_obligation"
);
pub fn schema() -> Value {
    let mut schema = super::input_schema::<MakerInput>("maker-input.json");
    let mut generator = schemars::generate::SchemaSettings::draft2020_12()
        .with(|s| s.inline_subschemas = true)
        .into_generator();
    schema["$defs"]["text"] = WireText::json_schema(&mut generator).to_value();
    schema["$defs"]["strings"] =
        <List<TextRef, 1, UNBOUNDED, true>>::json_schema(&mut generator).to_value();
    schema
}

pub struct OutsidePathSchema;
impl JsonSchema for OutsidePathSchema {
    fn inline_schema() -> bool {
        true
    }
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "OutsidePathSchema".into()
    }
    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({"$ref":"#/$defs/text","maxLength":4096})
    }
}

pub fn generated() -> Result<crate::Generated, render::RenderError> {
    Ok(crate::Generated {
        path: format!("{}/maker-input.json", crate::SCHEMA_DIRECTORY),
        text: serde_json::to_string_pretty(&schema()).expect("maker input schema serializes")
            + "\n",
    })
}
