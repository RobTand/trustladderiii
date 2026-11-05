//! Controller-to-worker contracts. The final conversion through `Value` retains
//! the sorted key order used by request and prompt-cache identities (#730).
mod decision;
mod wire;
mod writer;

pub use decision::{Context, Decision};
pub use wire::*;
pub use writer::{MessageKind, WriteFor, WriteRequest};

use schemars::{JsonSchema, Schema, SchemaGenerator};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de::DeserializeOwned};
use serde_json::Value;
use std::borrow::Cow;

/// Read a stored wire value into its contract without normalizing optional
/// fields or clipping evidence on the way back out (#717).
#[derive(Clone, Debug, PartialEq)]
pub struct Lossless<T> {
    wire: Value,
    typed: T,
}

impl<T: DeserializeOwned> Lossless<T> {
    pub fn parse(wire: Value) -> Result<Self, serde_json::Error> {
        Ok(Self {
            typed: serde_json::from_value(wire.clone())?,
            wire,
        })
    }
    pub fn typed(&self) -> &T {
        &self.typed
    }
}
impl<T> Serialize for Lossless<T> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.wire.serialize(serializer)
    }
}
impl<'de, T: DeserializeOwned> Deserialize<'de> for Lossless<T> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Self::parse(Value::deserialize(d)?).map_err(serde::de::Error::custom)
    }
}
impl<T: JsonSchema> JsonSchema for Lossless<T> {
    // maker-reply.json references reviewer-input.json#/properties/checks/items/
    // properties/proof. Keep that public schema pointer while deriving the shape.
    fn inline_schema() -> bool {
        true
    }
    fn schema_name() -> Cow<'static, str> {
        T::schema_name()
    }
    fn json_schema(g: &mut SchemaGenerator) -> Schema {
        T::json_schema(g)
    }
}

/// Serialize through the same ordered map as the old `json!` builders.
pub fn value<T: Serialize>(input: T) -> Result<Value, serde_json::Error> {
    serde_json::to_value(input)
}

/// External materials are parsed once at the request boundary. The role tag is
/// supplied by the controller, rather than inferred from overlapping shapes.
pub fn role_wire(role: &str, input: Value) -> Result<Value, serde_json::Error> {
    value(RoleInput::parse(role, input)?)
}

fn schema<T: JsonSchema>(name: &str) -> super::Generated {
    let mut schema = schemars::generate::SchemaSettings::draft2020_12()
        .into_generator()
        .into_root_schema_for::<T>()
        .to_value();
    let object = schema
        .as_object_mut()
        .expect("a struct schema is an object");
    object.remove("title");
    object.insert(
        "$id".into(),
        format!("{}{name}.json", super::SCHEMA_BASE).into(),
    );
    super::strip_integer_formats(&mut schema);
    super::Generated {
        path: format!("{}/{name}.json", super::SCHEMA_DIRECTORY),
        text: super::render(&schema),
    }
}

pub fn generated() -> Vec<super::Generated> {
    vec![
        schema::<WorkerRequest>("request"),
        schema::<IngestInput>("ingest-input"),
        schema::<AuditInput>("audit-input"),
        schema::<PlannerInput>("planner-input"),
        schema::<MakerInput>("maker-input"),
        schema::<ReviewerInput>("reviewer-input"),
        schema::<WriterInput>("writer-input"),
        schema::<ClassifierInput>("classifier-input"),
        schema::<PostmortemInput>("postmortem-input"),
        schema::<Context>("decision-context"),
    ]
}

#[cfg(test)]
mod tests;

#[derive(Serialize)]
pub struct Trace {
    repo: String,
    issue: u64,
    request: String,
    attempt: u8,
    round: u8,
}
impl Trace {
    pub fn new(repo: String, issue: u64, request: String, attempt: u8, round: u8) -> Self {
        Self {
            repo,
            issue,
            request,
            attempt,
            round,
        }
    }
    pub fn into_map(self) -> Result<std::collections::BTreeMap<String, Value>, serde_json::Error> {
        serde_json::from_value(value(self)?)
    }
}

#[derive(Serialize)]
pub struct CaseRequest {
    pub node: u64,
    pub repo: String,
    pub issue: u64,
    pub title: String,
    pub text: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RoleInput {
    Ingest(IngestInput),
    Audit(AuditInput),
    Planner(PlannerInput),
    Maker(MakerInput),
    Reviewer(ReviewerInput),
    Writer(WriterInput),
    Classifier(ClassifierInput),
    Postmortem(PostmortemInput),
}
impl RoleInput {
    pub fn parse(role: &str, input: Value) -> Result<Self, serde_json::Error> {
        macro_rules! parse {
            ($variant:ident) => {
                serde_json::from_value(input).map(Self::$variant)
            };
        }
        match role {
            "ingest" => parse!(Ingest),
            "audit" => parse!(Audit),
            "planner" => parse!(Planner),
            "maker" => parse!(Maker),
            "reviewer" => parse!(Reviewer),
            "writer" => parse!(Writer),
            "classifier" => parse!(Classifier),
            "postmortem" => parse!(Postmortem),
            _ => Err(serde::de::Error::custom("unknown worker role")),
        }
    }
}

/// Typed projection of a planner check into the trusted check executor's input.
pub fn harness_check(plan: &Value) -> Result<crate::contracts::Check, serde_json::Error> {
    Ok(crate::contracts::Check {
        name: serde_json::from_value(plan["name"].clone())?,
        kind: serde_json::from_value(plan["kind"].clone())?,
        tests: serde_json::from_value(plan["tests"].clone())?,
        forcing: plan
            .get("forcing")
            .cloned()
            .map(serde_json::from_value)
            .transpose()?,
        declared_symptom: plan
            .get("declared_symptom")
            .cloned()
            .map(serde_json::from_value)
            .transpose()?,
        proof: plan
            .get("proof")
            .cloned()
            .map(serde_json::from_value)
            .transpose()?,
    })
}

#[derive(Serialize)]
pub struct CheckRun<'a> {
    pub repository: &'a std::path::Path,
    pub base_sha: &'a str,
    pub plan: Vec<crate::contracts::Check>,
}

#[derive(Serialize)]
pub struct CurrentFinding {
    pub id: Value,
    pub reviewed_sha: Value,
}

/// Use Path's serde implementation, including its refusal of non-UTF-8 paths.
/// Lossy display conversion would silently change request identity.
pub trait WirePath {
    fn wire_path(&self) -> Result<String, serde_json::Error>;
}
impl<T: AsRef<std::path::Path>> WirePath for T {
    fn wire_path(&self) -> Result<String, serde_json::Error> {
        serde_json::from_value(value(self.as_ref())?)
    }
}
