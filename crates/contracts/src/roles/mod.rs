//! Typed worker role contracts.

pub mod postmortem_input;

// P1b: shared reply contracts.
pub mod classifier_reply;
pub mod ingest_reply;
pub mod maker_reply;
pub mod writer_reply;

pub mod reviewer_input;

// Schema-only references keep the published cross-file boundary while fields
// deserialize into the shared Rust types. They contain no validation rules.
macro_rules! schema_ref {
    ($name:ident, $reference:literal) => {
        pub struct $name;
        impl schemars::JsonSchema for $name {
            fn inline_schema() -> bool { true }
            fn schema_name() -> std::borrow::Cow<'static, str> { stringify!($name).into() }
            fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
                schemars::json_schema!({"$ref": $reference})
            }
        }
    };
}

fn input_schema<T: schemars::JsonSchema>(file: &str) -> serde_json::Value {
    let mut generator = schemars::generate::SchemaSettings::draft2020_12()
        .with(|settings| settings.inline_subschemas = true)
        .into_generator();
    let mut schema = T::json_schema(&mut generator).to_value();
    schema.as_object_mut().unwrap().remove("description");
    schema["$schema"] = "https://json-schema.org/draft/2020-12/schema".into();
    schema["$id"] = format!("{}{file}", crate::SCHEMA_BASE).into();
    schema
}

pub mod audit_input;
pub mod classifier_input;
pub mod ingest_input;
pub mod maker_input;
pub mod planner_input;
pub mod writer_input;

pub mod ingest_retention;
pub mod audit_reply;
pub mod planner_reply;
pub mod reviewer_reply;
pub mod postmortem_reply;

// The planner publishes the same declarations the harness parses. Only schema
// annotation spelling differs; the runtime boundary types remain shared.
fn inline_declaration_schema<T: schemars::JsonSchema>() -> schemars::Schema {
    let mut generator = schemars::generate::SchemaSettings::draft2020_12()
        .with(|settings| settings.inline_subschemas = true)
        .into_generator();
    let mut value = T::json_schema(&mut generator).to_value();
    fn strip(value: &mut serde_json::Value) {
        match value {
            serde_json::Value::Object(object) => {
                for key in ["title", "format", "default"] { object.remove(key); }
                if object.contains_key("enum") { object.remove("type"); }
                for value in object.values_mut() { strip(value); }
            }
            serde_json::Value::Array(items) => items.iter_mut().for_each(strip),
            _ => (),
        }
    }
    strip(&mut value);
    if let Some(subjects) = value.pointer_mut("/properties/subjects") { subjects["minItems"] = 0.into(); }
    value.try_into().expect("a generated schema is an object")
}
