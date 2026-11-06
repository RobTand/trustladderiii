//! Typed role contracts consumed by renderers, constructors and rule-table adapters.
pub mod guidance;

use crate::{
    roles,
    rules::{self, Policy, Role, Rule, Violation},
};
use schemars::JsonSchema;
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};

/// Tables attach role semantics without coupling contracts to a controller or harness.
pub trait RoleContract {
    type Input: DeserializeOwned + Serialize + JsonSchema;
    type Reply: DeserializeOwned + Serialize + JsonSchema;
    type Facts;
    const ROLE: Role;
    fn input_schema() -> Value;
    fn reply_schema() -> Value;
    fn reply_schema_text() -> String;
    fn rules(&self) -> &[Rule<Self::Facts>];

    fn validate_input(document: Value) -> Result<Self::Input, serde_json::Error> {
        serde_json::from_value(document)
    }
    fn validate_reply(document: Value) -> Result<Self::Reply, serde_json::Error> {
        serde_json::from_value(document)
    }
    fn evaluate(&self, facts: &Self::Facts, policy: &Policy<'_>) -> Result<Vec<Violation>, String> {
        rules::evaluate(self.rules(), facts, Self::ROLE, policy)
    }
    fn render(&self, options: RenderOptions) -> Result<Rendered, String> {
        render(Self::ROLE, Self::reply_schema(), options)
    }
}

/// Native threads are sampled by the adapter at its existing call point.
#[derive(Clone, Copy)]
pub struct RenderOptions {
    pub test_cpus: u64,
    pub generation: bool,
}
impl Default for RenderOptions {
    fn default() -> Self {
        Self {
            test_cpus: 1,
            generation: false,
        }
    }
}

/// Inputs and the candidate reply travel together for role checks. Consumers may
/// choose their own facts type when a check also needs repository observations.
#[derive(Clone)]
pub struct RoleFacts<I, R> {
    pub input: I,
    pub reply: R,
}

pub struct Rendered {
    pub prompt_suffix: String,
    pub model_schema: Value,
    pub guidance: &'static str,
}

macro_rules! contract {
    ($name:ident, $role:ident, $input:ident, $input_type:ident, $reply:ident, $reply_type:ident) => {
        pub struct $name<F = RoleFacts<roles::$input::$input_type, roles::$reply::$reply_type>> {
            pub rules: Vec<Rule<F>>,
        }
        impl<F> RoleContract for $name<F> {
            type Input = roles::$input::$input_type;
            type Reply = roles::$reply::$reply_type;
            type Facts = F;
            const ROLE: Role = Role::$role;
            fn input_schema() -> Value {
                roles::$input::schema()
            }
            fn reply_schema() -> Value {
                roles::$reply::schema_value()
            }
            fn reply_schema_text() -> String {
                roles::$reply::schema_text()
            }
            fn rules(&self) -> &[Rule<Self::Facts>] {
                &self.rules
            }
        }
    };
}
contract!(
    Ingest,
    Ingest,
    ingest_input,
    IngestInput,
    ingest_reply,
    IngestReply
);
contract!(
    Audit,
    Audit,
    audit_input,
    AuditInput,
    audit_reply,
    AuditReply
);
contract!(
    Planner,
    Planner,
    planner_input,
    PlannerInput,
    planner_reply,
    PlannerReply
);
contract!(
    Maker,
    Maker,
    maker_input,
    MakerInput,
    maker_reply,
    MakerReply
);
contract!(
    Reviewer,
    Reviewer,
    reviewer_input,
    ReviewerInput,
    reviewer_reply,
    ReviewerReply
);
contract!(
    Classifier,
    Classifier,
    classifier_input,
    ClassifierInput,
    classifier_reply,
    ClassifierReply
);
contract!(
    Writer,
    Writer,
    writer_input,
    WriterInput,
    writer_reply,
    WriterReply
);
// The postmortem input generator exposes the same root as schema_value below.
pub struct Postmortem<
    F = RoleFacts<
        roles::postmortem_input::PostmortemInput,
        roles::postmortem_reply::PostmortemReply,
    >,
