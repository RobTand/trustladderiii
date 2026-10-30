use serde::{
    Deserialize, Deserializer,
    de::{self, MapAccess, SeqAccess, Visitor},
};
use serde_json::{Map, Number, Value};
use std::fmt;

struct Document(Value);

impl<'de> Deserialize<'de> for Document {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Json;
        impl<'de> Visitor<'de> for Json {
            type Value = Document;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("JSON without duplicate object keys")
            }
            fn visit_bool<E: de::Error>(self, v: bool) -> Result<Document, E> {
                Ok(Document(Value::Bool(v)))
            }
            fn visit_i64<E: de::Error>(self, v: i64) -> Result<Document, E> {
                Ok(Document(Value::Number(v.into())))
            }
            fn visit_u64<E: de::Error>(self, v: u64) -> Result<Document, E> {
                Ok(Document(Value::Number(v.into())))
            }
            fn visit_f64<E: de::Error>(self, v: f64) -> Result<Document, E> {
                Number::from_f64(v)
                    .map(|n| Document(Value::Number(n)))
                    .ok_or_else(|| E::custom("non-finite JSON number"))
            }
            fn visit_str<E: de::Error>(self, v: &str) -> Result<Document, E> {
                Ok(Document(Value::String(v.into())))
            }
            fn visit_string<E: de::Error>(self, v: String) -> Result<Document, E> {
                Ok(Document(Value::String(v)))
            }
            fn visit_unit<E: de::Error>(self) -> Result<Document, E> {
                Ok(Document(Value::Null))
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Document, A::Error> {
                let mut values = Vec::new();
                while let Some(Document(value)) = seq.next_element()? {
                    values.push(value);
                }
                Ok(Document(Value::Array(values)))
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Document, A::Error> {
                let mut values = Map::new();
                while let Some((key, Document(value))) = map.next_entry::<String, Document>()? {
                    if values.insert(key, value).is_some() {
                        return Err(de::Error::custom("duplicate JSON key"));
                    }
                }
                Ok(Document(Value::Object(values)))
            }
        }
        deserializer.deserialize_any(Json)
    }
}

pub(super) fn parse(bytes: &[u8]) -> Result<Value, serde_json::Error> {
    serde_json::from_slice::<Document>(bytes).map(|v| v.0)
}
