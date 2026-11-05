//! Ordered schema presentation and Rust-first wire declarations. Object order is part
//! of the prompt bytes, so a Value (whose maps sort keys) is insufficient (#648).
pub(super) use super::super::bounded::{List, Parallel, Text, True, UNBOUNDED, present};
pub(super) use schemars::{JsonSchema, Schema, SchemaGenerator};
pub(super) use serde::{
    Deserialize, Deserializer, Serialize, Serializer, de::Error as _, ser::SerializeMap,
};
pub(super) use serde_json::{Number, Value};
pub(super) use std::{borrow::Cow, collections::BTreeMap};

#[derive(Clone, Debug)]
pub enum Doc {
    Object(Vec<(String, Doc)>),
    Array(Vec<Doc>),
    Atom(Value),
}
impl Serialize for Doc {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Object(fields) => {
                let mut map = s.serialize_map(Some(fields.len()))?;
                for (key, value) in fields {
                    map.serialize_entry(key, value)?;
                }
                map.end()
            }
            Self::Array(items) => items.serialize(s),
            Self::Atom(value) => value.serialize(s),
        }
    }
}
impl Doc {
    pub fn atom(value: impl Into<Value>) -> Self {
        Self::Atom(value.into())
    }
    pub fn object(fields: Vec<(&str, Doc)>) -> Self {
        Self::Object(fields.into_iter().map(|(k, v)| (k.into(), v)).collect())
    }
    pub fn set(&mut self, key: &str, value: Doc) {
        if let Self::Object(fields) = self {
            if let Some((_, old)) = fields.iter_mut().find(|(k, _)| k == key) {
                *old = value;
            } else {
                fields.push((key.into(), value));
            }
        }
    }
    pub fn take(&mut self, key: &str) -> Option<Doc> {
        if let Self::Object(fields) = self {
            let i = fields.iter().position(|(k, _)| k == key)?;
            Some(fields.remove(i).1)
        } else {
            None
        }
    }
    pub fn ordered(mut self, order: &[&str]) -> Self {
        let mut out = Vec::new();
        for key in order {
            if let Some(value) = self.take(key) {
                out.push(((*key).into(), value));
            }
        }
        if let Self::Object(fields) = self {
            out.extend(fields);
        }
        Self::Object(out)
    }
    pub fn value(&self) -> Value {
        serde_json::to_value(self).expect("schema serializes")
    }
}

pub trait WireSchema {
    fn wire_schema() -> Doc;
}
impl<const MIN: usize, const MAX: usize> WireSchema for Text<MIN, MAX> {
    fn wire_schema() -> Doc {
        let mut s = Doc::object(vec![("type", Doc::atom("string"))]);
        if MIN > 0 {
            s.set("minLength", Doc::atom(MIN));
        }
        if MAX != UNBOUNDED {
            s.set("maxLength", Doc::atom(MAX));
        }
        s
    }
}
impl<T: WireSchema, const MIN: usize, const MAX: usize, const UNIQUE: bool> WireSchema
    for List<T, MIN, MAX, UNIQUE>
{
    fn wire_schema() -> Doc {
        let mut s = Doc::object(vec![
            ("type", Doc::atom("array")),
            ("items", T::wire_schema()),
        ]);
        if MIN > 0 {
            s.set("minItems", Doc::atom(MIN));
        }
        if MAX != UNBOUNDED {
            s.set("maxItems", Doc::atom(MAX));
        }
        if UNIQUE {
            s.set("uniqueItems", Doc::atom(true));
        }
        s
    }
}
impl<T: WireSchema> WireSchema for Option<T> {
    fn wire_schema() -> Doc {
        T::wire_schema()
    }
}
impl<T: WireSchema> WireSchema for BTreeMap<String, T> {
    fn wire_schema() -> Doc {
        Doc::object(vec![
            ("type", Doc::atom("object")),
            ("additionalProperties", T::wire_schema()),
        ])
    }
}
impl WireSchema for bool {
    fn wire_schema() -> Doc {
        Doc::object(vec![("type", Doc::atom("boolean"))])
    }
}
impl WireSchema for super::super::bounded::Parallel {
    fn wire_schema() -> Doc {
        Doc::object(vec![("enum", Doc::Atom(serde_json::json!([0, 2, 3, 4])))])
    }
}
impl WireSchema for super::super::bounded::True {
    fn wire_schema() -> Doc {
        Doc::object(vec![("const", Doc::atom(true))])
    }
}

