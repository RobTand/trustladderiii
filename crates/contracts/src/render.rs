//! Preserve published schema key order without changing serde_json's workspace features.

use schemars::JsonSchema;
use serde::{
    Serialize, Serializer,
    ser::{SerializeMap, SerializeSeq},
};
use serde_json::Value;
use std::{
    borrow::Cow,
    collections::{BTreeMap, BTreeSet},
};

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

/// Named types implement this once so every file uses the same object-order source.
pub trait SchemaOrder: JsonSchema {
    fn order() -> TypeOrder<'static>;
}

/// Render two-space JSON and a final newline after checking every object and order entry.
pub fn render(file: &str, value: &Value, layout: &Layout<'_>) -> Result<String, RenderError> {
    let error = |pointer: &str, kind| RenderError {
        file: file.into(),
        pointer: pointer.into(),
        kind,
    };
    let mut orders = BTreeMap::new();
    let mut insert = |pointer: String, keys| {
        if orders.insert(pointer.clone(), keys).is_some() {
            Err(error(&pointer, ErrorKind::DuplicateOrder))
        } else {
            Ok(())
        }
    };
    for (pointer, keys) in layout.inline {
        insert((*pointer).into(), *keys)?;
    }
    let mut named = BTreeMap::new();
    for usage in layout.shared {
        let name = usage.order.name.as_ref();
        if let Some(existing) = named.insert(name, usage.order.objects)
            && existing != usage.order.objects
        {
            return Err(error(
                usage.pointer,
                ErrorKind::ConflictingTypeOrder { name: name.into() },
            ));
        }
        for (relative, keys) in usage.order.objects {
            insert(format!("{}{relative}", usage.pointer), *keys)?;
        }
    }
    let mut used = BTreeSet::new();
    validate(file, value, "", &orders, &mut used)?;
    if let Some(pointer) = orders.keys().find(|pointer| !used.contains(*pointer)) {
        return Err(error(pointer, ErrorKind::UnusedOrder));
    }
    let mut rendered = serde_json::to_string_pretty(&OrderedNode {
        value,
        pointer: String::new(),
        orders: &orders,
    })
    .expect("a checked JSON Value serializes");
    rendered.push('\n');
    Ok(rendered)
}

fn child(pointer: &str, key: &str) -> String {
    format!("{pointer}/{}", key.replace('~', "~0").replace('/', "~1"))
}

fn validate(
    file: &str,
    value: &Value,
    pointer: &str,
    orders: &BTreeMap<String, &[&str]>,
    used: &mut BTreeSet<String>,
) -> Result<(), RenderError> {
    let error = |kind| RenderError {
        file: file.into(),
        pointer: pointer.into(),
        kind,
    };
    match value {
        Value::Object(fields) => {
            if let Some(keys) = orders.get(pointer) {
                used.insert(pointer.into());
                let mut expected = keys.to_vec();
                expected.sort_unstable();
                let mut actual: Vec<_> = fields.keys().map(String::as_str).collect();
                actual.sort_unstable();
                if expected != actual {
                    return Err(error(ErrorKind::KeysDiffer {
                        expected: keys.iter().map(|key| (*key).into()).collect(),
                        actual: actual.into_iter().map(str::to_owned).collect(),
                    }));
                }
            } else if fields.len() >= 2 {
                return Err(error(ErrorKind::MissingOrder));
            }
            for (key, value) in fields {
                validate(file, value, &child(pointer, key), orders, used)?;
            }
        }
        Value::Array(values) => {
            for (index, value) in values.iter().enumerate() {
                validate(file, value, &format!("{pointer}/{index}"), orders, used)?;
            }
        }
        _ => {}
    }
    Ok(())
}

struct OrderedNode<'a> {
    value: &'a Value,
    pointer: String,
    orders: &'a BTreeMap<String, &'a [&'a str]>,
}

impl Serialize for OrderedNode<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self.value {
            Value::Object(fields) => {
                let mut map = serializer.serialize_map(Some(fields.len()))?;
                // Validation permits implicit order only for zero or one key.
                let implicit: Vec<_> = fields.keys().map(String::as_str).collect();
                let keys = self.orders.get(&self.pointer).copied().unwrap_or(&implicit);
                for key in keys {
                    map.serialize_entry(
                        key,
                        &OrderedNode {
                            value: &fields[*key],
                            pointer: child(&self.pointer, key),
                            orders: self.orders,
                        },
                    )?;
                }
                map.end()
            }
            Value::Array(values) => {
                let mut seq = serializer.serialize_seq(Some(values.len()))?;
                for (index, value) in values.iter().enumerate() {
                    seq.serialize_element(&OrderedNode {
                        value,
                        pointer: format!("{}/{index}", self.pointer),
                        orders: self.orders,
                    })?;
                }
                seq.end()
            }
            value => value.serialize(serializer),
        }
    }
}

// Bound types share one order implementation across all schema files. Bounds and their
// values remain in the bounded types; this table supplies keyword order only.
impl<const MIN: usize, const MAX: usize> SchemaOrder for super::bounded::Text<MIN, MAX> {
    fn order() -> TypeOrder<'static> {
        use super::bounded::UNBOUNDED;
        let objects: &'static [ObjectOrder<'static>] = match (MIN > 0, MAX != UNBOUNDED) {
            (false, false) => &[("", &["type"])],
            (true, false) => &[("", &["type", "minLength"])],
            (false, true) => &[("", &["type", "maxLength"])],
            (true, true) => &[("", &["type", "minLength", "maxLength"])],
        };
        TypeOrder::of::<Self>(objects)
    }
}

impl<const MIN: u64> SchemaOrder for super::bounded::Natural<MIN> {
    fn order() -> TypeOrder<'static> {
        TypeOrder::of::<Self>(&[("", &["type", "minimum"])])
    }
}