> {
    pub rules: Vec<Rule<F>>,
}
impl<F> RoleContract for Postmortem<F> {
    type Input = roles::postmortem_input::PostmortemInput;
    type Reply = roles::postmortem_reply::PostmortemReply;
    type Facts = F;
    const ROLE: Role = Role::Postmortem;
    fn input_schema() -> Value {
        roles::postmortem_input::schema()
    }
    fn reply_schema() -> Value {
        roles::postmortem_reply::schema_value()
    }
    fn reply_schema_text() -> String {
        roles::postmortem_reply::schema_text()
    }
    fn rules(&self) -> &[Rule<Self::Facts>] {
        &self.rules
    }
}

fn render(role: Role, mut schema: Value, options: RenderOptions) -> Result<Rendered, String> {
    let mut suffix = String::new();
    let mut append = |text: &str| {
        suffix.push_str("\n\n");
        suffix.push_str(text);
    };
    if matches!(role, Role::Ingest | Role::Audit | Role::Planner) {
        append(&guidance::forcing(&options.test_cpus.to_string()));
    }
    if role == Role::Planner {
        append(guidance::PLANNER);
    }
    if role == Role::Audit {
        append(&ingest_inventory()?);
    }
    if options.generation && matches!(role, Role::Ingest | Role::Planner | Role::Maker) {
        append(guidance::GENERATION);
    }
    if role == Role::Maker {
        maker_proposal(&mut schema)?;
    }
    if role == Role::Planner {
        let declaration: Value =
            serde_json::from_str(guidance::EXPLORATION).map_err(|e| e.to_string())?;
        for (key, value) in declaration.as_object().unwrap() {
            schema[key] = value.clone();
        }
    }
    let owned: &[&str] = match role {
        Role::Ingest => &["forecast", "evidence_errors"],
        Role::Audit => &["evidence_errors"],
        Role::Planner => &[
            "forecast",
            "execution",
            "violations",
            "decision_context",
            "evidence_errors",
            "forcing_rehearsals",
        ],
        _ => &[],
    };
    if let Some(variants) = schema.get_mut("oneOf").and_then(Value::as_array_mut) {
        for variant in variants {
            remove_fields(variant, owned, false);
        }
    } else {
        remove_fields(&mut schema, owned, false);
    }
    if let Some(defs) = schema.get_mut("$defs").and_then(Value::as_object_mut) {
        for field in owned {
            defs.remove(*field);
        }
    }
    let model_schema = bundle(schema)?;
    Ok(Rendered {
        prompt_suffix: suffix,
        model_schema,
        guidance: guidance::ONLY_JSON,
    })
}

fn ingest_inventory() -> Result<String, String> {
    let schema = roles::ingest_reply::schema_value();
    let fields = |name: &str| -> Result<String, String> {
        Ok(schema["$defs"][name]["properties"]
            .as_object()
            .ok_or("missing ingest field inventory")?
            .keys()
            .cloned()
            .collect::<Vec<_>>()
            .join(", "))
    };
    Ok(format!(
        "Ingest reply contract, generated from ingest-reply.json. A proof_obligation has only these fields: {}. An evidence_contract entry has only these fields: {}.",
        fields("proof_obligation")?,
        fields("evidence_requirement")?
    ))
}
fn remove_fields(schema: &mut Value, fields: &[&str], required: bool) {
    if let Some(properties) = schema.get_mut("properties").and_then(Value::as_object_mut) {
        for field in fields {
            properties.remove(*field);
        }
    }
    if required && let Some(required) = schema.get_mut("required").and_then(Value::as_array_mut) {
        required.retain(|v| !fields.iter().any(|name| v == name));
    }
}
fn maker_proposal(schema: &mut Value) -> Result<(), String> {
    for variant in schema["oneOf"]
        .as_array_mut()
        .ok_or("maker variants missing")?
    {
        remove_fields(variant, &["budget_exhaustion"], false);
        match variant["properties"]["kind"]["const"].as_str() {
            Some("done") => remove_fields(
                variant,
                &[
                    "head",
                    "test_evidence",
                    "generation_evidence",
                    "checking_handoff",
                ],
                true,
            ),
            Some("needs_files") => remove_fields(variant, &["fits"], false),
            Some("blocked") => {
                for cited in variant["properties"]["evidence"]["oneOf"]
                    .as_array_mut()
                    .ok_or("maker evidence variants missing")?
                {
                    remove_fields(cited, &["tool", "class"], true);
                }
            }
            _ => (),
        }
    }
    Ok(())
}

