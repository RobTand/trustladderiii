use serde::{
    Deserialize, Deserializer,
    de::{self, DeserializeSeed, MapAccess, SeqAccess, Visitor},
};
use serde_json::{Map, Number, Value, value::RawValue};
use std::fmt;

struct Document(Value);
struct DocumentSeed(usize);

impl<'de> Deserialize<'de> for Document {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        DocumentSeed(0).deserialize(deserializer)
    }
}

impl<'de> DeserializeSeed<'de> for DocumentSeed {
    type Value = Document;

    fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<Document, D::Error> {
        // Read each scalar from its JSON token. arbitrary_precision presents
        // numbers as private maps to deserialize_any; those are not JSON objects.
        // Raw tokens also keep real objects with those private-looking keys intact.
        let raw = Box::<RawValue>::deserialize(deserializer)?;
        let container = matches!(raw.get().as_bytes()[0], b'[' | b'{');
        if container && self.0 >= 127 {
            return Err(de::Error::custom("recursion limit exceeded"));
        }
        struct Json(usize);
        impl<'de> Visitor<'de> for Json {
            type Value = Document;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("JSON without duplicate object keys")
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Document, A::Error> {
                let mut values = Vec::new();
                while let Some(Document(value)) = seq.next_element_seed(DocumentSeed(self.0 + 1))? {
                    values.push(value);
                }
                Ok(Document(Value::Array(values)))
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Document, A::Error> {
                let mut values = Map::new();
                while let Some(key) = map.next_key::<String>()? {
                    let Document(value) = map.next_value_seed(DocumentSeed(self.0 + 1))?;
                    if values.insert(key, value).is_some() {
                        return Err(de::Error::custom("duplicate JSON key"));
                    }
                }
                Ok(Document(Value::Object(values)))
            }
        }
        let mut parser = serde_json::Deserializer::from_str(raw.get());
        match raw.get().as_bytes()[0] {
            b'[' => parser
                .deserialize_seq(Json(self.0))
                .map_err(de::Error::custom),
            b'{' => parser
                .deserialize_map(Json(self.0))
                .map_err(de::Error::custom),
            _ => {
                let mut value: Value =
                    serde_json::from_str(raw.get()).map_err(de::Error::custom)?;
                // Retain this boundary's original finite-f64 decoding for floating
                // tokens and integers outside i64/u64. PB uses its separate reader.
                if let Value::Number(number) = &mut value
                    && !number.is_i64()
                    && !number.is_u64()
                {
                    *number = number
                        .as_f64()
                        .and_then(Number::from_f64)
                        .ok_or_else(|| <D::Error as de::Error>::custom("number out of range"))?;
                }
                Ok(Document(value))
            }
        }
    }
}

pub(crate) fn parse(bytes: &[u8]) -> Result<Value, serde_json::Error> {
    serde_json::from_slice::<Document>(bytes).map(|v| v.0)
}
