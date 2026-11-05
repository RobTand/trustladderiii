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

/// Text with `MIN..=MAX` characters. It has no `Default`: an empty text is invalid when
/// `MIN > 0` (#750 review).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
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

// The integer value of a JSON number, if it has one. JSON Schema's `integer` accepts any
// number with a zero fractional part, `3.0` included (#750 review finding 3).
fn integral(number: &serde_json::Number) -> Option<i128> {
    if let Some(value) = number.as_u64() {
        return Some(value.into());
    }
    if let Some(value) = number.as_i64() {
        return Some(value.into());
    }
    let value = number.as_f64()?;
    // Every integral f64 in this range converts exactly. 2^64 itself is past u64::MAX.
    (value.fract() == 0.0
        && (-9_223_372_036_854_775_808.0..18_446_744_073_709_551_616.0).contains(&value))
    .then_some(value as i128)
}

/// An integer in `MIN..=MAX`. The JSON number is kept as written (`3` or `3.0`), so a
/// receipt that hashes it re-serializes byte for byte.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Integer<const MIN: u64, const MAX: u64> {
    value: u64,
    number: serde_json::Number,
}

impl<const MIN: u64, const MAX: u64> Integer<MIN, MAX> {
    pub fn new(value: u64) -> Result<Self, String> {
        Self::from_number(value.into())
    }

    pub fn from_number(number: serde_json::Number) -> Result<Self, String> {
        match integral(&number) {
            Some(value) if (i128::from(MIN)..=i128::from(MAX)).contains(&value) => Ok(Self {
                value: value as u64,
                number,
            }),
            _ => Err(format!(
                "expected an integer from {MIN} to {MAX}, found {number}"
            )),
        }
    }

    pub fn get(&self) -> u64 {
        self.value
    }
}

impl<const MIN: u64, const MAX: u64> Serialize for Integer<MIN, MAX> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.number.serialize(serializer)
    }
}

impl<'de, const MIN: u64, const MAX: u64> Deserialize<'de> for Integer<MIN, MAX> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self::from_number(serde_json::Number::deserialize(deserializer)?).map_err(D::Error::custom)
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
        json_schema!({"type": "integer", "minimum": MIN, "maximum": MAX})
    }
}

/// A non-negative count, up to the largest u64.
pub type Count = Integer<0, { u64::MAX }>;

/// A signed 64-bit integer, such as a process exit code, kept as written.
///
/// serde_json reads an integer literal outside i64/u64 as the nearest f64, and below
/// i64::MIN that f64 is -2^63 itself, which is in range. Python keeps the literal exact and
/// refuses it. So a value read from a number is checked again against its raw JSON token
/// (`check_token`) wherever the token is still available; `SymptomRecord`'s own
/// `Deserialize` does this for its exit code (#750 rounds 3 and 4).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Signed {
    value: i64,
    number: serde_json::Number,
}

impl Signed {
    pub fn new(value: i64) -> Self {
        Self {
            value,
            number: value.into(),
        }
    }

    pub fn from_number(number: serde_json::Number) -> Result<Self, String> {
        match integral(&number).and_then(|value| i64::try_from(value).ok()) {
            Some(value) => Ok(Self { value, number }),
            None => Err(format!("expected a 64-bit integer, found {number}")),
        }
    }

    /// Refuse an integer literal outside i64 that a lossy read brought into range. A literal
    /// with a fraction or exponent is a float in both serde_json and Python, and the f64
    /// comparison already decides it the same way in both.
    pub fn check_token(token: &str) -> Result<(), String> {
        if token.contains(['.', 'e', 'E']) || token.parse::<i64>().is_ok() {
            Ok(())
        } else {
            Err(format!("expected a 64-bit integer, found {token}"))
        }
    }

    pub fn get(&self) -> i64 {
        self.value
    }
}

impl Serialize for Signed {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.number.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Signed {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self::from_number(serde_json::Number::deserialize(deserializer)?).map_err(D::Error::custom)
    }
}

