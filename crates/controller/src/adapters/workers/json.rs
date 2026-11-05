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

#[cfg(test)]
mod tests {
    use super::parse;

    #[test]
    fn scalar_tokens_keep_the_existing_controller_number_and_object_contract() {
        let value = parse(
            br#"{"integer":18446744073709551616,"float":0.1,"negative":-0.0,"$serde_json::private::Number":"literal"}"#,
        )
        .unwrap();
        assert!(value["integer"].is_f64());
        assert_eq!(
            value["integer"].as_f64(),
            Some(18_446_744_073_709_551_616.0)
        );
        assert_eq!(value["float"].as_f64(), Some(0.1));
        assert_eq!(value["negative"].to_string(), "-0.0");
        assert_eq!(value["$serde_json::private::Number"], "literal");
        assert!(parse(br#"{"nested":[{"a":1,"\u0061":2}]}"#).is_err());
        assert!(parse(b"1e400").is_err());
    }

    #[test]
    fn raw_scalar_decoding_retains_the_original_nesting_limit() {
        for (depth, accepted) in [(127, true), (128, false)] {
            let raw = format!("{}0{}", "[".repeat(depth), "]".repeat(depth));
            assert_eq!(parse(raw.as_bytes()).is_ok(), accepted, "depth {depth}");
        }
    }
}
