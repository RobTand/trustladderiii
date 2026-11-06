//! Shared resolved refusal rows. Executable callbacks and adapter policy stay off wire.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

pub const EVIDENCE_ITEMS: usize = 16;
pub const EXCERPT_BYTES: usize = 4096;
pub const FAILING_TARGETS: usize = 64;
pub const INSTRUCTION_BYTES: usize = 1024;
pub const TRUNCATED: &str = "\n[truncated]";

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Role { Ingest, Audit, Planner, Maker, Reviewer, Classifier, Writer, Postmortem }

impl Role {
    pub const ALL: [Self; 8] = [Self::Ingest, Self::Audit, Self::Planner, Self::Maker, Self::Reviewer, Self::Classifier, Self::Writer, Self::Postmortem];
    pub fn as_str(self) -> &'static str {
        match self { Self::Ingest => "ingest", Self::Audit => "audit", Self::Planner => "planner", Self::Maker => "maker", Self::Reviewer => "reviewer", Self::Classifier => "classifier", Self::Writer => "writer", Self::Postmortem => "postmortem" }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RenderContext { #[default] Reply, Tool }

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(try_from = "String", into = "String")]
pub struct FieldPointer(String);
impl TryFrom<String> for FieldPointer {
    type Error = String;
    fn try_from(value: String) -> Result<Self, String> {
        if !value.starts_with('/') || value.chars().any(char::is_control) { return Err("field pointer must be absolute".into()); }
        let mut bytes = value.bytes();
        while let Some(byte) = bytes.next() {
            if byte == b'~' && !matches!(bytes.next(), Some(b'0' | b'1')) { return Err("invalid JSON pointer escape".into()); }
        }
        Ok(Self(value))
    }
}
impl From<FieldPointer> for String { fn from(value: FieldPointer) -> Self { value.0 } }
impl FieldPointer { pub fn as_str(&self) -> &str { &self.0 } }

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum NameKind { Field, Path, Symbol }

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Name { pub kind: NameKind, pub value: String }

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Allowed { Finite { values: Vec<Value> }, Schema { keyword: SchemaKeyword, value: Value } }

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum SchemaKeyword { Const, Enum, Type, Pattern, Minimum, Maximum, MinLength, MaxLength, MinItems, MaxItems, Required, AdditionalProperties }

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum EditOp { Add, Replace, Remove }

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Repair {
    Edit { pointer: FieldPointer, op: EditOp, allowed: Allowed },
    CantMeet { reason: String },
    SourceEdit { path: String, instruction: String },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Evidence {
    TestFailure { target: String, test: String, excerpt: String, log_ref: String, receipt: String },
    Attribution { batch_head: crate::bounded::CommitSha, member: String, member_head: crate::bounded::CommitSha, method: AttributionMethod },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum AttributionMethod {
    OutsideSelection { failing_targets: Vec<String>, #[serde(default, skip_serializing_if = "is_zero")] failing_targets_truncated: u64 },
    PrefixBisection { passed_prefix_head: crate::bounded::CommitSha, failed_prefix_head: crate::bounded::CommitSha },
}

fn is_zero(value: &u64) -> bool { *value == 0 }

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Violation {
    pub rule_id: String,
    pub role: Role,
    pub names: Vec<Name>,
    pub bad_value: Value,
    pub allowed: Allowed,
    pub repair: Repair,
    pub gating: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence: Vec<Evidence>,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub evidence_truncated: u64,
}

/// Adapter policy is not part of the serialized row or the schema.
pub struct TerminalForm { pub role: Role, pub predicate: fn(&Value) -> bool, pub phrase: &'static str }
pub struct Policy<'a> { pub terminals: &'a [TerminalForm], pub source_editors: &'a [Role] }

pub enum Callback<F, T> { Static(T), Facts(fn(&F) -> T), Role(fn(&F, Role) -> T) }
pub enum Witness { Shared(Value), ByRole(BTreeMap<Role, Value>) }
pub struct Registration<F> { pub example: F, pub reply: Value, pub witness: Witness }
pub struct Rule<F> {
    pub id: &'static str,
    pub check: fn(&F) -> Vec<F>,
    pub names: fn(&F) -> Vec<Name>,
    pub bad_value: fn(&F) -> Value,
    pub allowed: Callback<F, Allowed>,
    pub repair: Callback<F, Repair>,
    pub gating: bool,
    pub roles: Vec<Role>,
}

impl<F> Rule<F> {
    pub fn register(self, _fixture: &Registration<F>, _policy: &Policy<'_>) -> Result<Self, String> { todo!("C0 registration") }
    pub fn evaluate(&self, _facts: &F, _role: Role, _policy: &Policy<'_>) -> Result<Vec<Violation>, String> { todo!("C0 callback resolution") }
}

pub fn render_all(_violations: &[Violation], _role: Role, _context: RenderContext, _policy: &Policy<'_>) -> Result<String, String> { todo!("C0 resolved rendering") }
pub fn permits(_violations: &[Violation], _before: &Value, _after: &Value, _role: Role, _policy: &Policy<'_>) -> Result<bool, String> { todo!("C0 repair guard") }