impl JsonSchema for Signed {
    fn inline_schema() -> bool {
        true
    }

    fn schema_name() -> Cow<'static, str> {
        "Signed".into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({"type": "integer", "minimum": i64::MIN, "maximum": i64::MAX})
    }
}

/// A declared symptom campaign's xdist worker count: 0 (serial), 2, 3 or 4, kept as written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Parallel {
    value: u64,
    number: serde_json::Number,
}

impl Parallel {
    pub const VALUES: [u64; 4] = [0, 2, 3, 4];

    pub fn new(value: u64) -> Result<Self, String> {
        Self::from_number(value.into())
    }

    pub fn from_number(number: serde_json::Number) -> Result<Self, String> {
        match integral(&number) {
            Some(value)
                if Self::VALUES
                    .iter()
                    .any(|&allowed| i128::from(allowed) == value) =>
            {
                Ok(Self {
                    value: value as u64,
                    number,
                })
            }
            _ => Err(format!("parallel must be 0, 2, 3 or 4, not {number}")),
        }
    }

    pub fn get(&self) -> u64 {
        self.value
    }
}

impl Serialize for Parallel {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.number.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Parallel {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self::from_number(serde_json::Number::deserialize(deserializer)?).map_err(D::Error::custom)
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

/// The runner's record protocol version, `1`. JSON Schema's `const` compares numbers by
/// value, so `1.0` is the same constant; the number is kept as written, because the record
/// is hashed (#750 re-review finding 5).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordSchemaVersion(serde_json::Number);

impl RecordSchemaVersion {
    pub fn new() -> Self {
        Self(1u64.into())
    }

    pub fn from_number(number: serde_json::Number) -> Result<Self, String> {
        if integral(&number) == Some(1) {
            Ok(Self(number))
        } else {
            Err(format!("expected 1, found {number}"))
        }
    }
}

impl Default for RecordSchemaVersion {
    fn default() -> Self {
        Self::new()
    }
}

impl Serialize for RecordSchemaVersion {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for RecordSchemaVersion {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self::from_number(serde_json::Number::deserialize(deserializer)?).map_err(D::Error::custom)
    }
}

impl JsonSchema for RecordSchemaVersion {
    fn inline_schema() -> bool {
        true
    }

    fn schema_name() -> Cow<'static, str> {
        "RecordSchemaVersion".into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({"const": 1})
    }
}

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

// P1b keeps JSON numbers as written: the existing schemas accept integral floats and
// unbounded integers, while forecast() intentionally diagnoses them after validation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct ReplyNumber<
    const MIN: i64,
    const MAX: u64,
    const LOWER: bool,
    const UPPER: bool,
    const INTEGRAL: bool,
>(serde_json::Number);

impl<
    'de,
    const MIN: i64,
    const MAX: u64,
    const LOWER: bool,
    const UPPER: bool,
    const INTEGRAL: bool,
> Deserialize<'de> for ReplyNumber<MIN, MAX, LOWER, UPPER, INTEGRAL>
{
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let number = serde_json::Number::deserialize(deserializer)?;
        let value = number
            .as_f64()
            .ok_or_else(|| D::Error::custom("expected a finite number"))?;
        if (INTEGRAL && value.fract() != 0.0)
            || (LOWER && value < MIN as f64)
            || (UPPER && value > MAX as f64)
        {
            return Err(D::Error::custom("reply number outside its contract"));
        }
        Ok(Self(number))
    }
}

