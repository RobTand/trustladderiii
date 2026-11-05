//! Planner wire contract (#820). Draft proof omissions are retained for constrained repair.
use super::super::{
    bounded::{List, Text, UNBOUNDED, UnitPath, present},
    checks::{CheckKind, DeclaredSymptom, Forcing, ProofDeclaration},
};
use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Deserializer, Serialize, de::Error as _};
use serde_json::{Number, Value};
use std::{borrow::Cow, collections::BTreeMap};

// JSON Schema integers can be written as 1.0 and have no machine-width maximum.
// Keep their representation until the original forecast validator diagnoses it.
#[derive(Clone, Debug, Eq, Serialize)]
#[serde(transparent)]
pub struct PlanInteger<const MIN: u64>(Number);
// uniqueItems compares mathematical integers, including 1 and 1.0. Avoid
// rounding large u64 values through f64 when just one side is a float.
impl<const MIN: u64> PartialEq for PlanInteger<MIN> {
    fn eq(&self, other: &Self) -> bool {
        fn integer_float(integer: u64, number: &Number) -> bool {
            number.as_f64().is_some_and(|float| {
                (0.0..18_446_744_073_709_551_616.0).contains(&float) && float as u64 == integer
            })
        }
        match (self.0.as_u64(), other.0.as_u64()) {
            (Some(a), Some(b)) => a == b,
            (Some(a), None) => integer_float(a, &other.0),
            (None, Some(b)) => integer_float(b, &self.0),
            (None, None) => self.0.as_f64() == other.0.as_f64(),
        }
    }
}
impl<'de, const MIN: u64> Deserialize<'de> for PlanInteger<MIN> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let number = Number::deserialize(d)?;
        let value = number
            .as_f64()
            .ok_or_else(|| D::Error::custom("expected a finite integer"))?;
        if value.fract() != 0.0 || value < MIN as f64 {
            return Err(D::Error::custom("planner integer outside its contract"));
        }
        Ok(Self(number))
    }
}
impl<const MIN: u64> JsonSchema for PlanInteger<MIN> {
    fn inline_schema() -> bool {
        true
    }
    fn schema_name() -> Cow<'static, str> {
        format!("PlanInteger_{MIN}").into()
    }
    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({"type":"integer", "minimum": MIN})
    }
}

/// Undeclared differs from an explicitly empty declaration. Q54 and #683/#696 repair.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Declaration<T> {
    Undeclared,
    Declared(T),
}
impl<T> Default for Declaration<T> {
    fn default() -> Self {
        Self::Undeclared
    }
}
impl<T> Declaration<T> {
    pub fn is_undeclared(&self) -> bool {
        matches!(self, Self::Undeclared)
    }
    pub fn declared(&self) -> Option<&T> {
        match self {
            Self::Undeclared => None,
            Self::Declared(v) => Some(v),
        }
    }
}
impl<'de, T: Deserialize<'de>> Deserialize<'de> for Declaration<T> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        T::deserialize(d).map(Self::Declared)
    }
}
impl<T: Serialize> Serialize for Declaration<T> {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Declared(v) => v.serialize(s),
            Self::Undeclared => Err(serde::ser::Error::custom("undeclared proof field")),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Proof {
    #[serde(default, skip_serializing_if = "Declaration::is_undeclared")]
    pub subjects: Declaration<List<UnitPath, 0, 16, true>>,
    #[serde(default, skip_serializing_if = "Declaration::is_undeclared")]
    pub observer_support: Declaration<List<UnitPath, 0, 16, true>>,
    #[serde(default, skip_serializing_if = "Declaration::is_undeclared")]
    pub red_message: Declaration<Text<1, 160>>,
}
impl Proof {
    pub fn complete(&self) -> Option<ProofDeclaration> {
        Some(ProofDeclaration {
            subjects: self.subjects.declared()?.clone(),
            observer_support: self.observer_support.declared()?.clone(),
            red_message: self.red_message.declared()?.clone(),
        })
    }
}
// Accepted plans use the shared complete declaration schema. Drafts retain missing keys
// instead of inventing the Python validation-only PROOF_DECLARATION placeholder.
impl JsonSchema for Proof {
    fn schema_name() -> Cow<'static, str> {
        "proof".into()
    }
    fn json_schema(g: &mut SchemaGenerator) -> Schema {
        ProofDeclaration::json_schema(g)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct PlannerCheckKind(CheckKind);
impl PlannerCheckKind {
    pub fn get(&self) -> CheckKind {
        self.0
    }
}
impl<'de> Deserialize<'de> for PlannerCheckKind {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let kind = CheckKind::deserialize(d)?;
        if matches!(kind, CheckKind::Reproducing) {
            return Err(D::Error::custom("reproducing is not a planner check kind"));
        }
        Ok(Self(kind))
    }
}
impl JsonSchema for PlannerCheckKind {
    fn inline_schema() -> bool {
        true
    }
    fn schema_name() -> Cow<'static, str> {
        "PlannerCheckKind".into()
    }
    fn json_schema(g: &mut SchemaGenerator) -> Schema {
        let mut s = CheckKind::json_schema(g);
        s.remove("type");
        if let Some(Value::Array(kinds)) = s.get_mut("enum") {
            kinds.retain(|k| k != "reproducing");
        }
        s
    }
}
fn constant(s: &mut Schema) {
    s.remove("type");
    if let Some(Value::Array(mut v)) = s.remove("enum") {
        s.insert("const".into(), v.remove(0));
    }
}
fn enumeration(s: &mut Schema) {
    s.remove("type");
}
fn one_of(s: &mut Schema) {
    if let Some(v) = s.remove("anyOf") {
        s.insert("oneOf".into(), v);
    }
}
fn zero_min_items(s: &mut Schema) {
    s.insert("minItems".into(), 0.into());
}

