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

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Ingest,
    Audit,
    Planner,
    Maker,
    Reviewer,
    Classifier,
    Writer,
    Postmortem,
}

impl Role {
    pub const ALL: [Self; 8] = [
        Self::Ingest,
        Self::Audit,
        Self::Planner,
        Self::Maker,
        Self::Reviewer,
        Self::Classifier,
        Self::Writer,
        Self::Postmortem,
    ];
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ingest => "ingest",
            Self::Audit => "audit",
            Self::Planner => "planner",
            Self::Maker => "maker",
            Self::Reviewer => "reviewer",
            Self::Classifier => "classifier",
            Self::Writer => "writer",
            Self::Postmortem => "postmortem",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RenderContext {
    #[default]
    Reply,
    Tool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct FieldPointer(String);
impl JsonSchema for FieldPointer {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "FieldPointer".into()
    }
    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({"type":"string","pattern":r"^(?:/(?:[^~]|~[01])*)?$"})
    }
}
impl TryFrom<String> for FieldPointer {
    type Error = String;
    fn try_from(value: String) -> Result<Self, String> {
        if !value.is_empty() && !value.starts_with('/') {
            return Err("field pointer must be absolute".into());
        }
        let mut bytes = value.bytes();
        while let Some(byte) = bytes.next() {
            if byte == b'~' && !matches!(bytes.next(), Some(b'0' | b'1')) {
                return Err("invalid JSON pointer escape".into());
            }
        }
        Ok(Self(value))
    }
}
impl From<FieldPointer> for String {
    fn from(value: FieldPointer) -> Self {
        value.0
    }
}
impl FieldPointer {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum NameKind {
    Field,
    Path,
    Symbol,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Name {
    pub kind: NameKind,
    pub value: String,
}

impl<'de> Deserialize<'de> for Name {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            kind: NameKind,
            value: String,
        }
        let wire = Wire::deserialize(deserializer)?;
        let name = Self {
            kind: wire.kind,
            value: wire.value,
        };
        name.validate().map_err(serde::de::Error::custom)?;
        Ok(name)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Allowed {
    Finite {
        values: Vec<Value>,
    },
    Schema {
        keyword: SchemaKeyword,
        value: Value,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum SchemaKeyword {
    Schema,
    Const,
    Enum,
    Type,
    Pattern,
    Minimum,
    Maximum,
    MinLength,
    MaxLength,
    MinItems,
    MaxItems,
    Required,
    AdditionalProperties,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum EditOp {
    Add,
    Replace,
    Remove,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Repair {
    Edit {
        pointer: FieldPointer,
        op: EditOp,
        allowed: Allowed,
    },
    CantMeet {
        reason: String,
    },
    SourceEdit {
        path: String,
        #[serde(deserialize_with = "instruction_from_wire")]
        #[schemars(length(min = 1, max = INSTRUCTION_BYTES), description = "Nonblank instruction of at most 1024 UTF-8 bytes. JSON Schema maxLength counts characters; Rust deserialization and validation enforce the byte cap.")]
        instruction: String,
    },
}

fn instruction_from_wire<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<String, D::Error> {
    let instruction = String::deserialize(deserializer)?;
    if instruction.trim().is_empty() || instruction.len() > INSTRUCTION_BYTES {
        return Err(serde::de::Error::custom(
            "source instruction must be nonblank and at most 1024 UTF-8 bytes",
        ));
    }
    Ok(instruction)
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Evidence {
    TestFailure {
        target: String,
        test: String,
        #[schemars(length(max = EXCERPT_BYTES), description = "At most 4096 UTF-8 bytes after Violation normalization, including newline+[truncated]. JSON Schema maxLength counts characters; Violation serialization/deserialization and rendering enforce the UTF-8 byte cap.")]
        excerpt: String,
        log_ref: String,
        receipt: String,
    },
    Attribution {
        batch_head: crate::bounded::CommitSha,
        member: String,
        member_head: crate::bounded::CommitSha,
        method: AttributionMethod,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum AttributionMethod {
    OutsideSelection {
        #[schemars(length(max = FAILING_TARGETS))]
        failing_targets: Vec<String>,
        #[serde(default, skip_serializing_if = "is_zero")]
        failing_targets_truncated: u64,
    },
    PrefixBisection {
        passed_prefix_head: crate::bounded::CommitSha,
        failed_prefix_head: crate::bounded::CommitSha,
    },
}

fn is_zero(value: &u64) -> bool {
    *value == 0
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, from = "ViolationWire", into = "ViolationWire")]
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

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ViolationWire {
    rule_id: String,
    role: Role,
    names: Vec<Name>,
    bad_value: Value,
    allowed: Allowed,
    repair: Repair,
    gating: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[schemars(length(max = EVIDENCE_ITEMS))]
    evidence: Vec<Evidence>,
    #[serde(default, skip_serializing_if = "is_zero")]
    evidence_truncated: u64,
}

impl From<Violation> for ViolationWire {
    fn from(mut row: Violation) -> Self {
        row.bound_evidence();
        Self {
            rule_id: row.rule_id,
            role: row.role,
            names: row.names,
            bad_value: row.bad_value,
            allowed: row.allowed,
            repair: row.repair,
            gating: row.gating,
            evidence: row.evidence,
            evidence_truncated: row.evidence_truncated,
        }
    }
}
impl From<ViolationWire> for Violation {
    fn from(row: ViolationWire) -> Self {
        let mut row = Self {
            rule_id: row.rule_id,
            role: row.role,
            names: row.names,
            bad_value: row.bad_value,
            allowed: row.allowed,
            repair: row.repair,
            gating: row.gating,
            evidence: row.evidence,
            evidence_truncated: row.evidence_truncated,
        };
        row.bound_evidence();
        row
    }
}

/// Adapter policy is not part of the serialized row or the schema.
pub struct TerminalForm {
    pub role: Role,
    pub predicate: fn(&Value) -> bool,
    pub phrase: &'static str,
}
/// A consuming JSON Schema validator handles constraints beyond enum/const/type.
/// The callback receives keyword, resolved constraint value, and candidate JSON value.
/// Return validation errors as Err; registration and guards propagate them.
pub type SchemaMembership = fn(SchemaKeyword, &Value, &Value) -> Result<bool, String>;
pub struct Policy<'a> {
    pub terminals: &'a [TerminalForm],
    pub source_editors: &'a [Role],
    pub schema_membership: Option<SchemaMembership>,
}

pub enum Callback<F, T> {
    Static(T),
    Facts(fn(&F) -> T),
    Role(fn(&F, Role) -> T),
}
pub enum Witness {
    Shared(Value),
    ByRole(BTreeMap<Role, Value>),
}
pub struct Registration<F> {
    pub example: F,
    pub reply: Value,
    pub witness: Witness,
    pub with_reply: fn(&F, &Value) -> F,
}
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

impl<F, T: Clone> Callback<F, T> {
    fn resolve(&self, facts: &F, role: Role) -> T {
        match self {
            Self::Static(value) => value.clone(),
            Self::Facts(callback) => callback(facts),
            Self::Role(callback) => callback(facts, role),
        }
    }
}

impl<F> Rule<F> {
    pub fn register(self, fixture: &Registration<F>, policy: &Policy<'_>) -> Result<Self, String> {
        if self.roles.is_empty() || !valid_id(self.id) {
            return Err("registration needs a dotted id and consuming roles".into());
        }
        if (matches!(self.allowed, Callback::Role(_)) || matches!(self.repair, Callback::Role(_)))
            && !matches!(fixture.witness, Witness::ByRole(_))
        {
            return Err("role callbacks require explicit role witnesses".into());
        }
        for role in &self.roles {
            let rows = self.evaluate(&fixture.example, *role, policy)?;
            if rows.is_empty() {
                return Err("registration needs a violating example".into());
            }
            render_all(&rows, *role, RenderContext::Reply, policy)?;
            let witness = match &fixture.witness {
                Witness::Shared(value) => value,
                Witness::ByRole(values) => {
                    values.get(role).ok_or("missing declared role witness")?
                }
            };
            if rows
                .iter()
                .all(|v| matches!(v.repair, Repair::SourceEdit { .. }))
            {
                if !witness.is_null() {
                    return Err("source-only repairs require a null witness for their role".into());
                }
                continue;
            }
            if !permits(&rows, &fixture.reply, witness, *role, policy)? {
                return Err("rendered repair rejects its witness".into());
            }
            if rows
                .iter()
                .any(|v| matches!(v.repair, Repair::CantMeet { .. }))
            {
                continue;
            }
            let unresolved = |reply: &Value| -> Result<bool, String> {
                Ok(self
                    .evaluate(
                        &(fixture.with_reply)(&fixture.example, reply),
                        *role,
                        policy,
                    )?
                    .iter()
                    .any(|row| !matches!(row.repair, Repair::SourceEdit { .. })))
            };
            if unresolved(witness)? {
                return Err("witness still fails a reply repair".into());
            }
            for row in &rows {
                if let Repair::Edit {
                    pointer,
                    op,
                    allowed,
                } = &row.repair
                {
                    if *op == EditOp::Remove {
                        continue;
                    }
                    for choice in allowed.alternatives() {
                        let candidate = apply(witness, pointer, EditOp::Replace, choice)?;
                        if !permits(&rows, &fixture.reply, &candidate, *role, policy)?
                            || unresolved(&candidate)?
                        {
                            return Err(
                                "advertised alternative still fails the guard or row".into()
                            );
                        }
                    }
                }
            }
        }
        Ok(self)
    }
    pub fn evaluate(
        &self,
        facts: &F,
        role: Role,
        policy: &Policy<'_>,
    ) -> Result<Vec<Violation>, String> {
        if !self.roles.contains(&role) {
            return Err(format!("undeclared role {} for {}", role.as_str(), self.id));
        }
        (self.check)(facts)
            .into_iter()
            .map(|facts| {
                let row = Violation {
                    rule_id: self.id.into(),
                    role,
                    names: (self.names)(&facts),
                    bad_value: (self.bad_value)(&facts),
                    allowed: self.allowed.resolve(&facts, role),
                    repair: self.repair.resolve(&facts, role),
                    gating: self.gating,
                    evidence: vec![],
                    evidence_truncated: 0,
                };
                row.validate(role, policy)?;
                Ok(row)
            })
            .collect()
    }
}

fn valid_id(id: &str) -> bool {
    id.contains('.')
        && id.split('.').all(|p| {
            p.as_bytes().first().is_some_and(u8::is_ascii_lowercase)
                && p.bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
        })
}
fn source_path(path: &str) -> bool {
    !path.is_empty()
        && !path.starts_with('/')
        && !path.contains(['\\', '\0', '\r', '\n'])
        && !path.split('/').any(|p| p == "..")
}
impl Name {
    pub fn validate(&self) -> Result<(), String> {
        let valid = match self.kind {
            NameKind::Field => self
                .value
                .clone()
                .try_into()
                .map(|_: FieldPointer| true)
                .unwrap_or(false),
            NameKind::Path => source_path(&self.value),
            NameKind::Symbol => self.value.split_once("::").is_some_and(|(path, symbol)| {
                source_path(path)
                    && (path.ends_with(".py") || path.ends_with(".rs"))
                    && symbol.split('.').all(|part| {
                        let mut chars = part.chars();
                        chars
                            .next()
                            .is_some_and(|c| c == '_' || unicode_ident::is_xid_start(c))
                            && chars.all(unicode_ident::is_xid_continue)
                    })
            }),
        };
        if valid {
            Ok(())
        } else {
            Err("invalid rule subject".into())
        }
    }
    fn render(&self) -> String {
        let label = match self.kind {
            NameKind::Field => "reply field",
            NameKind::Path => "source path",
            NameKind::Symbol => "symbol",
        };
        format!("{label} {}", serde_json::to_string(&self.value).unwrap())
    }
}
impl Allowed {
    fn alternatives(&self) -> Vec<&Value> {
        match self {
            Self::Finite { values } => values.iter().collect(),
            Self::Schema {
                keyword: SchemaKeyword::Enum,
                value: Value::Array(values),
            } => values.iter().collect(),
            Self::Schema {
                keyword: SchemaKeyword::Const,
                value,
            } => vec![value],
            _ => vec![],
        }
    }
    fn accepts(&self, value: &Value, policy: &Policy<'_>) -> Result<bool, String> {
        Ok(match self {
            Self::Finite { values } => values.contains(value),
            Self::Schema {
                keyword: SchemaKeyword::Const,
                value: expected,
            } => value == expected,
            Self::Schema {
                keyword: SchemaKeyword::Enum,
                value: Value::Array(choices),
            } => choices.contains(value),
            Self::Schema {
                keyword: SchemaKeyword::Type,
                value: Value::String(kind),
            } => match kind.as_str() {
                "null" => value.is_null(),
                "boolean" => value.is_boolean(),
                "string" => value.is_string(),
                "object" => value.is_object(),
                "array" => value.is_array(),
                "number" => value.is_number(),
                "integer" => value.as_number().is_some_and(|n| {
                    n.is_i64() || n.is_u64() || n.as_f64().is_some_and(|n| n.fract() == 0.0)
                }),
                _ => return Err("unknown JSON type".into()),
            },
            Self::Schema {
                keyword,
                value: constraint,
            } => {
                return policy
                    .schema_membership
                    .ok_or("schema membership requires the consuming schema validator")?(
                    *keyword, constraint, value,
                );
            }
        })
    }
    fn render(&self) -> String {
        match self {
            Self::Finite { values } => {
                format!("enum {}", display_json(&Value::Array(values.clone())))
            }
            Self::Schema { keyword, value } => format!(
                "{} {}",
                serde_json::to_value(keyword).unwrap().as_str().unwrap(),
                display_json(value)
            ),
        }
    }
    fn wording(&self) -> &'static str {
        match self {
            Self::Finite { .. }
            | Self::Schema {
                keyword: SchemaKeyword::Enum,
                ..
            } => "one of the allowed values above",
            Self::Schema {
                keyword: SchemaKeyword::Const,
                ..
            } => "the allowed value above",
            _ => "a value satisfying the allowed constraint above",
        }
    }
}
impl Policy<'_> {
    fn terminal(&self, role: Role) -> Result<&TerminalForm, String> {
        self.terminals
            .iter()
            .find(|form| form.role == role && !form.phrase.trim().is_empty())
            .ok_or_else(|| format!("no terminal form for {}", role.as_str()))
    }
}
impl Violation {
    pub fn validate(&self, role: Role, policy: &Policy<'_>) -> Result<(), String> {
        if self.role != role {
            return Err("violation role differs from evaluation role".into());
        }
        if !valid_id(&self.rule_id) || self.names.is_empty() {
            return Err("refusal needs a dotted rule and typed subjects".into());
        }
        for name in &self.names {
            name.validate()?;
        }
        match &self.repair {
            Repair::Edit { .. } => (),
            Repair::CantMeet { reason } => {
                if reason.trim().is_empty() {
                    return Err("terminal repair needs a reason".into());
                }
                policy.terminal(role)?;
            }
            Repair::SourceEdit { path, instruction } => {
                if !policy.source_editors.contains(&role)
                    || !source_path(path)
                    || instruction.trim().is_empty()
                    || instruction.len() > INSTRUCTION_BYTES
                {
                    return Err("source edit needs an editor, path and bounded instruction".into());
                }
                if self.names.iter().any(|n| match n.kind {
                    NameKind::Field => true,
                    NameKind::Path => &n.value != path,
                    NameKind::Symbol => n
                        .value
                        .split_once("::")
                        .is_none_or(|(subject, _)| subject != path),
                }) {
                    return Err("source edit path differs from its subjects".into());
                }
            }
        }
        Ok(())
    }
    /// Producers redact before this boundary. Omission counts survive round trips.
    pub fn with_evidence(mut self, evidence: Vec<Evidence>) -> Self {
        self.evidence = evidence;
        self.bound_evidence();
        self
    }
    fn bound_evidence(&mut self) {
        self.evidence_truncated = self
            .evidence_truncated
            .saturating_add(self.evidence.len().saturating_sub(EVIDENCE_ITEMS) as u64);
        self.evidence.truncate(EVIDENCE_ITEMS);
        for item in &mut self.evidence {
            match item {
                Evidence::TestFailure { excerpt, .. } => {
                    if excerpt.len() > EXCERPT_BYTES {
                        let mut end = EXCERPT_BYTES - TRUNCATED.len();
                        while !excerpt.is_char_boundary(end) {
                            end -= 1;
                        }
                        excerpt.truncate(end);
                        excerpt.push_str(TRUNCATED);
                    }
                }
                Evidence::Attribution {
                    method:
                        AttributionMethod::OutsideSelection {
                            failing_targets,
                            failing_targets_truncated,
                        },
                    ..
                } => {
                    *failing_targets_truncated = failing_targets_truncated.saturating_add(
                        failing_targets.len().saturating_sub(FAILING_TARGETS) as u64,
                    );
                    failing_targets.truncate(FAILING_TARGETS);
                }
                Evidence::Attribution { .. } => (),
            }
        }
    }
}
/// The caller parses context before aggregation, including empty arrays. Never clips rows.
pub fn render_all(
    violations: &[Violation],
    role: Role,
    context: RenderContext,
    policy: &Policy<'_>,
) -> Result<String, String> {
    let mut lines = vec![];
    for original in violations {
        let mut violation = original.clone();
        violation.bound_evidence();
        violation.validate(role, policy)?;
        let mut text = format!(
            "{}: {}.",
            violation.rule_id,
            violation
                .names
                .iter()
                .map(Name::render)
                .collect::<Vec<_>>()
                .join(", ")
        );
        let bad = display_json(&violation.bad_value);
        if bad.chars().count() <= 256 {
            text.push_str(&format!(" Bad value: {bad}."));
        }
        text.push_str(&format!(" Allowed: {}.", violation.allowed.render()));
        match &violation.repair {
            Repair::Edit {
                pointer,
                op,
                allowed,
            } => {
                if context == RenderContext::Tool {
                    return Err("tool refusal cannot edit a reply".into());
                }
                let operation = match op {
                    EditOp::Add => "add",
                    EditOp::Replace => "replace",
                    EditOp::Remove => "remove",
                };
                text.push_str(&format!(
                    " Permitted edit: {operation} reply field {}",
                    serde_json::to_string(pointer.as_str()).unwrap()
                ));
                if allowed != &violation.allowed {
                    text.push_str(&format!("; allowed: {}.", allowed.render()));
                } else if *op == EditOp::Remove {
                    text.push('.');
                } else {
                    text.push_str(&format!(" with {}.", allowed.wording()));
                }
            }
            Repair::CantMeet { reason } => {
                if context == RenderContext::Reply {
                    text.push_str(&format!(" {}", policy.terminal(role)?.phrase));
                }
                text.push_str(&format!(" Reason: {reason}"));
            }
            Repair::SourceEdit { path, instruction } => text.push_str(&format!(
                " Change {}: {}",
                serde_json::to_string(path).unwrap(),
                instruction
            )),
        }
        if violation.gating {
            text.push_str(" Further checks were not run.");
        }
        for evidence in &violation.evidence {
            match evidence {
                Evidence::TestFailure {
                    target,
                    test,
                    excerpt,
                    log_ref,
                    receipt,
                } => text.push_str(&format!(
                    "\nTest failure {} {}: {excerpt} [log {log_ref}; receipt {receipt}]",
                    serde_json::to_string(target).unwrap(),
                    serde_json::to_string(test).unwrap()
                )),
                Evidence::Attribution {
                    batch_head,
                    member,
                    member_head,
                    method,
                } => {
                    text.push_str(&format!(
                        "\nAttribution batch {} member {} head {}: ",
                        batch_head.as_str(),
                        serde_json::to_string(member).unwrap(),
                        member_head.as_str()
                    ));
                    match method {
                        AttributionMethod::OutsideSelection {
                            failing_targets,
                            failing_targets_truncated,
                        } => {
                            text.push_str(&format!(
                                "outside selection {}",
                                serde_json::to_string(failing_targets).unwrap()
                            ));
                            if *failing_targets_truncated != 0 {
                                text.push_str(&format!(" [truncated: {failing_targets_truncated} failing targets omitted]"));
                            }
                        }
                        AttributionMethod::PrefixBisection {
                            passed_prefix_head,
                            failed_prefix_head,
                        } => text.push_str(&format!(
                            "prefix bisection passed {} failed {}",
                            passed_prefix_head.as_str(),
                            failed_prefix_head.as_str()
                        )),
                    }
                }
            }
        }
        if violation.evidence_truncated != 0 {
            text.push_str(&format!(
                "\n[truncated: {} evidence items omitted]",
                violation.evidence_truncated
            ));
        }
        lines.push(text);
    }
    Ok(lines.join("\n"))
}
fn parts(pointer: &FieldPointer) -> Vec<String> {
    pointer
        .0
        .split('/')
        .skip(1)
        .map(|s| s.replace("~1", "/").replace("~0", "~"))
        .collect()
}
fn apply(
    document: &Value,
    pointer: &FieldPointer,
    op: EditOp,
    value: &Value,
) -> Result<Value, String> {
    let mut result = document.clone();
    let mut parts = parts(pointer);
    if parts.is_empty() {
        return if op == EditOp::Replace {
            Ok(value.clone())
        } else {
            Err("only replace is supported at the reply root".into())
        };
    }
    let key = parts.pop().ok_or("edit needs a field")?;
    let mut parent = &mut result;
    for part in parts {
        parent = match parent {
            Value::Object(fields) => fields.get_mut(&part),
            Value::Array(items) => part.parse::<usize>().ok().and_then(|n| items.get_mut(n)),
            _ => None,
        }
        .ok_or("edit parent is absent")?;
    }
    match parent {
        Value::Object(fields) => match op {
            EditOp::Add if !fields.contains_key(&key) => {
                fields.insert(key, value.clone());
            }
            EditOp::Replace if fields.contains_key(&key) => {
                fields.insert(key, value.clone());
            }
            EditOp::Remove if fields.contains_key(&key) => {
                fields.remove(&key);
            }
            _ => return Err("edit operation does not match field presence".into()),
        },
        Value::Array(items) => {
            let index = key.parse::<usize>().map_err(|_| "invalid array index")?;
            match op {
                EditOp::Add if index <= items.len() => items.insert(index, value.clone()),
                EditOp::Replace if index < items.len() => items[index] = value.clone(),
                EditOp::Remove if index < items.len() => {
                    items.remove(index);
                }
                _ => return Err("array edit out of bounds".into()),
            }
        }
        _ => return Err("edit parent must be a container".into()),
    }
    Ok(result)
}
pub fn permits(
    violations: &[Violation],
    before: &Value,
    after: &Value,
    role: Role,
    policy: &Policy<'_>,
) -> Result<bool, String> {
    for row in violations {
        row.validate(role, policy)?;
    }
    if violations
        .iter()
        .any(|v| matches!(v.repair, Repair::CantMeet { .. }))
    {
        return Ok((policy.terminal(role)?.predicate)(after));
    }
    let mut expected = before.clone();
    for row in violations {
        if let Repair::Edit {
            pointer,
            op,
            allowed,
        } = &row.repair
        {
            let value = if *op == EditOp::Remove {
                &Value::Null
            } else {
                match after.pointer(pointer.as_str()) {
                    Some(value) => value,
                    None => return Ok(false),
                }
            };
            if *op != EditOp::Remove && !allowed.accepts(value, policy)? {
                return Ok(false);
            }
            expected = match apply(&expected, pointer, *op, value) {
                Ok(value) => value,
                Err(_) => return Ok(false),
            };
        }
    }
    Ok(expected == *after)
}
pub fn evaluate<F>(
    rules: &[Rule<F>],
    facts: &F,
    role: Role,
    policy: &Policy<'_>,
) -> Result<Vec<Violation>, String> {
    let mut found = vec![];
    for rule in rules {
        let rows = rule.evaluate(facts, role, policy)?;
        let stop = rule.gating && !rows.is_empty();
        found.extend(rows);
        if stop {
            break;
        }
    }
    Ok(found)
}

// Match the existing refusal renderer's JSON separators and Unicode spelling.
fn display_json(value: &Value) -> String {
    match value {
        Value::Array(items) => format!(
            "[{}]",
            items
                .iter()
                .map(display_json)
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Value::Object(fields) => format!(
            "{{{}}}",
            fields
                .iter()
                .map(|(key, value)| format!(
                    "{}: {}",
                    serde_json::to_string(key).unwrap(),
                    display_json(value)
                ))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        _ => value.to_string(),
    }
}

/// The serialized boundary contains resolved data only. Rendering policy is external.
pub fn schema() -> Value {
    schemars::generate::SchemaSettings::draft2020_12()
        .into_generator()
        .into_root_schema_for::<Violation>()
        .to_value()
}

pub fn generated() -> Vec<crate::Generated> {
    let schema = format!(
        "{}\n",
        serde_json::to_string_pretty(&schema()).expect("schema serializes")
    );
    let docs = format!(
        "# Rust role contracts\n\nGenerated by `tl3-contracts-schemas` from the shared Rust form.\n\n`RoleContract` associates typed Input, Reply and Facts. Each role table is `Vec<Rule<F>>`; the default facts are `RoleFacts<Input, Reply>`. A consumer may supply a richer facts type without introducing a controller dependency. `validate_input` and `validate_reply` deserialize through the existing role types. `render(RenderOptions)` returns prompt suffix, model-facing schema and guidance. `evaluate(&Facts, &Policy)` returns resolved violations in check order and stops after a failing gating rule.\n\n`Rule<F>` has check `fn(&F) -> Vec<F>`, names `fn(&F) -> Vec<Name>`, bad_value `fn(&F) -> Value`, allowed and repair callbacks. `Callback` is Static, Facts `fn(&F) -> T`, or Role `fn(&F, Role) -> T`. `Registration<F>` carries example, complete before reply, witness and with_reply `fn(&F, &Value) -> F`. Role callbacks require explicit `Witness::ByRole` entries for every declared role. Mixed repairs resolve independently per role. Source-only rows require a null witness. A mixed SourceEdit and field-Edit set requires the complete edited reply witness; only unresolved SourceEdit rows may remain after rechecking that reply. CantMeet satisfies that role's terminal predicate, and Edit witnesses pass guard and check.\n\n`Policy` carries terminal predicates, source editors and optional schema_membership `fn(SchemaKeyword, &Value, &Value) -> Result<bool, String>`. The arguments are keyword, resolved constraint value and candidate JSON value. Adapter errors propagate through registration and guards, never fail open. Enum/const/type membership preserves strict JSON value types; other schema constraints require that consuming validator. Callbacks and policy never serialize. `render_all` takes the closed RenderContext enum at the call boundary, including empty aggregates. Reply is default; Tool omits CantMeet's terminal phrase, preserves SourceEdit and rejects Edit. `permits` grants only independent Edit pointers, with CantMeet terminal precedence. Empty evidence preserves refusal output.\n\nSourceEdit uses a repository-relative path matching its path/symbol subjects and a nonblank instruction of at most {INSTRUCTION_BYTES} UTF-8 bytes. The instruction byte cap is enforced at deserialization as well as validation. JSON Schema maxLength counts characters, so the schema description states the authoritative UTF-8 byte unit. This is an explicit new Rust boundary; no existing Python or harness API changes.\n\nEvidence is producer-redacted before construction. At most {EVIDENCE_ITEMS} records, {EXCERPT_BYTES} UTF-8 excerpt bytes and {FAILING_TARGETS} failing targets persist. Excerpt truncation includes literal newline plus `[truncated]` inside the byte cap. Whole-item overflow increments evidence_truncated; target overflow increments failing_targets_truncated. Counts persist across replay and render as omitted-count markers. Schema maxLength is a character bound; Violation normalization at serialization/deserialization and rendering enforces the excerpt UTF-8 byte cap. A schema-only validator must not treat character counts as byte counts. These limits never clip independent violations or the whole message. The harness R1 65536-character and byte-budget fail-loud boundaries remain with its adapter.\n\nLater stdlib-only Python adoption must retain role, evidence and order through proof_rules.violation_data and ProofRefusal.for_check. Backend labels/case identifiers belong in TestFailure.target/test, never Name.\n\nThe wire schema is [schema/violation.json](schema/violation.json). Rustdoc documents the callable interface. Four remaining role reply roots are generated in contracts; budget-wire and harness-wire retain their controller owners, quota-event retains QuotaEvent. `tl3_controller::contract_schemas::generated` aggregates all artifacts without reversing dependencies.\n"
    );
    vec![
        crate::Generated {
            path: "crates/contracts/schema/violation.json".into(),
            text: schema,
        },
        crate::Generated {
            path: "crates/contracts/ROLE_API.md".into(),
            text: docs,
        },
    ]
}
