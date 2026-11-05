use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub type Result<T> = std::result::Result<T, Refusal>;

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct Refusal {
    #[serde(rename = "type")]
    pub kind: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub execution: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cause: Option<Box<Refusal>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub json: Option<JsonError>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unicode: Option<UnicodeError>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct UnicodeError {
    pub raw: String,
    pub start: usize,
    pub end: usize,
    pub reason: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct JsonError {
    pub reason: String,
    pub document: String,
    pub position: usize,
}

impl Refusal {
    pub fn value(message: impl Into<String>) -> Self {
        Self {
            kind: "ValueError".into(),
            message: message.into(),
            execution: None,
            cause: None,
            json: None,
            unicode: None,
        }
    }
    pub fn typed(kind: &str, message: impl Into<String>) -> Self {
        Self { kind: kind.into(), ..Self::value(message) }
    }
    pub fn key(name: &str) -> Self {
        Self {
            kind: "KeyError".into(),
            message: format!("'{name}'"),
            ..Self::value("")
        }
    }
    pub fn transport(phase: &str, reason: &str, cause: Option<Self>) -> Self {
        Self {
            kind: "ExecutionFailure".into(),
            message: format!(
                "Test execution transport_failure in {phase}: {reason}. No test verdict is available; inspect this action, not fleet availability."
            ),
            execution: Some(
                json!({"outcome":"transport_failure","phase":phase,"termination_reason":reason,"test_started":false}),
            ),
            cause: cause.map(Box::new),
            json: None,
            unicode: None,
        }
    }
    pub fn utf8(raw: &[u8], error: std::str::Utf8Error) -> Self {
        use base64::Engine;
        let start = error.valid_up_to();
        let end = start + error.error_len().unwrap_or(raw.len()-start);
        let reason = if error.error_len().is_none() { "unexpected end of data" }
            else if raw[start] < 0xc2 || raw[start] > 0xf4 { "invalid start byte" }
            else { "invalid continuation byte" };
        let detail = if end == start + 1 { format!("byte 0x{:02x} in position {start}",raw[start]) }
            else { format!("bytes in position {start}-{}",end-1) };
        Self { kind: "UnicodeDecodeError".into(), message: format!("'utf-8' codec can't decode {detail}: {reason}"),
            unicode: Some(UnicodeError {raw: base64::engine::general_purpose::STANDARD.encode(raw),start,end,reason: reason.into()}),
            ..Self::value("") }
    }
    pub fn json(reason: &str, document: &str, position: usize) -> Self {
        let position = position.min(document.len());
        let prefix = &document[..position];
        let line = prefix.bytes().filter(|b| *b == b'\n').count() + 1;
        let column = prefix.rsplit('\n').next().unwrap_or("").chars().count() + 1;
        let character = prefix.chars().count();
        Self {
            kind: "JSONDecodeError".into(),
            message: format!("{reason}: line {line} column {column} (char {character})"),
            json: Some(JsonError {
                reason: reason.into(),
                document: document.into(),
                position: character,
            }),
            ..Self::value("")
        }
    }
}

pub fn parse(raw: &str) -> Result<Value> {
    serde_json::from_str(raw).map_err(|error| {
        let text = error.to_string();
        let position = if error.is_eof() {
            raw.len()
        } else {
            raw.split_inclusive('\n')
                .take(error.line().saturating_sub(1))
                .map(str::len)
                .sum::<usize>()
                + error.column().saturating_sub(1)
        };
        let reason = if text.starts_with("trailing characters") {
            "Extra data"
        } else if text.starts_with("key must be a string")
            || text.starts_with("EOF while parsing an object")
        {
            "Expecting property name enclosed in double quotes"
        } else if text.starts_with("expected `:`") {
            "Expecting ':' delimiter"
        } else if text.starts_with("expected `,`") {
            "Expecting ',' delimiter"
        } else {
            "Expecting value"
        };
        Refusal::json(reason, raw, position)
    })
}

pub fn get<'a>(value: &'a Value, key: &str) -> Result<&'a Value> {
    match value {
        Value::Object(fields) => fields.get(key).ok_or_else(|| Refusal::key(key)),
        Value::Array(_) => Err(Refusal::typed("TypeError", "list indices must be integers or slices, not str")),
        Value::String(_) => Err(Refusal::typed("TypeError", "string indices must be integers, not 'str'")),
        _ => Err(Refusal::typed("TypeError", format!("'{}' object is not subscriptable", python_type(value)))),
    }
}

pub fn python_type(value: &Value) -> &'static str {
    match value {
        Value::Null => "NoneType", Value::Bool(_) => "bool", Value::String(_) => "str",
        Value::Array(_) => "list", Value::Object(_) => "dict",
        Value::Number(n) => if n.is_f64() { "float" } else { "int" },
    }
}