// These definitions are owned by P1a/P1b. The packaged schemas validate them at the
// boundary; the wrapper preserves their raw fields until those typed roots are merged.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct DecisionContext(pub BTreeMap<String, Value>);
impl JsonSchema for DecisionContext {
    fn inline_schema() -> bool {
        true
    }
    fn schema_name() -> Cow<'static, str> {
        "DecisionContext".into()
    }
    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({"$ref":"decision-context.json"})
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct EvidenceContract(pub Vec<Value>);
impl JsonSchema for EvidenceContract {
    fn inline_schema() -> bool {
        true
    }
    fn schema_name() -> Cow<'static, str> {
        "EvidenceContract".into()
    }
    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({"$ref":"ingest-reply.json#/$defs/evidence_contract"})
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(rename = "execution")]
#[schemars(
    description = "Code-owned execution estimate. Does not replace the exact token forecast or grant fresh maker allowance."
)]
pub struct Execution {
    pub allowed_tool_turns: PlanInteger<0>,
    pub finalization_turns: PlanInteger<0>,
    pub usable_tool_turns: PlanInteger<0>,
    pub estimated_tool_turns: PlanInteger<0>,
    pub components: BTreeMap<String, PlanInteger<0>>,
    pub exploration_declared: bool,
    pub fits: bool,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "List<Execution,0,UNBOUNDED,false>")]
    pub pieces: Option<List<Execution, 0, UNBOUNDED, false>>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "PlanInteger<0>")]
    pub aggregate_tool_turns: Option<PlanInteger<0>>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "PlanInteger<0>")]
    pub aggregate_allowed_tool_turns: Option<PlanInteger<0>>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(inline, transform=constant)]
pub enum ForecastTokenizer {
    #[serde(rename = "o200k_base")]
    O200kBase,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(rename = "forecast")]
#[schemars(description = "Appended by the reply check. The model must omit this field.")]
pub struct Forecast {
    pub tokenizer: ForecastTokenizer,
    pub brief_tokens: PlanInteger<0>,
    pub file_tokens: BTreeMap<String, PlanInteger<0>>,
    pub overhead_tokens: PlanInteger<0>,
    pub tokens: PlanInteger<0>,
    pub fit_tokens: PlanInteger<1>,
    pub context_tokens: PlanInteger<1>,
    pub fits: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(inline, transform=constant)]
pub enum LeafKind {
    #[serde(rename = "leaf")]
    Leaf,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(inline)]
