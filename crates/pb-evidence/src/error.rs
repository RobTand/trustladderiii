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
    pub json: Option<Box<JsonError>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unicode: Option<Box<UnicodeError>>,
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
        Self {
            kind: kind.into(),
            ..Self::value(message)
        }
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
        let end = start + error.error_len().unwrap_or(raw.len() - start);
        let reason = if error.error_len().is_none() {
            "unexpected end of data"
        } else if raw[start] < 0xc2 || raw[start] > 0xf4 {
            "invalid start byte"
        } else {
            "invalid continuation byte"
        };
        let detail = if end == start + 1 {
            format!("byte 0x{:02x} in position {start}", raw[start])
        } else {
            format!("bytes in position {start}-{}", end - 1)
        };
        Self {
            kind: "UnicodeDecodeError".into(),
            message: format!("'utf-8' codec can't decode {detail}: {reason}"),
            unicode: Some(Box::new(UnicodeError {
                raw: base64::engine::general_purpose::STANDARD.encode(raw),
                start,
                end,
                reason: reason.into(),
            })),
            ..Self::value("")
        }
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
            json: Some(Box::new(JsonError {
                reason: reason.into(),
                document: document.into(),
                position: character,
            })),
            ..Self::value("")
        }
    }
}

// Diagnose malformed JSON using CPython's grammar and error positions. Serde's
// messages and EOF positions differ, so they cannot define Python cause text.
struct JsonSyntax<'a> {
    raw: &'a str,
    at: usize,
}
impl JsonSyntax<'_> {
    fn fail<T>(&self, reason: &str, at: usize) -> Result<T> {
        Err(Refusal::json(reason, self.raw, at))
    }
    fn byte(&self) -> Option<u8> {
        self.raw.as_bytes().get(self.at).copied()
    }
    fn space(&mut self) {
        while matches!(self.byte(), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.at += 1;
        }
    }
    fn string(&mut self) -> Result<()> {
        let opening = self.at;
        self.at += 1;
        while let Some(byte) = self.byte() {
            match byte {
                b'"' => {
                    self.at += 1;
                    return Ok(());
                }
                b'\\' => {
                    let slash = self.at;
                    self.at += 1;
                    match self.byte() {
                        Some(b'"' | b'\\' | b'/' | b'b' | b'f' | b'n' | b'r' | b't') => {
                            self.at += 1
                        }
                        Some(b'u') => {
                            let marker = self.at;
                            self.at += 1;
                            for _ in 0..4 {
                                if !self.byte().is_some_and(|b| b.is_ascii_hexdigit()) {
                                    return self.fail("Invalid \\uXXXX escape", marker);
                                }
                                self.at += 1;
                            }
                        }
                        None => return self.fail("Unterminated string starting at", opening),
                        _ => return self.fail("Invalid \\escape", slash),
                    }
                }
                0..=31 => return self.fail("Invalid control character at", self.at),
                _ => {
                    self.at += self.raw[self.at..]
                        .chars()
                        .next()
                        .expect("remaining char")
                        .len_utf8()
                }
            }
        }
        self.fail("Unterminated string starting at", opening)
    }
    fn value(&mut self) -> Result<()> {
        self.space();
        match self.byte() {
            Some(b'"') => self.string(),
            Some(b'[') => {
                self.at += 1;
                self.space();
                if self.byte() == Some(b']') {
                    self.at += 1;
                    return Ok(());
                }
                loop {
                    self.value()?;
                    self.space();
                    match self.byte() {
                        Some(b']') => {
                            self.at += 1;
                            return Ok(());
                        }
                        Some(b',') => self.at += 1,
                        _ => return self.fail("Expecting ',' delimiter", self.at),
                    }
                }
            }
            Some(b'{') => {
                self.at += 1;
                self.space();
                if self.byte() == Some(b'}') {
                    self.at += 1;
                    return Ok(());
                }
                loop {
                    self.space();
                    if self.byte() != Some(b'"') {
                        return self
                            .fail("Expecting property name enclosed in double quotes", self.at);
                    }
                    self.string()?;
                    self.space();
                    if self.byte() != Some(b':') {
                        return self.fail("Expecting ':' delimiter", self.at);
                    }
                    self.at += 1;
                    self.value()?;
                    self.space();
                    match self.byte() {
                        Some(b'}') => {
                            self.at += 1;
                            return Ok(());
                        }
                        Some(b',') => self.at += 1,
                        _ => return self.fail("Expecting ',' delimiter", self.at),
                    }
                }
            }
            _ => {
                for word in ["null", "true", "false", "NaN", "Infinity", "-Infinity"] {
                    if self.raw[self.at..].starts_with(word) {
                        self.at += word.len();
                        return Ok(());
                    }
                }
                let start = self.at;
                if self.byte() == Some(b'-') {
                    self.at += 1;
                }
                match self.byte() {
                    Some(b'0') => self.at += 1,
                    Some(b'1'..=b'9') => {
                        while self.byte().is_some_and(|b| b.is_ascii_digit()) {
                            self.at += 1;
                        }
                    }
                    _ => return self.fail("Expecting value", start),
                }
                if self.byte() == Some(b'.')
                    && self
                        .raw
                        .as_bytes()
                        .get(self.at + 1)
                        .is_some_and(|b| b.is_ascii_digit())
                {
                    self.at += 1;
                    while self.byte().is_some_and(|b| b.is_ascii_digit()) {
                        self.at += 1;
                    }
                }
                if matches!(self.byte(), Some(b'e' | b'E')) {
                    let exponent = self.at;
                    self.at += 1;
                    if matches!(self.byte(), Some(b'+' | b'-')) {
                        self.at += 1;
                    }
                    if self.byte().is_some_and(|b| b.is_ascii_digit()) {
                        while self.byte().is_some_and(|b| b.is_ascii_digit()) {
                            self.at += 1;
                        }
                    } else {
                        self.at = exponent;
                    }
                }
                Ok(())
            }
        }
    }
}