fn schema(file: &str) -> Result<Value, String> {
    match file {
        "ingest-reply.json" => Ok(roles::ingest_reply::schema_value()),
        "planner-reply.json" => Ok(roles::planner_reply::schema_value()),
        "reviewer-input.json" => Ok(roles::reviewer_input::schema()),
        _ => Err(format!("unknown model schema dependency {file}")),
    }
}

/// Resolve model-visible external definitions and prune unreachable definitions.
fn bundle(mut root: Value) -> Result<Value, String> {
    use std::collections::{BTreeMap, BTreeSet};
    let mut defs = root
        .as_object_mut()
        .and_then(|object| object.remove("$defs"))
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default();
    let mut names = BTreeMap::new();
    fn visit(
        value: &mut Value,
        defs: &mut serde_json::Map<String, Value>,
        names: &mut BTreeMap<String, String>,
    ) -> Result<(), String> {
        match value {
            Value::Object(object) => {
                if let Some(reference) = object.get_mut("$ref")
                    && let Some(text) = reference.as_str()
                    && let Some((file, pointer)) = text.split_once('#')
                    && !file.is_empty()
                    && pointer.starts_with("/$defs/")
                {
                    let key = text.to_owned();
                    let name = if let Some(name) = names.get(&key) {
                        name.clone()
                    } else {
                        let base = pointer.trim_start_matches("/$defs/");
                        let name = if defs.contains_key(base) {
                            format!(
                                "{}_{}",
                                file.trim_end_matches(".json").replace('-', "_"),
                                base
                            )
                        } else {
                            base.into()
                        };
                        names.insert(key.clone(), name.clone());
                        let mut copied = schema(file)?
                            .pointer(pointer)
                            .cloned()
                            .ok_or("external definition missing")?;
                        qualify(&mut copied, file);
                        visit(&mut copied, defs, names)?;
                        defs.insert(name.clone(), copied);
                        name
                    };
                    *reference = json!(format!("#/$defs/{name}"));
                }
                for child in object.values_mut() {
                    visit(child, defs, names)?;
                }
            }
            Value::Array(items) => {
                for item in items {
                    visit(item, defs, names)?;
                }
            }
            _ => (),
        }
        Ok(())
    }
    fn qualify(value: &mut Value, file: &str) {
        match value {
            Value::Object(object) => {
                if let Some(reference) = object.get_mut("$ref")
                    && let Some(text) = reference.as_str()
                    && text.starts_with('#')
                {
                    *reference = json!(format!("{file}{text}"));
                }
                for child in object.values_mut() {
                    qualify(child, file);
                }
            }
            Value::Array(items) => {
                for item in items {
                    qualify(item, file);
                }
            }
            _ => (),
        }
    }
    // Process the existing definitions too, since their refs may be external.
    for name in defs.keys().cloned().collect::<Vec<_>>() {
        let mut value = defs.remove(&name).unwrap();
        visit(&mut value, &mut defs, &mut names)?;
        defs.insert(name, value);
    }
    visit(&mut root, &mut defs, &mut names)?;
    fn references(value: &Value, used: &mut BTreeSet<String>) {
        match value {
            Value::Object(object) => {
                if let Some(reference) = object
                    .get("$ref")
                    .and_then(Value::as_str)
                    .and_then(|s| s.strip_prefix("#/$defs/"))
                {
                    used.insert(reference.into());
                }
                for child in object.values() {
                    references(child, used);
                }
            }
            Value::Array(items) => {
                for item in items {
                    references(item, used);
                }
            }
            _ => (),
        }
    }
    let mut used = BTreeSet::new();
    references(&root, &mut used);
    loop {
        let before = used.len();
        for name in used.clone() {
            if let Some(value) = defs.get(&name) {
                references(value, &mut used);
            }
        }
        if used.len() == before {
            break;
        }
    }
    defs.retain(|key, _| used.contains(key));
    if !defs.is_empty() {
        root["$defs"] = Value::Object(defs);
    }
    Ok(root)
}
