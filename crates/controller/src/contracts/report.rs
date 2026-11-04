// The trusted harness's check report: the `TL3_CHECKS` line `tl3.checks.v1` that the
// controller parses, stores, and later sends to review as `checks` (#741). The shapes
// follow what the harness emits (`harness/src/tl3_harness/execution.py::result_json`),
// pinned by the shared vectors in `harness/test-vectors/contracts`.

use super::bounded::{
    AppliedSeconds, ClippedText, CommitSha, Count, Integer, List, Nullable, RecordSchemaVersion,
    Sha256Hex, Signed, Text, True, UNBOUNDED, present, required_nullable,
};
use super::checks::{CheckKind, DeclaredSymptom, Forcing, ProofDeclaration};
use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize};
use std::collections::BTreeMap;

/// A test's retained failure message, in characters.
pub const MESSAGE_CHARS: usize = 1000;

// The report version the trusted action writes (action.py). A report can outlive the
// controller that staged its check: PrismaBuild actions finish across a deploy, and each
// controller stages only its own release's harness (checks::Harness::pin). So:
// - a controller accepts the current version and the previous one;
// - a report shape the previous controller would refuse, or that this controller would
//   refuse from the previous harness, gets a new version, and fields it adds stay
//   optional while the previous version is accepted;
// - the previous release's reports stay as vectors under
//   harness/test-vectors/contracts/released/<version>/, which the controller must parse.
// A version this controller does not know is refused, never guessed at (#750 review).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum ReportSchema {
    #[serde(rename = "tl3.checks.v1")]
    V1,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CheckReport {
    pub schema: ReportSchema,
    pub effect_key: String,
    pub base: CommitSha,
    pub head: CommitSha,
    #[serde(deserialize_with = "required_nullable")]
    #[schemars(with = "Nullable<CommitSha>")]
    pub reviewed: Option<CommitSha>,
    pub results: Vec<CheckResult>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum Verdict {
    Pass,
    Fail,
    InfrastructureFailure,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum Custody {
    InProcess,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CheckResult {
    pub name: Text<1, UNBOUNDED>,
    pub kind: CheckKind,
    pub verdict: Verdict,
    pub custody: Custody,
    pub tests: List<TestResult, 1, UNBOUNDED>,
    // Complete-unit admission and the unchecked suffix, retained for review (#591).
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "Selection")]
    pub selection: Option<Selection>,
    // A forced check echoes its forcing so review can judge it (#393).
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
    #[schemars(with = "ProofReceipt")]
    pub proof: Option<ProofReceipt>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "DeclaredSymptom")]
    pub declared_symptom: Option<DeclaredSymptom>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "SymptomEvidence")]
    pub symptom_evidence: Option<SymptomEvidence>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum Outcome {
    Passed,
    Assertion,
    AddedName,
    Error,
    Skipped,
    Missing,
    Uncollected,
    NoReport,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TestResult {
    pub test: Text<1, UNBOUNDED>,
    pub base: Outcome,
    pub head: Outcome,
    pub pre_existing: bool,
    pub passed: bool,
    #[serde(default)]
    pub message: ClippedText<MESSAGE_CHARS>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "Outcome")]
    pub head_forced: Option<Outcome>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "PhaseEvidence")]
    pub phases: Option<PhaseEvidence>,
}

// Phase facts per side; at least one side is present.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(extend("minProperties" = 1))]
pub struct PhaseEvidence {
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "PhaseRun")]
    base: Option<PhaseRun>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "PhaseRun")]
    head: Option<PhaseRun>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "PhaseRun")]
    head_forced: Option<PhaseRun>,
}

// The sides are private, so no caller can build the empty evidence the schema refuses.
impl PhaseEvidence {
    pub fn new(
        base: Option<PhaseRun>,
        head: Option<PhaseRun>,
        head_forced: Option<PhaseRun>,
    ) -> Result<Self, String> {
        if base.is_none() && head.is_none() && head_forced.is_none() {
            return Err("phase evidence needs at least one side".into());
        }
        Ok(Self {
            base,
            head,
            head_forced,
        })
    }