pub struct PlannerCheck {
    pub kind: PlannerCheckKind,
    #[schemars(
        description = "Exact pytest function or method selectors, validated by tl3_harness.Check: tests/test_api.py::test_status or tests/test_api.py::TestAPI::test_status. An unparameterized function selects all its cases. Whole files, directories and globs are forbidden. Regression selectors must name existing base definitions."
    )]
    #[schemars(transform=zero_min_items)]
    pub node_ids: List<Text<1, UNBOUNDED>, 0, UNBOUNDED, true>,
    #[schemars(description = "Zero-based positions in inputs.ingest.goals.")]
    #[schemars(transform=zero_min_items)]
    pub goal_ids: List<PlanInteger<0>, 0, UNBOUNDED, true>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "Forcing")]
    pub forcing: Option<Forcing>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "Proof")]
    pub proof: Option<Proof>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "DeclaredSymptom")]
    pub declared_symptom: Option<DeclaredSymptom>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(inline)]
#[schemars(
    description = "A verbatim quote of at most 40 lines from the read-only checkout, at these 1-based lines of this repo-relative file."
)]
pub struct Citation {
    pub path: Text<1, 1024>,
    pub start_line: PlanInteger<1>,
    pub end_line: PlanInteger<1>,
    pub quote: Text<8, 2000>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(inline)]
pub struct MemoryFlag {
    pub id: Text<1, 256>,
    #[schemars(
        description = "The memory's own words that the evidence contradicts, quoted verbatim from its content."
    )]
    pub claim: Text<8, 1000>,
    pub reason: Text<1, 1000>,
    #[schemars(
        description = "A verbatim quote of at most 40 lines from the read-only checkout, at these 1-based lines of this repo-relative file."
    )]
    pub evidence: Citation,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(inline)]
pub struct Convention {
    pub text: Text<1, 1000>,
    #[schemars(
        description = "A verbatim quote of at most 40 lines from the read-only checkout, at these 1-based lines of this repo-relative file."
    )]
    pub evidence: Citation,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(inline)]
pub struct Leaf {
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "PlanInteger<0>")]
    #[schemars(
        description = "Required for accepted leaves and splits. Estimate orientation reads, searches, globs and recall separately from implementation and proof. Zero must be explicit."
    )]
    pub estimated_exploration_turns: Option<PlanInteger<0>>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "PlanInteger<0>")]
    #[schemars(
        description = "Estimate all required tool work, including proof, metadata and final verification. Code may increase this estimate."
    )]
    pub estimated_tool_turns: Option<PlanInteger<0>>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "Execution")]
    #[schemars(description = "Appended by worker code. The model must omit this field.")]
    pub execution: Option<Execution>,
    pub kind: LeafKind,
    #[schemars(transform=zero_min_items)]
    pub files: List<Text<1, UNBOUNDED>, 0, UNBOUNDED, true>,
    #[schemars(transform=zero_min_items)]
    pub checks: List<PlannerCheck, 0, UNBOUNDED, false>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "List<Text<1,UNBOUNDED>,0,UNBOUNDED,true>")]
    #[schemars(
        description = "Existing tests, by pytest node ID, whose definitions the change modifies. Omit it when the change edits no existing test."
    )]
    #[schemars(transform=zero_min_items)]
    pub edits_tests: Option<List<Text<1, UNBOUNDED>, 0, UNBOUNDED, true>>,
    pub no_design_questions: bool,
    pub lands_on_its_own: bool,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "Forecast")]
    pub forecast: Option<Forecast>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "List<Text<1,UNBOUNDED>,0,UNBOUNDED,false>")]
    #[schemars(description = "Code-owned rule violations. The model must omit this field.")]
    pub violations: Option<List<Text<1, UNBOUNDED>, 0, UNBOUNDED, false>>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "List<Text<1,UNBOUNDED>,0,UNBOUNDED,true>")]
    #[schemars(description = "IDs of supplied memories this reply relied on.")]
    pub memory_ids: Option<List<Text<1, UNBOUNDED>, 0, UNBOUNDED, true>>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "List<MemoryFlag,0,3,false>")]
    #[schemars(
        description = "Cited memories the checkout shows to be wrong, each with the evidence that contradicts it. Code checks the quote before the memory is retired."
    )]
    pub memory_flags: Option<List<MemoryFlag, 0, 3, false>>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "List<Convention,0,3,false>")]
    #[schemars(
        description = "Repository conventions the checkout shows, each with evidence, kept for later requests. Code checks the quote."
    )]
    pub conventions: Option<List<Convention, 0, 3, false>>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "DecisionContext")]
    #[schemars(
        description = "Code supplies the applicable approvals and restrictions from the planner inputs."
    )]
    pub decision_context: Option<DecisionContext>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "EvidenceContract")]
    pub evidence_contract: Option<EvidenceContract>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "List<Text<1,8192>,0,1024,true>")]
    #[schemars(description = "Code-owned unsupported evidence requirements.")]
    pub evidence_errors: Option<List<Text<1, 8192>, 0, 1024, true>>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "List<BTreeMap<String,Value>,0,9,false>")]
    #[schemars(
        description = "Code-owned verified planner rehearsal receipts. Independent Checking is still required."
    )]
    pub forcing_rehearsals: Option<List<BTreeMap<String, Value>, 0, 9, false>>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(inline, transform=constant)]
