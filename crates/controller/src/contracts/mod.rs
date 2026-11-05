//! Wire contracts that cross the Python/Rust boundary, defined once in Rust (#730 wave A,
//! #741). Serde reads and writes them; schemars renders their JSON Schemas, which are
//! committed and checked for drift. Python validates against the committed files.
//!
//! This module depends only on std, serde, serde_json and schemars, never on the rest of
//! the controller, so it can become its own crate unchanged.

pub mod bounded;
pub mod checks;
pub mod report;

pub use checks::{Check, CheckKind, CheckRequest, DeclaredSymptom, Forcing, ProofDeclaration};
pub use report::{CheckReport, CheckResult, MESSAGE_CHARS, ProofReceipt, TestResult};

use schemars::generate::SchemaSettings;
use serde_json::Value;

/// Where the worker schemas live, relative to the repository root.
pub const SCHEMA_DIRECTORY: &str = "workers/src/tl3_workers/schemas";

/// The `$id` prefix every worker schema shares, so cross-file `$ref`s resolve.
pub const SCHEMA_BASE: &str = "https://trustladderiii.invalid/workers/protocol-1/";

/// One generated file: its path relative to the repository root, and its text.
pub struct Generated {
    pub path: String,
    pub text: String,
}

/// Every file this module generates.
pub fn generated() -> Vec<Generated> {
    vec![Generated {
        path: format!("{SCHEMA_DIRECTORY}/check-report.json"),
        text: render(&check_report_schema()),
    }]
}

/// The harness check report's schema: self-contained, with every definition in `$defs`.
pub fn check_report_schema() -> Value {
    let mut schema = SchemaSettings::draft2020_12()
        .into_generator()
        .into_root_schema_for::<CheckReport>()
        .to_value();
    let object = schema
        .as_object_mut()
        .expect("a struct's schema is an object");
    object.remove("title");
    object.insert(
        "$id".into(),
        Value::String(format!("{SCHEMA_BASE}check-report.json")),
    );
    strip_integer_formats(&mut schema);
    schema
}

// schemars annotates Rust integer widths ("format": "uint64"). They are not JSON Schema
// formats, and the hand-written worker schemas never carried them.
fn strip_integer_formats(value: &mut Value) {
    match value {
        Value::Object(object) => {
            if object
                .get("format")
                .and_then(Value::as_str)
                .is_some_and(|format| format.starts_with("int") || format.starts_with("uint"))
            {
                object.remove("format");
            }
            object.values_mut().for_each(strip_integer_formats);
        }
        Value::Array(items) => items.iter_mut().for_each(strip_integer_formats),
        _ => {}
    }
}

// Two-space indentation and a final newline, as the hand-written schema files use.
fn render(schema: &Value) -> String {
    let mut text = serde_json::to_string_pretty(schema).expect("a schema serializes");
    text.push('\n');
    text
}

pub mod render;
