//! Membership for resolved canonical rows, with the packaged Rust-owned schemas.
use serde_json::{Value, json};
use std::sync::OnceLock;
use tl3_contracts::rules::SchemaKeyword;
include!(concat!(env!("OUT_DIR"), "/schemas.rs"));

// Inspect schema positions only. Property names and const/enum values are instance data.
fn supported(schema: &Value) -> Result<(), String> {
    let Some(object) = schema.as_object() else {
        return if schema.is_boolean() {
            Ok(())
        } else {
            Err("invalid schema value".into())
        };
    };
    for (key, value) in object {
        match key.as_str() {
            "$defs" | "definitions" | "properties" | "patternProperties" | "dependentSchemas" => {
                let values = value.as_object().ok_or("schema map must be an object")?;
                for child in values.values() {
                    supported(child)?;
                }
            }
            "allOf" | "anyOf" | "oneOf" | "prefixItems" => {
                let children = value
                    .as_array()
                    .ok_or("schema alternatives must be an array")?;
                for child in children {
                    supported(child)?;
                }
            }
            "not"
            | "if"
            | "then"
            | "else"
            | "items"
            | "contains"
            | "additionalProperties"
            | "unevaluatedProperties"
            | "unevaluatedItems"
            | "propertyNames"
            | "contentSchema" => supported(value)?,
            "$schema" | "$id" | "$ref" | "$anchor" | "$comment" | "type" | "const" | "enum"
            | "title" | "description" | "default" | "examples" | "readOnly" | "writeOnly"
            | "deprecated" | "format" | "minimum" | "maximum" | "exclusiveMinimum"
            | "exclusiveMaximum" | "multipleOf" | "minLength" | "maxLength" | "pattern"
            | "minItems" | "maxItems" | "uniqueItems" | "minContains" | "maxContains"
            | "minProperties" | "maxProperties" | "required" | "dependentRequired"
            | "contentEncoding" | "contentMediaType" => (),
            _ => return Err("unsupported schema keyword in refusal constraint".into()),
        }
    }
    Ok(())
}

fn registry() -> Result<&'static jsonschema::Registry<'static>, String> {
    static REGISTRY: OnceLock<Result<jsonschema::Registry<'static>, String>> = OnceLock::new();
    REGISTRY
        .get_or_init(|| {
            let mut registry = jsonschema::Registry::new();
            for (name, text) in PACKAGED_SCHEMAS {
                let schema: Value =
                    serde_json::from_str(text).map_err(|_| "invalid packaged schema")?;
                supported(&schema)?;
                registry = registry
                    .add(format!("{}{name}", tl3_contracts::SCHEMA_BASE), schema)
                    .map_err(|_| "unavailable packaged schema reference")?;
            }
            registry
                .prepare()
                .map_err(|_| "unresolved packaged schema reference".into())
        })
        .as_ref()
        .map_err(Clone::clone)
}

pub(super) fn membership(
    keyword: SchemaKeyword,
    constraint: &Value,
    value: &Value,
) -> Result<bool, String> {
    let schema = if keyword == SchemaKeyword::Schema {
        constraint.clone()
    } else {
        let name = serde_json::to_value(keyword).map_err(|_| "invalid schema keyword")?;
        json!({name.as_str().ok_or("invalid schema keyword")?: constraint})
    };
    supported(&schema)?;
    let validator = jsonschema::options()
        .with_registry(registry()?)
        .build(&schema)
        .map_err(|_| "unsupported or unresolved refusal schema")?;
    Ok(validator.is_valid(value))
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ValidationFact {
    keyword: String,
    validator_value: Value,
    instance: Value,
    schema: Value,
    path: Vec<Value>,
    schema_path: Vec<Value>,
    context: Vec<ValidationFact>,
}

fn selected<'a>(fact: &'a ValidationFact, out: &mut Vec<&'a ValidationFact>) {
    if !matches!(fact.keyword.as_str(), "oneOf" | "anyOf") || fact.context.is_empty() {
        out.push(fact);
        return;
    }
    let Some(variants) = fact.validator_value.as_array() else {
        out.push(fact);
        return;
    };
    let discriminators = variants
        .iter()
        .map(|variant| {
            variant["properties"]
                .as_object()
                .into_iter()
                .flat_map(|properties| properties.iter())
                .filter(|(_, item)| item.get("const").is_some() || item.get("enum").is_some())
                .map(|(name, _)| name.clone())
                .collect::<std::collections::BTreeSet<_>>()
        })
        .reduce(|left, right| left.intersection(&right).cloned().collect())
        .unwrap_or_default();
    let matching: Vec<_> = variants
        .iter()
        .enumerate()
        .filter(|(_, variant)| {
            !discriminators.is_empty()
                && discriminators.iter().all(|key| {
                    let Some(value) = fact.instance.get(key) else {
                        return false;
                    };
                    let constraint = &variant["properties"][key];
                    constraint
                        .get("const")
                        .is_some_and(|expected| expected == value)
                        || constraint["enum"]
                            .as_array()
                            .is_some_and(|choices| choices.contains(value))
                })
        })
        .map(|(index, _)| index as u64)
        .collect();
    if matching.len() != 1 {
        out.push(fact);
        return;
    }
    for child in &fact.context {
        if child.schema_path.first().and_then(Value::as_u64) == Some(matching[0]) {
            selected(child, out);
        }
    }
}