pub enum SplitKind {
    #[serde(rename = "split")]
    Split,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(inline, transform=enumeration)]
pub enum SplitReason {
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
pub struct Piece {
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "PlanInteger<0>")]
    pub estimated_exploration_turns: Option<PlanInteger<0>>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "PlanInteger<0>")]
    pub estimated_tool_turns: Option<PlanInteger<0>>,
    pub brief: Text<1, UNBOUNDED>,
    #[schemars(description = "Zero-based positions in inputs.ingest.goals.")]
    #[schemars(transform=zero_min_items)]
    pub goal_ids: List<PlanInteger<0>, 0, UNBOUNDED, true>,
    #[schemars(transform=zero_min_items)]
    pub files: List<Text<1, UNBOUNDED>, 0, UNBOUNDED, true>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(inline)]
pub struct Split {
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "PlanInteger<0>")]
    #[schemars(
        description = "Required for accepted leaves and splits. Estimate orientation reads, searches, globs and recall separately from implementation and proof. Zero must be explicit."
    )]
    pub estimated_exploration_turns: Option<PlanInteger<0>>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "PlanInteger<0>")]
    #[schemars(
        description = "Estimate all required tool work, including proof, metadata and final verification. Code may increase this estimate."
    )]
    pub estimated_tool_turns: Option<PlanInteger<0>>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "Execution")]
    #[schemars(description = "Appended by worker code. The model must omit this field.")]
    pub execution: Option<Execution>,
    pub kind: SplitKind,
    pub reason: SplitReason,
    #[schemars(transform=zero_min_items)]
    pub pieces: List<Piece, 0, UNBOUNDED, false>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "Forecast")]
    pub forecast: Option<Forecast>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "List<Text<1,UNBOUNDED>,0,UNBOUNDED,false>")]
    #[schemars(description = "Code-owned rule violations. The model must omit this field.")]
    pub violations: Option<List<Text<1, UNBOUNDED>, 0, UNBOUNDED, false>>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "List<Text<1,UNBOUNDED>,0,UNBOUNDED,true>")]
    #[schemars(description = "IDs of supplied memories this reply relied on.")]
    pub memory_ids: Option<List<Text<1, UNBOUNDED>, 0, UNBOUNDED, true>>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "List<MemoryFlag,0,3,false>")]
    #[schemars(
        description = "Cited memories the checkout shows to be wrong, each with the evidence that contradicts it. Code checks the quote before the memory is retired."
    )]
    pub memory_flags: Option<List<MemoryFlag, 0, 3, false>>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "List<Convention,0,3,false>")]
    #[schemars(
        description = "Repository conventions the checkout shows, each with evidence, kept for later requests. Code checks the quote."
    )]
    pub conventions: Option<List<Convention, 0, 3, false>>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "DecisionContext")]
    #[schemars(
        description = "Code supplies the applicable approvals and restrictions from the planner inputs."
    )]
    pub decision_context: Option<DecisionContext>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "EvidenceContract")]
    pub evidence_contract: Option<EvidenceContract>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "List<Text<1,8192>,0,1024,true>")]
    #[schemars(description = "Code-owned unsupported evidence requirements.")]
    pub evidence_errors: Option<List<Text<1, 8192>, 0, 1024, true>>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "List<BTreeMap<String,Value>,0,9,false>")]
    #[schemars(
        description = "Code-owned verified planner rehearsal receipts. Independent Checking is still required."
    )]
    pub forcing_rehearsals: Option<List<BTreeMap<String, Value>, 0, 9, false>>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(inline, transform=constant)]
