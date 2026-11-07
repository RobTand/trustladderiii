//! Membership for resolved canonical rows, with the packaged Rust-owned schemas.
use serde_json::{Value, json};
use std::sync::OnceLock;
use tl3_contracts::rules::SchemaKeyword;
include!(concat!(env!("OUT_DIR"), "/schemas.rs"));

// Inspect schema positions only. Property names and const/enum values are instance data.
fn supported(schema: &Value) -> Result<(), String> {
    let Some(object) = schema.as_object() else {
        return if schema.is_boolean() { Ok(()) } else { Err("invalid schema value".into()) };
    };
    for (key, value) in object {
        match key.as_str() {
            "$defs" | "definitions" | "properties" | "patternProperties" | "dependentSchemas" => {
                let values = value.as_object().ok_or("schema map must be an object")?;
                for child in values.values() { supported(child)?; }
            },
            "allOf" | "anyOf" | "oneOf" | "prefixItems" => {
                let children = value.as_array().ok_or("schema alternatives must be an array")?;
                for child in children { supported(child)?; }
            },
            "not" | "if" | "then" | "else" | "items" | "contains" | "additionalProperties"
                | "unevaluatedProperties" | "unevaluatedItems" | "propertyNames" | "contentSchema" => supported(value)?,
            "$schema" | "$id" | "$ref" | "$anchor" | "$comment"
                | "type" | "const" | "enum" | "title" | "description" | "default" | "examples"
                | "readOnly" | "writeOnly" | "deprecated" | "format"
                | "minimum" | "maximum" | "exclusiveMinimum" | "exclusiveMaximum" | "multipleOf"
                | "minLength" | "maxLength" | "pattern" | "minItems" | "maxItems" | "uniqueItems"
                | "minContains" | "maxContains" | "minProperties" | "maxProperties" | "required"
                | "dependentRequired" | "contentEncoding" | "contentMediaType" => (),
            _ => return Err("unsupported schema keyword in refusal constraint".into()),
        }
    }
    Ok(())
}

fn registry() -> Result<&'static jsonschema::Registry, String> {
    static REGISTRY: OnceLock<Result<jsonschema::Registry, String>> = OnceLock::new();
    REGISTRY.get_or_init(|| {
        let mut registry = jsonschema::Registry::new();
        for (name, text) in PACKAGED_SCHEMAS {
            let schema: Value = serde_json::from_str(text).map_err(|_| "invalid packaged schema")?;
            supported(&schema)?;
            registry = registry.add(format!("{}{name}", tl3_contracts::SCHEMA_BASE), schema)
                .map_err(|_| "unavailable packaged schema reference")?;
        }
        registry.prepare().map_err(|_| "unresolved packaged schema reference".into())
    }).as_ref().map_err(Clone::clone)
}

pub(super) fn membership(keyword: SchemaKeyword, constraint: &Value, value: &Value) -> Result<bool, String> {
    let schema = if keyword == SchemaKeyword::Schema { constraint.clone() } else {
        let name = serde_json::to_value(keyword).map_err(|_| "invalid schema keyword")?;
        json!({name.as_str().ok_or("invalid schema keyword")?: constraint})
    };
    supported(&schema)?;
    let validator = jsonschema::options().with_registry(registry()?).build(&schema)
        .map_err(|_| "unsupported or unresolved refusal schema")?;
    Ok(validator.is_valid(value))
}