impl<const MIN: i64, const MAX: u64, const LOWER: bool, const UPPER: bool, const INTEGRAL: bool>
    JsonSchema for ReplyNumber<MIN, MAX, LOWER, UPPER, INTEGRAL>
{
    fn inline_schema() -> bool {
        true
    }
    fn schema_name() -> Cow<'static, str> {
        format!("ReplyNumber_{MIN}_{MAX}_{LOWER}_{UPPER}_{INTEGRAL}").into()
    }
    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        let mut schema = json_schema!({"type": if INTEGRAL { "integer" } else { "number" }});
        if LOWER {
            schema.insert("minimum".into(), MIN.into());
        }
        if UPPER {
            schema.insert("maximum".into(), MAX.into());
        }
        schema
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct ReplyZero(serde_json::Number);
impl<'de> Deserialize<'de> for ReplyZero {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let number = serde_json::Number::deserialize(deserializer)?;
        if number.as_f64() == Some(0.0) {
            Ok(Self(number))
        } else {
            Err(D::Error::custom("expected zero"))
        }
    }
}
impl JsonSchema for ReplyZero {
    fn inline_schema() -> bool {
        true
    }
    fn schema_name() -> Cow<'static, str> {
        "ReplyZero".into()
    }
    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({"const": 0})
    }
}

// The pattern is also the parser's policy. These are the patterns already shipped in
// reply schemas; no regex dependency enters the bottom contracts crate.
// Match jsonschema-regex 0.46.10's translation of the published `\S`, including its
// limited Unicode set. Rust's is_whitespace and the engine's optimized whitespace
// path both use different sets; substituting either would change accepted replies.
pub fn schema_pattern_whitespace(c: char) -> bool {
    matches!(
        c,
        '\t' | '\n'
            | '\x0b'
            | '\x0c'
            | '\r'
            | ' '
            | '\u{00a0}'
            | '\u{feff}'
            | '\u{2003}'
            | '\u{2029}'
    )
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct ReplyText<const PATTERN: u8, const MIN: usize, const MAX: usize>(Text<MIN, MAX>);
impl<const PATTERN: u8, const MIN: usize, const MAX: usize> ReplyText<PATTERN, MIN, MAX> {
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
    fn pattern() -> &'static str {
        match PATTERN {
            0 | 1 => r"\S",
            2 => r"^[^\r\n]+$",
            3 => r"^tool-error-[1-9][0-9]*$",
            4 => UnitPath::PATTERN,
            5 => Sha256Hex::PATTERN,
            6 => CommitSha::PATTERN,
            _ => panic!("unknown reply text policy"),
        }
    }
}
impl<'de, const PATTERN: u8, const MIN: usize, const MAX: usize> Deserialize<'de>
    for ReplyText<PATTERN, MIN, MAX>
{
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = Text::<MIN, MAX>::deserialize(deserializer)?;
        let s = text.as_str();
        let valid = match PATTERN {
            0 => s.chars().any(|c| !schema_pattern_whitespace(c)),
            1 => {
                s.chars().any(|c| !schema_pattern_whitespace(c))
                    && !s.contains(['\r', '\n', '\u{85}', '\u{2028}', '\u{2029}'])
            }
            2 => !s.is_empty() && !s.contains(['\r', '\n']),
            3 => s.strip_prefix("tool-error-").is_some_and(|n| {
                !n.is_empty() && !n.starts_with('0') && n.bytes().all(|c| c.is_ascii_digit())
            }),
            4 => unit_path(s),
            5 => s.len() == 64 && lower_hex(s),
            6 => matches!(s.len(), 40 | 64) && lower_hex(s),
            _ => false,
        };
        if valid {
            Ok(Self(text))
        } else {
            Err(D::Error::custom("reply text does not match its pattern"))
        }
    }
}
impl<const PATTERN: u8, const MIN: usize, const MAX: usize> JsonSchema
    for ReplyText<PATTERN, MIN, MAX>
{
    fn inline_schema() -> bool {
        true
    }
    fn schema_name() -> Cow<'static, str> {
        format!("ReplyText_{PATTERN}_{MIN}_{MAX}").into()
    }
    fn json_schema(g: &mut SchemaGenerator) -> Schema {
        let mut schema = Text::<MIN, MAX>::json_schema(g);
        schema.insert("pattern".into(), Self::pattern().into());
        if PATTERN == 1 {
            schema.insert(
                "not".into(),
                serde_json::json!({"pattern": r"[\r\n\u0085\u2028\u2029]"}),
            );
        }
        schema
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ReplyNullable<T>(pub Option<T>);
impl<T: JsonSchema> JsonSchema for ReplyNullable<T> {
    fn inline_schema() -> bool {
        true
    }
    fn schema_name() -> Cow<'static, str> {
        format!("ReplyNullable_{}", T::schema_name()).into()
    }
    fn json_schema(g: &mut SchemaGenerator) -> Schema {
        let mut schema = T::json_schema(g);
        if let Some(values) = schema
            .get_mut("enum")
            .and_then(serde_json::Value::as_array_mut)
        {
            values.insert(0, serde_json::Value::Null);
        } else if let Some(kind) = schema
            .get("type")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned)
        {
            schema.insert("type".into(), serde_json::json!([kind, "null"]));
        } else {
            panic!("nullable reply leaf must have a type or enum");
        }
        schema
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ReplyNullChoice<T>(pub Option<T>);
impl<T: JsonSchema> JsonSchema for ReplyNullChoice<T> {
    fn inline_schema() -> bool {
        true
    }
    fn schema_name() -> Cow<'static, str> {
        format!("ReplyNullChoice_{}", T::schema_name()).into()
    }
    fn json_schema(g: &mut SchemaGenerator) -> Schema {
        json_schema!({"oneOf": [T::json_schema(g), {"type":"null"}]})
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct ReplyMaxText<T, const MAX: usize>(T);
impl<'de, T: Deserialize<'de> + Serialize, const MAX: usize> Deserialize<'de>
    for ReplyMaxText<T, MAX>
{
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = T::deserialize(deserializer)?;
        let json = serde_json::to_value(&text).map_err(D::Error::custom)?;
        if json.as_str().is_some_and(|s| s.chars().count() <= MAX) {
            Ok(Self(text))
        } else {
            Err(D::Error::custom("reply text exceeds field bound"))
        }
    }
}
impl<T: JsonSchema, const MAX: usize> JsonSchema for ReplyMaxText<T, MAX> {
    fn inline_schema() -> bool {
        true
    }
    fn schema_name() -> Cow<'static, str> {
        format!("ReplyMaxText_{}_{MAX}", T::schema_name()).into()
    }
    fn json_schema(g: &mut SchemaGenerator) -> Schema {
        let mut s = g.subschema_for::<T>();
        s.insert("maxLength".into(), MAX.into());
        s
    }
}