pub(super) fn refusal_rows(
    errors: &[ValidationFact],
    source: Option<&Value>,
    role: tl3_contracts::rules::Role,
) -> Result<Vec<tl3_contracts::rules::Violation>, String> {
    use tl3_contracts::rules::{Allowed, EditOp, FieldPointer, Name, NameKind, Repair, Violation};
    fn escape(value: &str) -> String {
        value.replace('~', "~0").replace('/', "~1")
    }
    fn constraint(value: &Value, source: &Value) -> Value {
        let mut value = value.clone();
        if let Some(object) = value.as_object_mut() {
            if let Some(definitions) = source.get("$defs") {
                object.insert("$defs".into(), definitions.clone());
            }
            if let Some(id) = source.get("$id") {
                object.insert("$id".into(), id.clone());
            }
        }
        value
    }
    let mut rows = Vec::new();
    let mut required = std::collections::BTreeSet::new();
    for parent in errors {
        let source = source.unwrap_or(&parent.schema);
        let mut facts = Vec::new();
        selected(parent, &mut facts);
        for fact in facts {
            let mut pointer = String::new();
            for part in &fact.path {
                let name = match part {
                    Value::String(name) => name.clone(),
                    Value::Number(index) if index.is_u64() => index.to_string(),
                    _ => return Err("invalid validation fact path".into()),
                };
                pointer.push('/');
                pointer.push_str(&escape(&name));
            }
            let mut edits = Vec::new();
            if fact.keyword == "required" {
                for key in fact
                    .validator_value
                    .as_array()
                    .ok_or("invalid required fact")?
                {
                    let key = key.as_str().ok_or("invalid required name")?;
                    let field = format!("{pointer}/{}", escape(key));
                    if fact.instance.get(key).is_none() && required.insert(field.clone()) {
                        let schema = fact
                            .schema
                            .get("properties")
                            .and_then(|properties| properties.get(key))
                            .cloned()
                            .unwrap_or(json!({}));
                        edits.push((field, EditOp::Add, Value::Null, schema));
                    }
                }
            } else if fact.keyword == "additionalProperties" && fact.validator_value == false {
                for (key, value) in fact
                    .instance
                    .as_object()
                    .ok_or("invalid additional property fact")?
                {
                    if fact.schema["properties"].get(key).is_some() {
                        continue;
                    }
                    let mut matched = false;
                    for pattern in fact.schema["patternProperties"]
                        .as_object()
                        .into_iter()
                        .flat_map(|patterns| patterns.keys())
                    {
                        matched |=
                            membership(SchemaKeyword::Pattern, &json!(pattern), &json!(key))?;
                    }
                    if !matched {
                        edits.push((
                            format!("{pointer}/{}", escape(key)),
                            EditOp::Remove,
                            value.clone(),
                            json!({"enum":[]}),
                        ));
                    }
                }
            } else {
                edits.push((
                    pointer,
                    EditOp::Replace,
                    fact.instance.clone(),
                    fact.schema.clone(),
                ));
            }
            for (pointer, op, bad_value, schema) in edits {
                let allowed = Allowed::Schema {
                    keyword: SchemaKeyword::Schema,
                    value: constraint(&schema, source),
                };
                // Compiles every resolved schema before publication, including nested custom keywords/references.
                membership(
                    SchemaKeyword::Schema,
                    match &allowed {
                        Allowed::Schema { value, .. } => value,
                        _ => unreachable!(),
                    },
                    &Value::Null,
                )?;
                rows.push(Violation {
                    rule_id: format!("schema.{}", fact.keyword.to_lowercase()),
                    role,
                    names: vec![Name {
                        kind: NameKind::Field,
                        value: pointer.clone(),
                    }],
                    bad_value,
                    repair: Repair::Edit {
                        pointer: FieldPointer::try_from(pointer)?,
                        op,
                        allowed: allowed.clone(),
                    },
                    allowed,
                    gating: false,
                    evidence: vec![],
                    evidence_truncated: 0,
                });
            }
        }
    }
    Ok(rows)
}