/// JSON Schema integers include integral floats and have no implicit u64 ceiling.
/// Keep the original number for round trips; decision consumers explicitly ask for u64.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct Whole<const MIN: i64, const MAX: u64>(Number);
impl<const MIN: i64, const MAX: u64> Whole<MIN, MAX> {
    pub fn as_u64(&self) -> Option<u64> {
        self.0.as_u64()
    }
}
impl<'de, const MIN: i64, const MAX: u64> Deserialize<'de> for Whole<MIN, MAX> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let n = Number::deserialize(d)?;
        let valid = if let Some(n) = n.as_i64() {
            n >= MIN && (MAX == u64::MAX || (n >= 0 && n as u64 <= MAX))
        } else if let Some(n) = n.as_u64() {
            (MIN <= 0 || n >= MIN as u64) && n <= MAX
        } else {
            n.as_f64().is_some_and(|n| {
                n.fract() == 0.0 && n >= MIN as f64 && (MAX == u64::MAX || n <= MAX as f64)
            })
        };
        if valid {
            Ok(Self(n))
        } else {
            Err(D::Error::custom("integer outside wire bounds"))
        }
    }
}
impl<const MIN: i64, const MAX: u64> WireSchema for Whole<MIN, MAX> {
    fn wire_schema() -> Doc {
        let mut s = Doc::object(vec![("type", Doc::atom("integer"))]);
        if MIN != i64::MIN {
            s.set("minimum", Doc::atom(MIN));
        }
        if MAX != u64::MAX {
            s.set("maximum", Doc::atom(MAX));
        }
        s
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct NonNegative(Number);
impl<'de> Deserialize<'de> for NonNegative {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let n = Number::deserialize(d)?;
        if n.as_f64().is_some_and(|n| n >= 0.0) {
            Ok(Self(n))
        } else {
            Err(D::Error::custom("negative wire number"))
        }
    }
}
impl WireSchema for NonNegative {
    fn wire_schema() -> Doc {
        Doc::object(vec![
            ("type", Doc::atom("number")),
            ("minimum", Doc::atom(0)),
        ])
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Maybe<T> {
    Value(T),
    Null(()),
}
impl<T: WireSchema> WireSchema for Maybe<T> {
    fn wire_schema() -> Doc {
        let mut s = T::wire_schema();
        let ty = s.take("type").expect("nullable scalar type");
        s.set("type", Doc::Array(vec![ty, Doc::atom("null")]));
        s.ordered(&["type"])
    }
}
macro_rules! schema_impl {
    ($name:ident) => {
        impl JsonSchema for $name {
            fn schema_name() -> Cow<'static, str> {
                stringify!($name).into()
            }
            fn json_schema(_: &mut SchemaGenerator) -> Schema {
                Schema::try_from(Self::wire_schema().value()).expect("object schema")
            }
        }
    };
}
macro_rules! checked_wire_object {
    ($name:ident, [$($required:literal),*], [$($order:literal),*], $extra:expr, {
        $( $(#[$attr:meta])* $field:ident : $ty:ty, [$($field_order:literal),*], $annotations:expr; )*
    }, $check:expr) => {
        #[derive(Clone,Debug,PartialEq,Eq,Serialize)]
        pub struct $name { $( $(#[$attr])* pub $field:$ty, )* }
        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D:Deserializer<'de>>(d:D)->Result<Self,D::Error> {
                #[derive(Deserialize)]
                #[serde(deny_unknown_fields)]
                struct Fields { $( $(#[$attr])* $field:$ty, )* }
                let _fields=Fields::deserialize(d)?;
                let value=Self { $($field:_fields.$field,)* };
                let valid:fn(&Self)->bool=$check;
                if valid(&value) { Ok(value) } else { Err(D::Error::custom("inconsistent reply fields")) }
            }
        }
        impl WireSchema for $name {
            fn wire_schema()->Doc {
                let fields=vec![$((stringify!($field), {
                    let mut s=<$ty>::wire_schema();
                    let annotations:Vec<(&str,Doc)>=$annotations;
                    for (key,value) in annotations { s.set(key,value); }
                    s.ordered(&[$($field_order),*])
                })),*];
                let mut s=Doc::object(vec![("type",Doc::atom("object")),("properties",Doc::object(fields)),
                    ("required",Doc::Array(vec![$(Doc::atom($required)),*])),("additionalProperties",Doc::atom(false))]);
                let extra:Vec<(&str,Doc)>=$extra;
                for (key,value) in extra { s.set(key,value); }
                s.ordered(&[$($order),*])
            }
        }
        schema_impl!($name);
    };
}

macro_rules! wire_object {
    ($name:ident, $required:tt, $order:tt, $extra:expr, $fields:tt) => {
        checked_wire_object!($name, $required, $order, $extra, $fields, |_: &Self| true);
    };
}
macro_rules! wire_choice {
    ($name:ident, $keyword:literal, {$($variant:ident => $value:literal),+}) => {
        #[derive(Clone,Copy,Debug,PartialEq,Eq,Serialize,Deserialize)]
        pub enum $name { $(#[serde(rename=$value)] $variant),+ }
        impl WireSchema for $name {
            fn wire_schema()->Doc {
                let values=vec![$(Doc::atom($value)),+];
                Doc::object(vec![($keyword, if $keyword=="const" { values[0].clone() } else { Doc::Array(values) })])
            }
        }
        schema_impl!($name);
    };
}
macro_rules! wire_union {
    ($name:ident, {$($variant:ident($ty:ty)),+}) => {
        #[derive(Clone,Debug,PartialEq,Eq,Serialize,Deserialize)]
        #[serde(untagged)]
        pub enum $name { $($variant(Box<$ty>)),+ }
        impl WireSchema for $name {
            fn wire_schema()->Doc { Doc::object(vec![("oneOf",Doc::Array(vec![$(<$ty>::wire_schema()),+]))]) }
        }
        schema_impl!($name);
    };
}
macro_rules! wire_tagged {
    ($name:ident, $tag:literal, {$($variant:ident($ty:ty) => ($value:literal,$index:expr,$required_index:expr)),+}) => {
        #[derive(Clone,Debug,PartialEq,Eq,Serialize,Deserialize)]
        #[serde(tag=$tag)]
        pub enum $name { $(#[serde(rename=$value)] $variant(Box<$ty>)),+ }
        impl WireSchema for $name {
            fn wire_schema()->Doc {
                let variants=vec![$({
                    let mut s=<$ty>::wire_schema();
                    if let Some(Doc::Object(mut fields))=s.take("properties") {
                        fields.insert($index,($tag.into(),Doc::object(vec![("const",Doc::atom($value))])));
                        s.set("properties",Doc::Object(fields));
                    }
                    if let Some(Doc::Array(mut required))=s.take("required") {
                        required.insert($required_index,Doc::atom($tag)); s.set("required",Doc::Array(required));
                    }
                    s.ordered(&["type","description","properties","required","additionalProperties","allOf"])
                }),+];
                Doc::object(vec![("oneOf",Doc::Array(variants))])
            }
        }
        schema_impl!($name);
    };
}
macro_rules! wire_link {
    ($name:ident, $ty:ty, $reference:literal) => {
        #[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub $ty);
        impl WireSchema for $name {
            fn wire_schema() -> Doc {
                Doc::object(vec![("$ref", Doc::atom($reference))])
            }
        }
        schema_impl!($name);
    };
}
macro_rules! wire_text {
    ($name:ident,$min:expr,$max:expr,$pattern:literal,$negative:literal) => {
        #[derive(Clone, Debug, PartialEq, Eq, Serialize)]
        #[serde(transparent)]
        pub struct $name(Text<$min, $max>);
        impl $name {
            pub fn as_str(&self) -> &str {
                self.0.as_str()
            }
        }
        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                let text = Text::<$min, $max>::deserialize(d)?;
                let matches = |pattern: &str| {
                    fancy_regex::Regex::new(pattern)
                        .map_err(D::Error::custom)?
                        .is_match(text.as_str())
                        .map_err(D::Error::custom)
                };
                if (!$pattern.is_empty() && !matches($pattern)?)
                    || (!$negative.is_empty() && matches($negative)?)
                {
                    return Err(D::Error::custom("text breaks wire pattern"));
                }
                Ok(Self(text))
            }
        }
        impl WireSchema for $name {
            fn wire_schema() -> Doc {
                let mut s = Text::<$min, $max>::wire_schema();
                if !$pattern.is_empty() {
                    s.set("pattern", Doc::atom($pattern));
                }
                if !$negative.is_empty() {
                    s.set("not", Doc::object(vec![("pattern", Doc::atom($negative))]));
                }
                s
            }
        }
        schema_impl!($name);
    };
}

impl WireSchema for () {
    fn wire_schema() -> Doc {
        Doc::object(vec![("type", Doc::atom("null"))])
    }
}
macro_rules! wire_zero {
    ($name:ident) => {
        #[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(Whole<0, 0>);
        impl WireSchema for $name {
            fn wire_schema() -> Doc {
                Doc::object(vec![("const", Doc::atom(0))])
            }
        }
        schema_impl!($name);
    };
}
macro_rules! wire_nullable_choice {
    ($name:ident,$ty:ty) => {
        #[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub Option<$ty>);
        impl WireSchema for $name {
            fn wire_schema() -> Doc {
                let mut s = <$ty>::wire_schema();
                if let Some(Doc::Array(mut values)) = s.take("enum") {
                    values.insert(0, Doc::Atom(Value::Null));
                    s.set("enum", Doc::Array(values));
                }
                s
            }
        }
        schema_impl!($name);
    };
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct Limited<T, const MAX: usize>(T);
impl<'de, T: Deserialize<'de> + Serialize, const MAX: usize> Deserialize<'de> for Limited<T, MAX> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let value = T::deserialize(d)?;
        let text = serde_json::to_value(&value).map_err(D::Error::custom)?;
        if text
            .as_str()
            .is_some_and(|text| text.chars().count() <= MAX)
        {
            Ok(Self(value))
        } else {
            Err(D::Error::custom("text exceeds wire bound"))
        }
    }
}

impl<T: WireSchema, const MAX: usize> WireSchema for Limited<T, MAX> {
    fn wire_schema() -> Doc {
        T::wire_schema()
    }
}
