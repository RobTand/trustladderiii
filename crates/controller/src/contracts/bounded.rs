// Bounded wire values. Each bound lives in exactly one type: `Deserialize` enforces it and
// `JsonSchema` states it, so serde and the generated schema cannot disagree (#741).
// Lengths count Unicode scalar values, as JSON Schema's minLength and maxLength do.

use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error as _};
use std::{borrow::Cow, collections::BTreeMap, marker::PhantomData, ops::Deref};

/// No upper bound on a length or a count.
pub const UNBOUNDED: usize = usize::MAX;

fn length_bounds(schema: &mut Schema, min_key: &str, max_key: &str, min: usize, max: usize) {
    if min > 0 {
        schema.insert(min_key.into(), min.into());
    }
    if max != UNBOUNDED {
        schema.insert(max_key.into(), max.into());
    }
}

fn length_error(min: usize, max: usize, length: usize, unit: &str) -> String {
    match (min, max) {
        (_, UNBOUNDED) => format!("expected at least {min} {unit}, found {length}"),
        (0, _) => format!("expected at most {max} {unit}, found {length}"),
        _ => format!("expected {min} to {max} {unit}, found {length}"),
    }
}

/// Text with `MIN..=MAX` characters.
#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct Text<const MIN: usize, const MAX: usize>(String);

impl<const MIN: usize, const MAX: usize> Text<MIN, MAX> {
    pub fn new(text: impl Into<String>) -> Result<Self, String> {
        let text = text.into();
        let length = text.chars().count();
        if length < MIN || length > MAX {
            return Err(length_error(MIN, MAX, length, "characters"));
        }
        Ok(Self(text))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de, const MIN: usize, const MAX: usize> Deserialize<'de> for Text<MIN, MAX> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self::new(String::deserialize(deserializer)?).map_err(D::Error::custom)
    }
}

impl<const MIN: usize, const MAX: usize> JsonSchema for Text<MIN, MAX> {
    fn inline_schema() -> bool {
        true
    }

    fn schema_name() -> Cow<'static, str> {
        format!("Text_{MIN}_{MAX}").into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        let mut schema = json_schema!({"type": "string"});
        length_bounds(&mut schema, "minLength", "maxLength", MIN, MAX);
        schema
    }
}

/// Text cut to its first `MAX` characters when read. The controller has always clipped
/// harness messages after parsing them; clipping while parsing keeps that behaviour and
/// lets the type, not a later pass, carry the bound.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct ClippedText<const MAX: usize>(String);

impl<const MAX: usize> ClippedText<MAX> {
    pub fn clipped(text: &str) -> Self {
        Self(text.chars().take(MAX).collect())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de, const MAX: usize> Deserialize<'de> for ClippedText<MAX> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(Self::clipped(&String::deserialize(deserializer)?))
    }
}

impl<const MAX: usize> JsonSchema for ClippedText<MAX> {
    fn inline_schema() -> bool {
        true
    }

    fn schema_name() -> Cow<'static, str> {
        format!("ClippedText_{MAX}").into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({"type": "string", "maxLength": MAX})
    }
}

fn lower_hex(text: &str) -> bool {
    text.bytes()
        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

macro_rules! patterned_text {
    ($(#[$meta:meta])* $name:ident, $pattern:expr, $accepts:expr) => {
        $(#[$meta])*
        #[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            /// The JSON Schema pattern that states this type's rule.
            pub const PATTERN: &'static str = $pattern;

            pub fn new(text: impl Into<String>) -> Result<Self, String> {
                let text = text.into();
                let accepts: fn(&str) -> bool = $accepts;
                if accepts(&text) {
                    Ok(Self(text))
                } else {
                    Err(format!(
                        "{:?} does not match {}",
                        text.chars().take(80).collect::<String>(),
                        Self::PATTERN
                    ))
                }
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                Self::new(String::deserialize(deserializer)?).map_err(D::Error::custom)
            }
        }
    };
}

patterned_text!(
    /// A lowercase SHA-256 digest.
    Sha256Hex,
    "^[0-9a-f]{64}$",
    |text| text.len() == 64 && lower_hex(text)
);

patterned_text!(
    /// A full lowercase Git object name, SHA-1 or SHA-256.
    CommitSha,
    "^(?:[0-9a-f]{40}|[0-9a-f]{64})$",
    |text| matches!(text.len(), 40 | 64) && lower_hex(text)
);

patterned_text!(
    /// A proof subject or observer support unit: a relative `.py` path, optionally
    /// followed by `::name` parts, that never names a parent directory.
    UnitPath,
    r"^(?!/)(?!.*(?:^|/)\.\.(?:/|::|$))[^\\\u0000\r\n]+\.py(?:::[A-Za-z_][A-Za-z_0-9]*)*$",
    unit_path
);

patterned_text!(
    /// A dotted Python name such as `builtins.TimeoutError`.
    ExceptionName,
    r"^[A-Za-z_]\w*(\.[A-Za-z_]\w*)+$",
    |text| dotted(text, |c| c == '_' || c.is_alphanumeric())
);

patterned_text!(
    /// The dotted attribute a forcing patch replaces, at most 200 characters.
    PatchTarget,
    r"^[A-Za-z_][A-Za-z0-9_]*(\.[A-Za-z_][A-Za-z0-9_]*)+$",
    |text| text.chars().count() <= 200 && dotted(text, |c| c == '_' || c.is_ascii_alphanumeric())
);

patterned_text!(
    /// An environment variable a forcing sets.
    EnvironmentName,
    "^[A-Z_][A-Z0-9_]{0,63}$",
    |text| {
        let mut chars = text.chars();
        chars
            .next()
            .is_some_and(|c| c == '_' || c.is_ascii_uppercase())
            && text.len() <= 64
            && chars.all(|c| c == '_' || c.is_ascii_uppercase() || c.is_ascii_digit())
    }
);

fn ascii_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c == '_' || c.is_ascii_alphabetic())
        && chars.all(|c| c == '_' || c.is_ascii_alphanumeric())
}