pub fn reply_enum_schema(schema: &mut Schema) {
    schema.remove("type");
}
pub fn reply_constant_schema(schema: &mut Schema) {
    schema.remove("type");
    if let Some(serde_json::Value::Array(mut values)) = schema.remove("enum") {
        schema.insert("const".into(), values.remove(0));
    }
}
pub fn reply_one_of(schema: &mut Schema) {
    if let Some(variants) = schema.remove("anyOf") {
        schema.insert("oneOf".into(), variants);
    }
}

/// Obtain validation rules from the reply types, before the shared order-only renderer.
pub fn reply_schema_value<T: JsonSchema>(
    role: &str,
    prepare: fn(&mut SchemaGenerator),
) -> serde_json::Value {
    let mut generator = schemars::generate::SchemaSettings::draft2020_12().into_generator();
    prepare(&mut generator);
    let mut schema = generator.into_root_schema_for::<T>().to_value();
    fn strip(value: &mut serde_json::Value) {
        match value {
            serde_json::Value::Object(obj) => {
                obj.remove("title");
                obj.remove("default");
                obj.remove("format");
                for child in obj.values_mut() {
                    strip(child);
                }
            }
            serde_json::Value::Array(items) => items.iter_mut().for_each(strip),
            _ => {}
        }
    }
    strip(&mut schema);
    schema.as_object_mut().expect("reply root schema").insert(
        "$id".into(),
        format!("{}{}-reply.json", super::SCHEMA_BASE, role).into(),
    );
    schema
}
