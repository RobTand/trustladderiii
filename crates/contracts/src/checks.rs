// The check request the controller stages for the trusted harness (`request.json`), and
// the plan entries it carries. Forcing, proof and declared-symptom declarations are the
// planner's `$defs` of the same names (planner-reply.json); a test keeps them equal.

use super::bounded::{CommitSha, Nullable};
use super::bounded::{
    DelaySeconds, EnvironmentName, ExceptionName, Integer, List, Object, Parallel, PatchTarget,
    Text, UNBOUNDED, UnitPath, present, required_nullable,
};
use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error as _, ser::SerializeMap};
use std::borrow::Cow;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum CheckKind {
    Demonstrating,
    Regression,
    BehaviorPreserving,
    Reproducing,
}

// One planned check. `forcing`, `proof` and `declared_symptom` come from the plan and are
// echoed by the harness, which bounds and applies them in the container.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Check {
    pub name: String,
    pub kind: CheckKind,
    pub tests: Vec<String>,
    // A demonstrating check's forced reproduction (#393).
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
    #[schemars(with = "ProofDeclaration")]
    pub proof: Option<ProofDeclaration>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "DeclaredSymptom")]
    pub declared_symptom: Option<DeclaredSymptom>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(
    description = "A forced reproduction (#393): the declarative condition the harness applies, outside the test, to recreate a flake. Only on demonstrating checks."
)]
pub struct Forcing {
    pub symptom: Text<1, 1000>,
    pub environment: Object<EnvironmentName, Text<0, 1000>, 8>,
    pub patches: List<Patch, 0, 8>,
}

// One forcing patch: a dotted target and exactly one of its four modes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Patch {
    pub target: PatchTarget,
    pub mode: PatchMode,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PatchMode {
    // The replacement value is any JSON the planner chose; its shape is the patch's own.
    Value(serde_json::Value),
    Returns(serde_json::Value),
    Raises(Text<1, UNBOUNDED>),
    DelaySeconds(DelaySeconds),
}

impl PatchMode {
    pub const KEYS: [&'static str; 4] = ["value", "returns", "raises", "delay_seconds"];

    fn key(&self) -> &'static str {
        match self {
            Self::Value(_) => "value",
            Self::Returns(_) => "returns",
            Self::Raises(_) => "raises",
            Self::DelaySeconds(_) => "delay_seconds",
        }
    }
}

impl Serialize for Patch {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(2))?;
        map.serialize_entry("target", &self.target)?;
        match &self.mode {
            PatchMode::Value(value) | PatchMode::Returns(value) => {
                map.serialize_entry(self.mode.key(), value)?
            }
            PatchMode::Raises(text) => map.serialize_entry("raises", text)?,
            PatchMode::DelaySeconds(seconds) => map.serialize_entry("delay_seconds", seconds)?,
        }
        map.end()
    }
}

impl<'de> Deserialize<'de> for Patch {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let mut fields = serde_json::Map::<String, serde_json::Value>::deserialize(deserializer)?;
        let target = fields
            .remove("target")
            .ok_or_else(|| D::Error::missing_field("target"))?;
        let target = PatchTarget::deserialize(target).map_err(D::Error::custom)?;
        let (key, value) = match fields.into_iter().collect::<Vec<_>>().as_slice() {
            [(key, value)] => (key.clone(), value.clone()),
            _ => {
                return Err(D::Error::custom(
                    "a patch names its target and exactly one of value, returns, raises or delay_seconds",
                ));
            }
        };
        let mode = match key.as_str() {
            "value" => PatchMode::Value(value),
            "returns" => PatchMode::Returns(value),
            "raises" => PatchMode::Raises(Text::deserialize(value).map_err(D::Error::custom)?),
            "delay_seconds" => {
                PatchMode::DelaySeconds(DelaySeconds::deserialize(value).map_err(D::Error::custom)?)
            }
            other => return Err(D::Error::unknown_field(other, &PatchMode::KEYS)),
        };
        Ok(Self { target, mode })
    }
}

impl JsonSchema for Patch {
    fn inline_schema() -> bool {
        true
    }

    fn schema_name() -> Cow<'static, str> {
        "Patch".into()
    }

    fn json_schema(generator: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "type": "object",
            "properties": {
                "target": generator.subschema_for::<PatchTarget>(),
                "value": {},
                "returns": {},
                "raises": generator.subschema_for::<Text<1, UNBOUNDED>>(),
                "delay_seconds": generator.subschema_for::<DelaySeconds>(),
            },
            "required": ["target"],
            "minProperties": 2,
            "maxProperties": 2,
            "additionalProperties": false,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProofDeclaration {
    pub subjects: List<UnitPath, 0, 16, true>,
    pub observer_support: List<UnitPath, 0, 16, true>,
    pub red_message: Text<1, 160>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum SymptomPhase {
    Call,
    Setup,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DeclaredSymptom {
    pub node: Text<1, UNBOUNDED>,
    pub exception: ExceptionName,
    pub phase: SymptomPhase,
    pub repeat: Integer<1, 10>,
    pub parallel: Parallel,
}

// The staged `request.json`. The controller writes it with sorted keys, as before.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CheckRequest {
    pub effect_key: String,
    pub base: CommitSha,
    pub head: CommitSha,
    #[serde(deserialize_with = "required_nullable")]
    #[schemars(with = "Nullable<CommitSha>")]
    pub reviewed: Option<CommitSha>,
    pub plan: Vec<Check>,
    pub image: String,
    pub protected_paths: Vec<String>,
    pub selected_failures: Vec<String>,
    pub always_tests: Vec<String>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "u32")]
    pub selected_tests: Option<u32>,
}