    pub fn base(&self) -> Option<&PhaseRun> {
        self.base.as_ref()
    }

    pub fn head(&self) -> Option<&PhaseRun> {
        self.head.as_ref()
    }

    pub fn head_forced(&self) -> Option<&PhaseRun> {
        self.head_forced.as_ref()
    }
}

impl<'de> Deserialize<'de> for PhaseEvidence {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Sides {
            #[serde(default, deserialize_with = "present")]
            base: Option<PhaseRun>,
            #[serde(default, deserialize_with = "present")]
            head: Option<PhaseRun>,
            #[serde(default, deserialize_with = "present")]
            head_forced: Option<PhaseRun>,
        }
        let sides = Sides::deserialize(deserializer)?;
        Self::new(sides.base, sides.head, sides.head_forced).map_err(serde::de::Error::custom)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PhaseRun {
    // A selector expanding to several cases retains its first failing case, or first passing case.
    pub test: Text<1, UNBOUNDED>,
    pub setup: PhaseFact,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "PhaseFact")]
    pub call: Option<PhaseFact>,
    pub teardown: PhaseFact,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PhaseFact {
    pub outcome: PhaseOutcome,
    pub exception: Text<0, 200>,
    pub assertion: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum PhaseOutcome {
    Passed,
    Failed,
    Skipped,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum ProofRule {
    #[serde(rename = "subject-observer-v1")]
    SubjectObserverV1,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum ProofMode {
    HeadObserver,
    PytestOutcome,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum RedMessageMatch {
    Message,
    SourceAssertion,
}

// The subject/observer proof receipt of a demonstrating or reproducing check.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProofReceipt {
    pub rule: ProofRule,
    #[serde(deserialize_with = "required_nullable")]
    #[schemars(with = "Nullable<ProofDeclaration>")]
    pub declaration: Option<ProofDeclaration>,
    pub mode: ProofMode,
    pub subjects: BTreeMap<String, SubjectDigests>,
    pub observer_sha256: BTreeMap<String, Option<Sha256Hex>>,
    pub support_sha256: BTreeMap<String, Sha256Hex>,
    pub source_base_tree_sha256: Sha256Hex,
    pub base_tree_sha256: Sha256Hex,
    pub head_tree_sha256: Sha256Hex,
    // How each observer's base failure matched the declared RED message (#709/#713).
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "BTreeMap<String, RedMessageMatch>")]
    pub red_message_matches: Option<BTreeMap<String, RedMessageMatch>>,
    // Per side, per test: the delay a delay patch actually applied (6c681fb319).
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "BTreeMap<String, BTreeMap<String, DelayBudget>>")]
    pub delay_budget: Option<BTreeMap<String, BTreeMap<String, DelayBudget>>>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SubjectDigests {
    #[serde(deserialize_with = "required_nullable")]
    #[schemars(with = "Nullable<Sha256Hex>")]
    pub base: Option<Sha256Hex>,
    #[serde(deserialize_with = "required_nullable")]
    #[schemars(with = "Nullable<Sha256Hex>")]
    pub head: Option<Sha256Hex>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DelayBudget {
    pub applied_seconds: AppliedSeconds,
    pub exhausted: bool,
}

// The harness's bounded complete-unit admission and unchecked-suffix receipt, present
// when the selected-test budget saturated (#359/#591).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Selection {
    pub policy: String,
    pub limit: Count,
    pub charged_cases: Count,
    pub base_units: Vec<SelectedUnit>,
    pub head_units: Vec<Vec<String>>,
    pub skipped_units: Vec<Vec<String>>,
    pub skipped_checked: bool,
    pub saturation: SelectionSaturation,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SelectedUnit {
    pub members: Vec<String>,
    pub cases: Count,
    pub outcomes: BTreeMap<String, Outcome>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SelectionSaturation {
    pub unit: Vec<String>,
    // Unknown when the unit was never collected.
    #[serde(deserialize_with = "required_nullable")]
    #[schemars(with = "Nullable<Count>")]
    pub cases: Option<Count>,
    pub remaining: Count,
    pub reason: String,
}

// A declared symptom's campaign evidence (#592/#626).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SymptomEvidence {
    pub base: CommitSha,
    pub head: CommitSha,
    #[serde(deserialize_with = "required_nullable")]
    #[schemars(with = "Nullable<Forcing>")]
    pub forcing: Option<Forcing>,
    pub declaration: DeclaredSymptom,
    pub runs: List<SymptomRun, 0, 30>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum SymptomSide {
    Base,
    Head,
    OrdinaryHead,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum RunOutcome {
    Passed,
    Symptom,
    Unrelated,
    Unavailable,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SymptomRun {
    pub side: SymptomSide,
    pub run: Integer<1, 10>,
    pub outcome: RunOutcome,
    // SHA-256 of the records' sorted, compact JSON.
    pub sha256: Sha256Hex,
    pub records: Vec<SymptomRecord>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "BTreeMap<String, Count>")]
    pub target_hits: Option<BTreeMap<String, Count>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum RecordOutcome {
    Passed,
    Failed,
    Error,
    Skipped,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum PhaseName {
    Setup,
    Call,
    Teardown,
}

// A pytest name the runner reported missing: a bare name or a (module, name) pair.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum MissingName {
    Name(String),
    Pair([String; 2]),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CollectionSaturation {
    pub unit: List<Text<1, UNBOUNDED>, 1, UNBOUNDED>,
    pub cases: Count,
    pub remaining: Count,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RecordFailure {
    // runner.py writes the full qualified class name, unclipped. test_tool.py refuses one
    // past 300 characters only after keeping (and hashing) the record, so the record is
    // evidence as written (#750 review finding 1).
    pub exception: Text<1, UNBOUNDED>,
    pub phase: PhaseName,
    pub source: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum ForcingValidation {
    #[serde(rename = "forcing_validation")]
    ForcingValidation,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
// forcing_ownership.py builds these from the patch target and the resolved module's name,
// unclipped; like RecordFailure, the record is kept as the runner wrote it.
pub struct ForcingRefusal {
    pub phase: ForcingValidation,
    pub target: String,
    pub resolved_module: String,
    pub owner_category: String,
    pub reason: String,
}

// One record of the runner's journal in a symptom run (runner.py, summary mode).
// `remote = "Self"` makes the derives inherent functions; the trait impls below wrap them,
// so every read of a record checks its exit code exactly.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(
    remote = "Self",
    tag = "event",
    rename_all = "kebab-case",
    deny_unknown_fields
)]
#[schemars(rename = "SymptomRecord")]
pub enum SymptomRecord {
    Start {
        schema: RecordSchemaVersion,
    },
    Finish {
        exit_code: Signed,
    },
    Collection {
        tests: Vec<Text<1, UNBOUNDED>>,
        #[serde(
            default,
            deserialize_with = "present",
            skip_serializing_if = "Option::is_none"
        )]
        #[schemars(with = "CollectionSaturation")]
        saturation: Option<CollectionSaturation>,
    },
    Result {
        test: Text<1, UNBOUNDED>,
        outcome: RecordOutcome,
        message: Text<0, MESSAGE_CHARS>,
        truncated: bool,
        #[serde(
            default,
            deserialize_with = "present",
            skip_serializing_if = "Option::is_none"
        )]
        #[schemars(with = "Vec<RecordFailure>")]
        failures: Option<Vec<RecordFailure>>,
        #[serde(
            default,
            deserialize_with = "present",
            skip_serializing_if = "Option::is_none"
        )]
        #[schemars(with = "BTreeMap<String, Count>")]
        target_hits: Option<BTreeMap<String, Count>>,
        #[serde(
            default,
            deserialize_with = "present",
            skip_serializing_if = "Option::is_none"
        )]
        #[schemars(with = "DelayBudget")]
        delay_budget: Option<DelayBudget>,
        #[serde(
            default,
            deserialize_with = "present",
            skip_serializing_if = "Option::is_none"
        )]
        #[schemars(with = "True")]
        forcing_tampered: Option<True>,
        #[serde(
            default,
            deserialize_with = "present",
            skip_serializing_if = "Option::is_none"
        )]
        #[schemars(with = "ForcingRefusal")]
        forcing_refusal: Option<ForcingRefusal>,
    },
    Phase {
        test: Text<1, UNBOUNDED>,
        when: PhaseName,
        outcome: PhaseOutcome,
        assertion: bool,
        #[serde(deserialize_with = "required_nullable")]
        #[schemars(with = "Nullable<Vec<MissingName>>")]
        missing: Option<Vec<MissingName>>,
        #[serde(
            default,
            deserialize_with = "present",
            skip_serializing_if = "Option::is_none"
        )]
        #[schemars(with = "Text<0, 300>")]
        exception: Option<Text<0, 300>>,
        #[serde(
            default,
            deserialize_with = "present",
            skip_serializing_if = "Option::is_none"
        )]
        #[schemars(with = "Text<0, 200>")]
        exception_type: Option<Text<0, 200>>,
        #[serde(
            default,
            deserialize_with = "present",
            skip_serializing_if = "Option::is_none"
        )]
        #[schemars(with = "Text<0, MESSAGE_CHARS>")]
        message: Option<Text<0, MESSAGE_CHARS>>,
    },
    CollectionError {
        #[serde(
            default,
            deserialize_with = "present",
            skip_serializing_if = "Option::is_none"
        )]
        #[schemars(with = "String")]
        test: Option<String>,
        // Absent, null and a list are three distinct records; keep them distinct.
        #[serde(
            default,
            deserialize_with = "present",
            skip_serializing_if = "Option::is_none"
        )]
        #[schemars(with = "Nullable<Vec<MissingName>>")]
        missing: Option<Option<Vec<MissingName>>>,
        #[serde(
            default,
            deserialize_with = "present",
            skip_serializing_if = "Option::is_none"
        )]
        #[schemars(with = "Text<0, 300>")]
        exception: Option<Text<0, 300>>,
        #[serde(
            default,
            deserialize_with = "present",
            skip_serializing_if = "Option::is_none"
        )]
        #[schemars(with = "Text<0, MESSAGE_CHARS>")]
        message: Option<Text<0, MESSAGE_CHARS>>,
        #[serde(
            default,
            deserialize_with = "present",
            skip_serializing_if = "Option::is_none"
        )]
        #[schemars(with = "bool")]
        truncated: Option<bool>,
    },
    Unavailable {
        message: Text<0, MESSAGE_CHARS>,
    },
    PlanError {
        message: Text<0, MESSAGE_CHARS>,
    },
}

