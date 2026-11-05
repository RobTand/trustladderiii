//! Python JSON text, without narrowing its integers, floats or Unicode strings.
//!
//! Numbers arrive from Python's json.dumps at the transport boundary. Their
//! spelling is already Python repr; parsing them as f64 would lose that fact.
use crate::{Error, Result};
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Value {
    Null,
    Bool(bool),
    Number(String),
    String(Vec<u32>),
    Array(Vec<Value>),
    Object(BTreeMap<Vec<u32>, Value>),
}

pub(super) fn text(value: &str) -> Vec<u32> {
    value.chars().map(u32::from).collect()
}

impl From<&str> for Value {
    fn from(value: &str) -> Self {
        Self::String(text(value))
    }
}

impl Value {
    pub(super) fn object(items: impl IntoIterator<Item = (&'static str, Self)>) -> Self {
        Self::Object(items.into_iter().map(|(k, v)| (text(k), v)).collect())
    }

    pub(super) fn get(&self, key: &str) -> &Self {
        self.find(key).unwrap_or(&Self::Null)
    }

    pub(super) fn find(&self, key: &str) -> Option<&Self> {
        match self {
            Self::Object(items) => items.get(&text(key)),
            _ => None,
        }
    }

    pub(super) fn string(&self) -> Result<String> {
        match self {
            Self::String(value) => utf8(value),
            _ => Err(Error::Invalid("prompt renderer expected text")),
        }
    }

    pub(super) fn dump(&self) -> Vec<u32> {
        let mut output = Vec::new();
        self.write(&mut output, false, false);
        output
    }

    pub(super) fn wire(&self) -> String {
        let mut output = Vec::new();
        self.write(&mut output, true, true);
        // The ASCII encoder emits only scalar values.
        output
            .into_iter()
            .map(|c| char::from_u32(c).unwrap())
            .collect()
    }

    fn write(&self, out: &mut Vec<u32>, ascii: bool, compact: bool) {
        let comma = if compact { "," } else { ", " };
        let colon = if compact { ":" } else { ": " };
        match self {
            Self::Null => out.extend(text("null")),
            Self::Bool(value) => out.extend(text(if *value { "true" } else { "false" })),
            Self::Number(value) => out.extend(text(value)),
            Self::String(value) => quoted(value, out, ascii),
            Self::Array(values) => {
                out.push('[' as u32);
                for (index, value) in values.iter().enumerate() {
                    if index != 0 {
                        out.extend(text(comma));
                    }
                    value.write(out, ascii, compact);
                }
                out.push(']' as u32);
            }
            Self::Object(values) => {
                out.push('{' as u32);
                for (index, (key, value)) in values.iter().enumerate() {
                    if index != 0 {
                        out.extend(text(comma));
                    }
                    quoted(key, out, ascii);
                    out.extend(text(colon));
                    value.write(out, ascii, compact);
                }
                out.push('}' as u32);
            }
        }
    }
}

fn quoted(value: &[u32], out: &mut Vec<u32>, ascii: bool) {
    out.push('"' as u32);
    for &c in value {
        match c {
            0x22 => out.extend(text("\\\"")),
            0x5c => out.extend(text("\\\\")),
            8 => out.extend(text("\\b")),
            12 => out.extend(text("\\f")),
            10 => out.extend(text("\\n")),
            13 => out.extend(text("\\r")),
            9 => out.extend(text("\\t")),
            0..=31 => out.extend(text(&format!("\\u{c:04x}"))),
            0x7f.. if ascii => {
                if c <= 0xffff {
                    out.extend(text(&format!("\\u{c:04x}")));
                } else {
                    let c = c - 0x10000;
                    out.extend(text(&format!(
                        "\\u{:04x}\\u{:04x}",
                        0xd800 + (c >> 10),
                        0xdc00 + (c & 0x3ff)
                    )));
                }
            }
            _ => out.push(c),
        }
    }
    out.push('"' as u32);
}

pub(super) fn utf8(value: &[u32]) -> Result<String> {
    value
        .iter()
        .map(|&c| char::from_u32(c).ok_or(Error::Invalid("prompt text contains a surrogate")))
        .collect()
}

pub(super) fn parse(input: &str) -> Result<Value> {
    let mut parser = Parser { input, position: 0 };
    let value = parser.value()?;
    parser.space();
    if parser.position != input.len() {
        return Err(invalid());
    }
    Ok(value)
}

fn invalid() -> Error {
    Error::Invalid("invalid render-prompt JSON")
}

struct Parser<'a> {
    input: &'a str,
    position: usize,
}