pub enum CantMeetKind {
    #[serde(rename = "cant_meet")]
    CantMeet,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(inline)]
pub struct CantMeet {
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "PlanInteger<0>")]
    #[schemars(
        description = "Required for accepted leaves and splits. Estimate orientation reads, searches, globs and recall separately from implementation and proof. Zero must be explicit."
    )]
    pub estimated_exploration_turns: Option<PlanInteger<0>>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "PlanInteger<0>")]
    #[schemars(
        description = "Estimate all required tool work, including proof, metadata and final verification. Code may increase this estimate."
    )]
    pub estimated_tool_turns: Option<PlanInteger<0>>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "Execution")]
    #[schemars(description = "Appended by worker code. The model must omit this field.")]
    pub execution: Option<Execution>,
    pub kind: CantMeetKind,
    pub report: Text<1, UNBOUNDED>,
    #[schemars(transform=zero_min_items)]
    pub files: List<Text<1, UNBOUNDED>, 0, UNBOUNDED, true>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "Forecast")]
    pub forecast: Option<Forecast>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "List<Text<1,UNBOUNDED>,0,UNBOUNDED,false>")]
    #[schemars(description = "Code-owned rule violations. The model must omit this field.")]
    pub violations: Option<List<Text<1, UNBOUNDED>, 0, UNBOUNDED, false>>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "List<Text<1,UNBOUNDED>,1,UNBOUNDED,true>")]
    #[schemars(
        description = "Concrete repository-relative paths that MUST change to meet the request's goals. Report only when the whole request cannot land usefully without these edits; never for inspected files, excluded edits, or an optional part. The controller verifies the effective protected-path list."
    )]
    pub protected_paths: Option<List<Text<1, UNBOUNDED>, 1, UNBOUNDED, true>>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "List<Text<1,UNBOUNDED>,0,UNBOUNDED,true>")]
    #[schemars(description = "IDs of supplied memories this reply relied on.")]
    pub memory_ids: Option<List<Text<1, UNBOUNDED>, 0, UNBOUNDED, true>>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "List<MemoryFlag,0,3,false>")]
    #[schemars(
        description = "Cited memories the checkout shows to be wrong, each with the evidence that contradicts it. Code checks the quote before the memory is retired."
    )]
    pub memory_flags: Option<List<MemoryFlag, 0, 3, false>>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "List<Convention,0,3,false>")]
    #[schemars(
        description = "Repository conventions the checkout shows, each with evidence, kept for later requests. Code checks the quote."
    )]
    pub conventions: Option<List<Convention, 0, 3, false>>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "DecisionContext")]
    #[schemars(
        description = "Code supplies the applicable approvals and restrictions from the planner inputs."
    )]
    pub decision_context: Option<DecisionContext>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "EvidenceContract")]
    pub evidence_contract: Option<EvidenceContract>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "List<Text<1,8192>,0,1024,true>")]
    #[schemars(description = "Code-owned unsupported evidence requirements.")]
    pub evidence_errors: Option<List<Text<1, 8192>, 0, 1024, true>>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "List<BTreeMap<String,Value>,0,9,false>")]
    #[schemars(
        description = "Code-owned verified planner rehearsal receipts. Independent Checking is still required."
    )]
    pub forcing_rehearsals: Option<List<BTreeMap<String, Value>, 0, 9, false>>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