impl Serialize for SymptomRecord {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        SymptomRecord::serialize(self, serializer)
    }
}

// SymptomRecord is internally tagged, so serde buffers it, and an exit code's exact token
// would be gone by the time Signed sees it. A record is therefore read field by field: the
// exit code as its raw token, every other field as a Value (exact for text and for Value
// input alike), then parsed from those Values and the token checked (#750 rounds 3 and 4).
impl<'de> Deserialize<'de> for SymptomRecord {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Fields;
        impl<'de> serde::de::Visitor<'de> for Fields {
            type Value = SymptomRecord;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("a symptom record")
            }

            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                mut map: A,
            ) -> Result<SymptomRecord, A::Error> {
                use serde::de::Error as _;
                use serde_json::{Value, value::RawValue};
                let mut fields = serde_json::Map::new();
                let mut exit_code = None;
                while let Some(key) = map.next_key::<String>()? {
                    let value = if key == "exit_code" {
                        let token: Box<RawValue> = map.next_value()?;
                        let value: Value =
                            serde_json::from_str(token.get()).map_err(A::Error::custom)?;
                        exit_code = Some(token);
                        value
                    } else {
                        map.next_value::<Value>()?
                    };
                    if fields.insert(key.clone(), value).is_some() {
                        return Err(A::Error::custom(format!("duplicate field `{key}`")));
                    }
                }
                let record =
                    SymptomRecord::deserialize(Value::Object(fields)).map_err(A::Error::custom)?;
                if let (SymptomRecord::Finish { .. }, Some(token)) = (&record, &exit_code) {
                    Signed::check_token(token.get()).map_err(A::Error::custom)?;
                }
                Ok(record)
            }
        }
        deserializer.deserialize_map(Fields)
    }
}
