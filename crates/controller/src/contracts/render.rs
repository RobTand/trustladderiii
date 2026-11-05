//! Preserve published schema key order without changing serde_json's workspace features.

use schemars::JsonSchema;
use serde_json::Value;
use std::borrow::Cow;

/// One object's JSON pointer and exact key order, relative to its containing scope.
pub type ObjectOrder<'a> = (&'a str, &'a [&'a str]);

/// One order declaration shared by every use of a named Rust schema type.
pub struct TypeOrder<'a> {
    pub name: Cow<'static, str>,
    pub objects: &'a [ObjectOrder<'a>],
}

impl<'a> TypeOrder<'a> {
    pub fn of<T: JsonSchema>(objects: &'a [ObjectOrder<'a>]) -> Self {
        Self {
            name: T::schema_name(),
            objects,
        }
    }
}

pub struct TypeUse<'a> {
    pub pointer: &'a str,
    pub order: &'a TypeOrder<'a>,
}

/// Pointer entries describe inline objects; named types bind their shared order here.
pub struct Layout<'a> {
    pub inline: &'a [ObjectOrder<'a>],
    pub shared: &'a [TypeUse<'a>],
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ErrorKind {
    MissingOrder,
    KeysDiffer {
        expected: Vec<String>,
        actual: Vec<String>,
    },
    UnusedOrder,
    DuplicateOrder,
    ConflictingTypeOrder {
        name: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RenderError {
    pub file: String,
    pub pointer: String,
    pub kind: ErrorKind,
}

impl std::fmt::Display for RenderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}: schema pointer {:?}: {:?}",
            self.file, self.pointer, self.kind
        )
    }
}
impl std::error::Error for RenderError {}

/// Render two-space JSON and a final newline after checking every object and order entry.
pub fn render(_file: &str, _value: &Value, _layout: &Layout<'_>) -> Result<String, RenderError> {
    todo!("P1a-0 strict order rendering")
}