// At least two dot-separated parts; each starts with an ASCII letter or `_`, then `word`.
fn dotted(text: &str, word: fn(char) -> bool) -> bool {
    let parts: Vec<_> = text.split('.').collect();
    parts.len() >= 2
        && parts.iter().all(|part| {
            let mut chars = part.chars();
            chars
                .next()
                .is_some_and(|c| c == '_' || c.is_ascii_alphabetic())
                && chars.all(word)
        })
}

// UnitPath::PATTERN without lookaheads: no leading `/`, no `..` segment ending at `/`,
// `::` or the end, no backslash, NUL, CR or LF, and some `X.py` prefix (X non-empty)
// followed only by `::identifier` parts.
fn unit_path(text: &str) -> bool {
    let length = text.chars().count();
    let parent = text.match_indices("..").any(|(index, _)| {
        let rest = &text[index + 2..];
        (index == 0 || text[..index].ends_with('/'))
            && (rest.is_empty() || rest.starts_with('/') || rest.starts_with("::"))
    });
    (1..=1000).contains(&length)
        && !text.starts_with('/')
        && !parent
        && !text.contains(['\\', '\0', '\r', '\n'])
        && text
            .match_indices(".py")
            .any(|(index, _)| index > 0 && names(&text[index + 3..]))
}

fn names(mut rest: &str) -> bool {
    while !rest.is_empty() {
        let Some(name) = rest.strip_prefix("::") else {
            return false;
        };
        let end = name.find("::").unwrap_or(name.len());
        if !ascii_identifier(&name[..end]) {
            return false;
        }
        rest = &name[end..];
    }
    true
}

macro_rules! pattern_schema {
    ($name:ident, $extra:expr) => {
        impl JsonSchema for $name {
            fn inline_schema() -> bool {
                true
            }

            fn schema_name() -> Cow<'static, str> {
                stringify!($name).into()
            }

            fn json_schema(_: &mut SchemaGenerator) -> Schema {
                let mut schema = json_schema!({"type": "string"});
                let extra: fn(&mut Schema) = $extra;
                extra(&mut schema);
                schema.insert("pattern".into(), Self::PATTERN.into());
                schema
            }
        }
    };
}

fn no_extra(_: &mut Schema) {}

pattern_schema!(Sha256Hex, no_extra);
pattern_schema!(CommitSha, no_extra);
pattern_schema!(ExceptionName, no_extra);
pattern_schema!(EnvironmentName, no_extra);
pattern_schema!(UnitPath, |schema: &mut Schema| {
    schema.insert("minLength".into(), 1.into());
    schema.insert("maxLength".into(), 1000.into());
});
pattern_schema!(PatchTarget, |schema: &mut Schema| {
    schema.insert("maxLength".into(), 200.into());
});

/// A list of `MIN..=MAX` items, all distinct when `UNIQUE`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct List<T, const MIN: usize, const MAX: usize, const UNIQUE: bool = false>(Vec<T>);

impl<T: PartialEq, const MIN: usize, const MAX: usize, const UNIQUE: bool>
    List<T, MIN, MAX, UNIQUE>
{
    pub fn new(items: Vec<T>) -> Result<Self, String> {
        if items.len() < MIN || items.len() > MAX {
            return Err(length_error(MIN, MAX, items.len(), "items"));
        }
        if UNIQUE
            && items
                .iter()
                .enumerate()
                .any(|(index, item)| items[..index].contains(item))
        {
            return Err("expected distinct items".into());
        }
        Ok(Self(items))
    }
}