impl Parser<'_> {
    fn peek(&self) -> Option<u8> {
        self.input.as_bytes().get(self.position).copied()
    }
    fn space(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\n' | b'\r' | b'\t')) {
            self.position += 1;
        }
    }
    fn take(&mut self, byte: u8) -> Result<()> {
        self.space();
        if self.peek() != Some(byte) {
            return Err(invalid());
        }
        self.position += 1;
        Ok(())
    }
    fn literal(&mut self, literal: &str, value: Value) -> Result<Value> {
        if !self.input[self.position..].starts_with(literal) {
            return Err(invalid());
        }
        self.position += literal.len();
        Ok(value)
    }
    fn value(&mut self) -> Result<Value> {
        self.space();
        match self.peek() {
            Some(b'n') => self.literal("null", Value::Null),
            Some(b't') => self.literal("true", Value::Bool(true)),
            Some(b'f') => self.literal("false", Value::Bool(false)),
            Some(b'"') => self.string().map(Value::String),
            Some(b'[') => {
                self.position += 1;
                self.space();
                let mut items = Vec::new();
                if self.peek() != Some(b']') {
                    loop {
                        items.push(self.value()?);
                        self.space();
                        if self.peek() != Some(b',') {
                            break;
                        }
                        self.position += 1;
                    }
                }
                self.take(b']')?;
                Ok(Value::Array(items))
            }
            Some(b'{') => {
                self.position += 1;
                self.space();
                let mut items = BTreeMap::new();
                if self.peek() != Some(b'}') {
                    loop {
                        let key = self.string()?;
                        self.take(b':')?;
                        if items.insert(key, self.value()?).is_some() {
                            return Err(invalid());
                        }
                        self.space();
                        if self.peek() != Some(b',') {
                            break;
                        }
                        self.position += 1;
                        self.space();
                    }
                }
                self.take(b'}')?;
                Ok(Value::Object(items))
            }
            Some(b'-' | b'0'..=b'9') => {
                let start = self.position;
                while matches!(
                    self.peek(),
                    Some(b'-' | b'+' | b'.' | b'e' | b'E' | b'0'..=b'9')
                ) {
                    self.position += 1;
                }
                let token = &self.input[start..self.position];
                // RawValue validates the grammar without narrowing the numeric token.
                serde_json::from_str::<Box<serde_json::value::RawValue>>(token)
                    .map_err(|_| invalid())?;
                Ok(Value::Number(token.into()))
            }
            _ => Err(invalid()),
        }
    }
    fn hex(&mut self) -> Result<u32> {
        let end = self.position.checked_add(4).ok_or_else(invalid)?;
        let value = self.input.get(self.position..end).ok_or_else(invalid)?;
        let value = u32::from_str_radix(value, 16).map_err(|_| invalid())?;
        self.position = end;
        Ok(value)
    }
    fn string(&mut self) -> Result<Vec<u32>> {
        self.take(b'"')?;
        let mut value = Vec::new();
        loop {
            match self.peek().ok_or_else(invalid)? {
                b'"' => {
                    self.position += 1;
                    return Ok(value);
                }
                b'\\' => {
                    self.position += 1;
                    let escape = self.peek().ok_or_else(invalid)?;
                    self.position += 1;
                    let c = match escape {
                        b'"' | b'\\' | b'/' => escape as u32,
                        b'b' => 8,
                        b'f' => 12,
                        b'n' => 10,
                        b'r' => 13,
                        b't' => 9,
                        b'u' => {
                            let high = self.hex()?;
                            if (0xd800..=0xdbff).contains(&high)
                                && self.input[self.position..].starts_with("\\u")
                            {
                                let after = self.position;
                                self.position += 2;
                                let low = self.hex()?;
                                if (0xdc00..=0xdfff).contains(&low) {
                                    0x10000 + ((high - 0xd800) << 10) + low - 0xdc00
                                } else {
                                    self.position = after;
                                    high
                                }
                            } else {
                                high
                            }
                        }
                        _ => return Err(invalid()),
                    };
                    value.push(c);
                }
                0..=31 => return Err(invalid()),
                _ => {
                    let c = self.input[self.position..]
                        .chars()
                        .next()
                        .ok_or_else(invalid)?;
                    self.position += c.len_utf8();
                    value.push(c as u32);
                }
            }
        }
    }
}