pub fn parse(raw: &str) -> Result<Value> {
    match serde_json::from_str(raw) {
        Ok(mut value) => {
            crate::python_json::normalize_floats(&mut value);
            Ok(value)
        }
        Err(error) => {
            if raw.starts_with('\u{feff}') {
                return Err(Refusal::json(
                    "Unexpected UTF-8 BOM (decode using utf-8-sig)",
                    raw,
                    0,
                ));
            }
            let mut syntax = JsonSyntax { raw, at: 0 };
            syntax.value()?;
            syntax.space();
            if syntax.at < raw.len() {
                return Err(Refusal::json("Extra data", raw, syntax.at));
            }
            // Valid JSON rejected for a representation/depth constraint is not a
            // malformed token; retain the decoder diagnostic for that boundary.
            Err(Refusal::value(error.to_string()))
        }
    }
}

pub fn get<'a>(value: &'a Value, key: &str) -> Result<&'a Value> {
    match value {
        Value::Object(fields) => fields.get(key).ok_or_else(|| Refusal::key(key)),
        Value::Array(_) => Err(Refusal::typed(
            "TypeError",
            "list indices must be integers or slices, not str",
        )),
        Value::String(_) => Err(Refusal::typed(
            "TypeError",
            "string indices must be integers, not 'str'",
        )),
        _ => Err(Refusal::typed(
            "TypeError",
            format!("'{}' object is not subscriptable", python_type(value)),
        )),
    }
}

pub fn python_type(value: &Value) -> &'static str {
    match value {
        Value::Null => "NoneType",
        Value::Bool(_) => "bool",
        Value::String(_) => "str",
        Value::Array(_) => "list",
        Value::Object(_) => "dict",
        Value::Number(n) => {
            if n.is_f64() {
                "float"
            } else {
                "int"
            }
        }
    }
}