impl<T, const MIN: usize, const MAX: usize, const UNIQUE: bool> Deref
    for List<T, MIN, MAX, UNIQUE>
{
    type Target = [T];

    fn deref(&self) -> &[T] {
        &self.0
    }
}

impl<'de, T, const MIN: usize, const MAX: usize, const UNIQUE: bool> Deserialize<'de>
    for List<T, MIN, MAX, UNIQUE>
where
    T: Deserialize<'de> + PartialEq,
{
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self::new(Vec::deserialize(deserializer)?).map_err(D::Error::custom)
    }
}

impl<T: JsonSchema, const MIN: usize, const MAX: usize, const UNIQUE: bool> JsonSchema
    for List<T, MIN, MAX, UNIQUE>
{
    fn inline_schema() -> bool {
        true
    }

    fn schema_name() -> Cow<'static, str> {
        format!("List_{}_{MIN}_{MAX}_{UNIQUE}", T::schema_name()).into()
    }

    fn json_schema(generator: &mut SchemaGenerator) -> Schema {
        let mut schema = json_schema!({"type": "array", "items": generator.subschema_for::<T>()});
        length_bounds(&mut schema, "minItems", "maxItems", MIN, MAX);
        if UNIQUE {
            schema.insert("uniqueItems".into(), true.into());
        }
        schema
    }
}

/// An integer in `MIN..=MAX`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct Integer<const MIN: u64, const MAX: u64>(u64);

impl<const MIN: u64, const MAX: u64> Integer<MIN, MAX> {
    pub fn new(value: u64) -> Result<Self, String> {
        if (MIN..=MAX).contains(&value) {
            Ok(Self(value))
        } else {
            Err(format!(
                "expected an integer from {MIN} to {MAX}, found {value}"
            ))
        }
    }

    pub fn get(&self) -> u64 {
        self.0
    }

    /// Red (#750 review): a stub until integral JSON numbers are kept as written.
    pub fn from_number(_number: serde_json::Number) -> Result<Self, String> {
        todo!("accept schema-valid integral numbers")
    }
}

impl<'de, const MIN: u64, const MAX: u64> Deserialize<'de> for Integer<MIN, MAX> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self::new(u64::deserialize(deserializer)?).map_err(D::Error::custom)
    }
}

impl<const MIN: u64, const MAX: u64> JsonSchema for Integer<MIN, MAX> {
    fn inline_schema() -> bool {
        true
    }

    fn schema_name() -> Cow<'static, str> {
        format!("Integer_{MIN}_{MAX}").into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        let mut schema = json_schema!({"type": "integer", "minimum": MIN});
        if MAX != u64::MAX {
            schema.insert("maximum".into(), MAX.into());
        }
        schema
    }
}

/// A non-negative count with no upper bound.
pub type Count = Integer<0, { u64::MAX }>;

/// A declared symptom campaign's xdist worker count: 0 (serial), 2, 3 or 4.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct Parallel(u64);

impl Parallel {
    pub const VALUES: [u64; 4] = [0, 2, 3, 4];

    pub fn get(&self) -> u64 {
        self.0
    }

    /// Red (#750 review): a stub until parallel has a constructor.
    pub fn from_number(_number: serde_json::Number) -> Result<Self, String> {
        todo!("construct a parallel count")
    }
}

impl<'de> Deserialize<'de> for Parallel {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = u64::deserialize(deserializer)?;
        if Self::VALUES.contains(&value) {
            Ok(Self(value))
        } else {
            Err(D::Error::custom(format!(
                "parallel must be 0, 2, 3 or 4, not {value}"
            )))
        }
    }
}

impl JsonSchema for Parallel {
    fn inline_schema() -> bool {
        true
    }

    fn schema_name() -> Cow<'static, str> {
        "Parallel".into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({"enum": Self::VALUES})
    }
}

