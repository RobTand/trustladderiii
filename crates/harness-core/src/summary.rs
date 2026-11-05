//! #823: bound a summary using the serializer used by its consuming envelope.
//!
//! `bounded(input, limit, serializer)` returns the retained value and a typed
//! `DropReport`. The serialized value, including any truncation notice, must fit
//! `limit`. Proof, totals and declared RED entries form an indivisible mandatory
//! core. A diagnostic is eligible only when that core exceeds `limit`; if the
//! diagnostic also does not fit, callers receive `BoundError::DiagnosticTooLarge`.
//!
//! `PythonEnvelope` reproduces default `json.dumps`: spaces after separators,
//! ASCII escapes, insertion-ordered object keys, and Python string code points.
//! Numeric atoms retain their boundary serialization; no floating-point decision
//! occurs here. A caller using another serializer receives a measured bound;
//! the Verus arithmetic contract applies to `PythonEnvelope`.

use serde::{Deserialize, Serialize};

/// Python strings can contain isolated surrogates, which Rust `String` cannot.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "Vec<u32>", into = "Vec<u32>")]
pub struct PythonText(Vec<u32>);

impl TryFrom<Vec<u32>> for PythonText {
    type Error = &'static str;

    fn try_from(value: Vec<u32>) -> Result<Self, Self::Error> {
        if value.iter().any(|point| *point > 0x10ffff) {
            return Err("Python code point exceeds U+10FFFF");
        }
        Ok(Self(value))
    }
}

impl From<PythonText> for Vec<u32> {
    fn from(value: PythonText) -> Self {
        value.0
    }
}

impl From<&str> for PythonText {
    fn from(value: &str) -> Self {
        Self(value.chars().map(u32::from).collect())
    }
}

impl PythonText {
    pub fn codepoints(&self) -> &[u32] {
        &self.0
    }

    pub fn prefix(&self, characters: usize) -> Self {
        Self(self.0.iter().copied().take(characters).collect())
    }
}

/// JSON objects retain insertion order. A number stores a validated JSON token.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SummaryValue {
    Null,
    Bool(bool),
    Number(JsonNumber),
    String(PythonText),
    Array(Vec<SummaryValue>),
    Object(Vec<(PythonText, SummaryValue)>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JsonNumber(String);

impl JsonNumber {
    pub fn parse(token: &str) -> Result<Self, BoundError> {
        serde_json::from_str::<serde_json::Number>(token)
            .map_err(|_| BoundError::InvalidInput("invalid numeric atom"))?;
        Ok(Self(token.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl SummaryValue {
    /// Parse Python JSON without losing object order or isolated surrogates.
    pub fn from_python_json(_input: &str) -> Result<Self, BoundError> {
        todo!("Red: preserve Python envelope values at the parsing boundary")
    }
}

pub trait SummarySerializer {
    fn serialize(&self, value: &SummaryValue) -> Result<Vec<u8>, BoundError>;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct PythonEnvelope;

impl SummarySerializer for PythonEnvelope {
    fn serialize(&self, _value: &SummaryValue) -> Result<Vec<u8>, BoundError> {
        todo!("Red: reproduce default Python json.dumps bytes")
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum PathPart {
    Field(String),
    Index(usize),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DroppedKind {
    SelectorCompaction,
    PassingEntry,
    ErrorEntry,
    FailureEntry,
    RunFailure,
    GroupDisplay,
    DisplayField,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Dropped {
    pub path: Vec<PathPart>,
    pub kind: DroppedKind,
    pub count: usize,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DropReport {
    pub dropped: Vec<Dropped>,
    pub diagnostic: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BoundedSummary {
    pub value: SummaryValue,
    /// Metadata is outside `value` and is not part of its byte budget.
    pub report: DropReport,
    pub serialized_bytes: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BoundError {
    InvalidInput(&'static str),
    DiagnosticTooLarge { required: usize, limit: usize },
    SizeOverflow,
}

impl std::fmt::Display for BoundError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidInput(reason) => formatter.write_str(reason),
            Self::DiagnosticTooLarge { required, limit } => {
                write!(
                    formatter,
                    "summary diagnostic requires {required} bytes; limit is {limit}"
                )
            }
            Self::SizeOverflow => formatter.write_str("summary size overflow"),
        }
    }
}

impl std::error::Error for BoundError {}

/// #704/#719/#723/#823: never return an oversized summary as a success.
pub fn bounded<S: SummarySerializer>(
    _input: SummaryValue,
    _limit: usize,
    _serializer: &S,
) -> Result<BoundedSummary, BoundError> {
    todo!("Red: measured bound with mandatory-core preservation")
}

/// #823: Python's ensure_ascii width for one string code point, without quotes.
#[cfg_attr(
    verus_keep_ghost,
    vstd::prelude::verus_spec(width => requires point <= 0x10ffff,
        ensures width >= 1, width <= 12,
        point > 0xffff ==> width == 12,
        point >= 0x80 && point <= 0xffff ==> width == 6,
        point == 127 ==> width == 6,
        point >= 32 && point <= 126 && point != 34 && point != 92 ==> width == 1,
        point == 8 || point == 9 || point == 10 || point == 12 || point == 13
            || point == 34 || point == 92 ==> width == 2,
        point < 32 && point != 8 && point != 9 && point != 10
            && point != 12 && point != 13 ==> width == 6,
    )
)]
pub fn escaped_codepoint_bytes(point: u32) -> u64 {
    match point {
        8 | 9 | 10 | 12 | 13 | 34 | 92 => 2,
        0..=31 | 127..=65535 => 6,
        32..=126 => 1,
        _ => 12,
    }
}

/// #823: subtraction-first budget arithmetic cannot overflow for any u64 budget.
#[cfg_attr(
    verus_keep_ghost,
    vstd::prelude::verus_spec(fits => ensures
        fits == ((mandatory as int) + (display as int) <= limit as int),
    )
)]
pub fn fits_budget(mandatory: u64, display: u64, limit: u64) -> bool {
    mandatory <= limit && display <= limit - mandatory
}

/// #823: diagnostic eligibility depends only on the mandatory core's size.
#[cfg_attr(
    verus_keep_ghost,
    vstd::prelude::verus_spec(level => requires core <= full,
        ensures level <= 2,
        (level == 0) == (full <= limit),
        (level == 1) == (core <= limit && full > limit),
        (level == 2) == (core > limit),
    )
)]
pub fn retention_level(full: u64, core: u64, limit: u64) -> u8 {
    if full <= limit {
        0
    } else if core <= limit {
        1
    } else {
        2
    }
}