#[schemars(transform=one_of)]
pub enum PlannerReply {
    Leaf(Leaf),
    Split(Split),
    CantMeet(CantMeet),
}
/// All values and constraints come from JsonSchema. The separate order table contains
/// keys only, because provider prompt text also depends on their insertion order.
pub fn schema() -> Value {
    let mut generator = schemars::generate::SchemaSettings::draft2020_12().into_generator();
    generator.subschema_for::<Execution>();
    generator.subschema_for::<Forecast>();
    generator.subschema_for::<Forcing>();
    generator.subschema_for::<Proof>();
    generator.subschema_for::<DeclaredSymptom>();
    let mut schema = generator.into_root_schema_for::<PlannerReply>().to_value();
    let definitions = schema["$defs"]
        .as_object()
        .expect("planner definitions")
        .clone();
    fn normalized(value: &mut Value, definitions: &serde_json::Map<String, Value>) {
        match value {
            Value::Object(object) => {
                if let Some(name) = object
                    .get("$ref")
                    .and_then(Value::as_str)
                    .and_then(|r| r.strip_prefix("#/$defs/"))
                    .map(str::to_owned)
                {
                    let keep = match name.as_str() {
                        "execution" | "forecast" | "proof" => Some(name.as_str()),
                        "Forcing" => Some("forcing"),
                        "DeclaredSymptom" => Some("declared_symptom"),
                        _ => None,
                    };
                    if let Some(keep) = keep {
                        object.insert("$ref".into(), format!("#/$defs/{keep}").into());
                    } else {
                        let mut inlined = definitions[&name].clone();
                        normalized(&mut inlined, definitions);
                        let siblings = std::mem::take(object);
                        *object = inlined.as_object().expect("derived object schema").clone();
                        object.extend(siblings.into_iter().filter(|(key, _)| key != "$ref"));
                    }
                }
                if object.get("additionalProperties") == Some(&Value::Bool(true)) {
                    object.remove("additionalProperties");
                }
                object.remove("title");
                object.remove("default");
                object.remove("format");
                if object.contains_key("const") || object.contains_key("enum") {
                    object.remove("type");
                }
                if let Some(Value::Object(names)) = object.get_mut("propertyNames") {
                    names.remove("type");
                }
                for child in object.values_mut() {
                    normalized(child, definitions);
                }
            }
            Value::Array(items) => items.iter_mut().for_each(|v| normalized(v, definitions)),
            _ => {}
        }
    }
    // Inline helper definitions; retain the five public cross-file pointers.
    schema
        .as_object_mut()
        .expect("planner schema object")
        .remove("$defs");
    normalized(&mut schema, &definitions);
    let mut kept = serde_json::Map::new();
    for (old, new) in [
        ("execution", "execution"),
        ("forecast", "forecast"),
        ("Forcing", "forcing"),
        ("proof", "proof"),
        ("DeclaredSymptom", "declared_symptom"),
    ] {
        let mut definition = definitions[old].clone();
        normalized(&mut definition, &definitions);
        kept.insert(new.into(), definition);
    }
    // List's zero lower bound is implicit; the published proof schema spells it out.
    kept["proof"]["properties"]["subjects"]["minItems"] = 0.into();
    let object = schema.as_object_mut().expect("planner schema object");
    object.insert("$defs".into(), kept.into());
    object.insert(
        "$id".into(),
        format!("{}planner-reply.json", super::super::SCHEMA_BASE).into(),
    );
    schema
}

pub fn schema_text() -> String {
    use super::super::render::{Layout, render};
    let entries: Vec<(String, Vec<String>)> =
        serde_json::from_str(include_str!("planner_reply_order.json"))
            .expect("planner key order metadata");
    let keys: Vec<Vec<&str>> = entries
        .iter()
        .map(|(_, keys)| keys.iter().map(String::as_str).collect())
        .collect();
    let inline: Vec<_> = entries
        .iter()
        .zip(&keys)
        .map(|((path, _), keys)| (path.as_str(), keys.as_slice()))
        .collect();
    render(
        "planner-reply.json",
        &schema(),
        &Layout {
            inline: &inline,
            shared: &[],
        },
    )
    .expect("planner schema key order is exhaustive")
}