// A JSON number kept as written, so an echoed value compares and re-serializes exactly
// as the harness wrote it (`1` stays `1`, `0.01` stays `0.01`). Bounds use its f64 value.
macro_rules! bounded_number {
    ($(#[$meta:meta])* $name:ident, $accepts:expr, $schema:tt) => {
        $(#[$meta])*
        #[derive(Clone, Debug, PartialEq, Eq, Serialize)]
        #[serde(transparent)]
        pub struct $name(serde_json::Number);

        impl $name {
            pub fn new(number: serde_json::Number) -> Result<Self, String> {
                let accepts: fn(f64) -> bool = $accepts;
                if number.as_f64().is_some_and(accepts) {
                    Ok(Self(number))
                } else {
                    Err(format!("{number} is out of range for {}", stringify!($name)))
                }
            }

            pub fn number(&self) -> &serde_json::Number {
                &self.0
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                Self::new(serde_json::Number::deserialize(deserializer)?).map_err(D::Error::custom)
            }
        }

        impl JsonSchema for $name {
            fn inline_schema() -> bool {
                true
            }

            fn schema_name() -> Cow<'static, str> {
                stringify!($name).into()
            }

            fn json_schema(_: &mut SchemaGenerator) -> Schema {
                json_schema!($schema)
            }
        }
    };
}

bounded_number!(
    /// A forcing patch's delay: more than 0 and at most 5 seconds.
    DelaySeconds,
    |seconds| seconds > 0.0 && seconds <= 5.0,
    {"type": "number", "exclusiveMinimum": 0, "maximum": 5}
);

bounded_number!(
    /// Delay a test actually received, within the harness's 10-second total.
    AppliedSeconds,
    |seconds| (0.0..=10.0).contains(&seconds),
    {"type": "number", "minimum": 0, "maximum": 10}
);

macro_rules! constant {
    ($(#[$meta:meta])* $name:ident, $value:expr) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
        pub struct $name;

        impl Serialize for $name {
            fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serde_json::json!($value).serialize(serializer)
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                let value = serde_json::Value::deserialize(deserializer)?;
                if value == serde_json::json!($value) {
                    Ok(Self)
                } else {
                    Err(D::Error::custom(format!("expected {}", serde_json::json!($value))))
                }
            }
        }

        impl JsonSchema for $name {
            fn inline_schema() -> bool {
                true
            }

            fn schema_name() -> Cow<'static, str> {
                stringify!($name).into()
            }

            fn json_schema(_: &mut SchemaGenerator) -> Schema {
                json_schema!({"const": $value})
            }
        }
    };
}

constant!(
    /// The literal `true`, for flags that are present only when set.
    True,
    true
);

constant!(
    /// The runner's record protocol version, `1`.
    RecordSchemaVersion,
    1
);

/// Schema-only marker for a field that must be present and may be `null`.
/// Pair it with `#[serde(deserialize_with = "required_nullable")]` on an `Option<T>`.
pub struct Nullable<T>(PhantomData<T>);

impl<T: JsonSchema> JsonSchema for Nullable<T> {
    fn inline_schema() -> bool {
        true
    }

    fn schema_name() -> Cow<'static, str> {
        format!("Nullable_{}", T::schema_name()).into()
    }

    fn json_schema(generator: &mut SchemaGenerator) -> Schema {
        json_schema!({"anyOf": [generator.subschema_for::<T>(), {"type": "null"}]})
    }
}

/// `Option<T>` that must be present: `null` is `None`, a missing key is an error.
pub fn required_nullable<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer)
}

/// `Option<T>` that may be missing but is never `null`. Pair it with `#[serde(default)]`.
pub fn present<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    T::deserialize(deserializer).map(Some)
}

/// String-keyed object with at most `MAX` entries.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct Object<K: Ord, V, const MAX: usize>(BTreeMap<K, V>);

impl<K: Ord, V, const MAX: usize> Object<K, V, MAX> {
    pub fn new(entries: BTreeMap<K, V>) -> Result<Self, String> {
        if entries.len() > MAX {
            return Err(length_error(0, MAX, entries.len(), "entries"));
        }
        Ok(Self(entries))
    }
}

impl<K: Ord, V, const MAX: usize> Deref for Object<K, V, MAX> {
    type Target = BTreeMap<K, V>;

    fn deref(&self) -> &BTreeMap<K, V> {
        &self.0
    }
}

impl<'de, K, V, const MAX: usize> Deserialize<'de> for Object<K, V, MAX>
where
    K: Deserialize<'de> + Ord,
    V: Deserialize<'de>,
{
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self::new(BTreeMap::deserialize(deserializer)?).map_err(D::Error::custom)
    }
}

impl<K: Ord + JsonSchema, V: JsonSchema, const MAX: usize> JsonSchema for Object<K, V, MAX> {
    fn inline_schema() -> bool {
        true
    }

    fn schema_name() -> Cow<'static, str> {
        format!("Object_{}_{}_{MAX}", K::schema_name(), V::schema_name()).into()
    }

    fn json_schema(generator: &mut SchemaGenerator) -> Schema {
        let mut schema = json_schema!({
            "type": "object",
            "additionalProperties": generator.subschema_for::<V>(),
        });
        let mut names = generator.subschema_for::<K>();
        if let Some(names) = names.as_object_mut() {
            // Property names are strings by definition; state only their rule.
            names.remove("type");
            if !names.is_empty() {
                schema.insert("propertyNames".into(), names.clone().into());
            }
        }
        length_bounds(&mut schema, "minProperties", "maxProperties", 0, MAX);
        schema
    }
}
