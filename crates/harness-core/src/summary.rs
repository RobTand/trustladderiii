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

#[cfg(verus_keep_ghost)]
use vstd::prelude::*;

#[cfg(not(verus_keep_ghost))]
mod runtime {
    use super::{
        display_priority, escaped_codepoint_bytes, fits_budget, mandatory_entry, retention_level,
        size_sum,
    };
    use schemars::JsonSchema;
    use serde::{Deserialize, Serialize};

    /// Python strings can contain isolated surrogates, which Rust `String` cannot.
    #[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
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
            let bytes = token.as_bytes();
            let mut index = usize::from(bytes.first() == Some(&b'-'));
            match bytes.get(index) {
                Some(b'0') => index += 1,
                Some(b'1'..=b'9') => {
                    index += 1;
                    while matches!(bytes.get(index), Some(b'0'..=b'9')) {
                        index += 1;
                    }
                }
                _ => return Err(BoundError::InvalidInput("invalid numeric atom")),
            }
            if bytes.get(index) == Some(&b'.') {
                index += 1;
                let start = index;
                while matches!(bytes.get(index), Some(b'0'..=b'9')) {
                    index += 1;
                }
                if index == start {
                    return Err(BoundError::InvalidInput("invalid numeric atom"));
                }
            }
            if matches!(bytes.get(index), Some(b'e' | b'E')) {
                index += 1;
                if matches!(bytes.get(index), Some(b'+' | b'-')) {
                    index += 1;
                }
                let start = index;
                while matches!(bytes.get(index), Some(b'0'..=b'9')) {
                    index += 1;
                }
                if index == start {
                    return Err(BoundError::InvalidInput("invalid numeric atom"));
                }
            }
            if index != bytes.len() {
                return Err(BoundError::InvalidInput("invalid numeric atom"));
            }
            Ok(Self(token.to_owned()))
        }

        pub fn as_str(&self) -> &str {
            &self.0
        }
    }

    impl SummaryValue {
        /// Parse Python JSON without losing object order or isolated surrogates.
        pub fn from_python_json(input: &str) -> Result<Self, BoundError> {
            let mut parser = Parser { input, position: 0 };
            let value = parser.value(0)?;
            parser.whitespace();
            if parser.position != input.len() {
                return Err(BoundError::InvalidInput("trailing JSON data"));
            }
            Ok(value)
        }

        pub fn field(&self, name: &str) -> Option<&Self> {
            let Self::Object(fields) = self else {
                return None;
            };
            fields
                .iter()
                .find(|(key, _)| *key == PythonText::from(name))
                .map(|(_, value)| value)
        }

        fn field_mut(&mut self, name: &str) -> Option<&mut Self> {
            let Self::Object(fields) = self else {
                return None;
            };
            fields
                .iter_mut()
                .find(|(key, _)| *key == PythonText::from(name))
                .map(|(_, value)| value)
        }

        fn set(&mut self, name: &str, value: Self) {
            if let Some(previous) = self.field_mut(name) {
                *previous = value;
            } else if let Self::Object(fields) = self {
                fields.push((PythonText::from(name), value));
            }
        }

        fn remove(&mut self, name: &str) {
            if let Self::Object(fields) = self {
                fields.retain(|(key, _)| *key != PythonText::from(name));
            }
        }

        fn is_text(&self, text: &str) -> bool {
            self == &Self::String(PythonText::from(text))
        }
    }

    pub trait SummarySerializer {
        fn serialize(&self, value: &SummaryValue) -> Result<Vec<u8>, BoundError>;
    }

    #[derive(Clone, Copy, Debug, Default)]
    pub struct PythonEnvelope;

    impl SummarySerializer for PythonEnvelope {
        fn serialize(&self, value: &SummaryValue) -> Result<Vec<u8>, BoundError> {
            let expected = encoded_size(value, 0)?;
            let mut output = Vec::new();
            encode(value, &mut output, 0)?;
            if u64::try_from(output.len()).ok() != Some(expected) {
                return Err(BoundError::InvalidInput("serializer size model disagrees"));
            }
            Ok(output)
        }
    }

    // The encoder and this arithmetic model share the proven atom widths.
    // proptest compares their composed output against Python's exact bytes.
    fn add_size(total: &mut u64, bytes: u64) -> Result<(), BoundError> {
        if !fits_budget(*total, bytes, u64::MAX) {
            return Err(BoundError::SizeOverflow);
        }
        *total = size_sum(*total, bytes);
        Ok(())
    }

    fn text_size(text: &PythonText) -> Result<u64, BoundError> {
        let mut size = 2;
        for &point in text.codepoints() {
            add_size(&mut size, escaped_codepoint_bytes(point))?;
        }
        Ok(size)
    }

    fn encoded_size(value: &SummaryValue, depth: usize) -> Result<u64, BoundError> {
        if depth > MAX_JSON_DEPTH {
            return Err(BoundError::InvalidInput(
                "summary nesting exceeds JSON transport depth",
            ));
        }
        match value {
            SummaryValue::Null | SummaryValue::Bool(true) => Ok(4),
            SummaryValue::Bool(false) => Ok(5),
            SummaryValue::Number(number) => {
                u64::try_from(number.as_str().len()).map_err(|_| BoundError::SizeOverflow)
            }
            SummaryValue::String(text) => text_size(text),
            SummaryValue::Array(items) => {
                let mut size = 2;
                for (index, item) in items.iter().enumerate() {
                    if index != 0 {
                        add_size(&mut size, 2)?;
                    }
                    add_size(&mut size, encoded_size(item, depth + 1)?)?;
                }
                Ok(size)
            }
            SummaryValue::Object(fields) => {
                let mut size = 2;
                for (index, (key, item)) in fields.iter().enumerate() {
                    if index != 0 {
                        add_size(&mut size, 2)?;
                    }
                    add_size(&mut size, text_size(key)?)?;
                    add_size(&mut size, 2)?;
                    add_size(&mut size, encoded_size(item, depth + 1)?)?;
                }
                Ok(size)
            }
        }
    }

    const MAX_JSON_DEPTH: usize = 128;

    fn hex4(word: u32, output: &mut Vec<u8>) {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        output.extend_from_slice(b"\\u");
        for shift in [12, 8, 4, 0] {
            output.push(HEX[((word >> shift) & 15) as usize]);
        }
    }

    /// Used by the envelope and the compact selector serializer.
    pub(crate) fn encode_text(text: &PythonText, output: &mut Vec<u8>) {
        output.push(b'"');
        for &point in text.codepoints() {
            match escaped_codepoint_bytes(point) {
                1 => output.push(point as u8),
                2 => output.extend_from_slice(match point {
                    8 => b"\\b",
                    9 => b"\\t",
                    10 => b"\\n",
                    12 => b"\\f",
                    13 => b"\\r",
                    34 => b"\\\"",
                    92 => b"\\\\",
                    _ => unreachable!("escape width and encoder agree"),
                }),
                6 => hex4(point, output),
                12 => {
                    let supplementary = point - 0x10000;
                    hex4(0xd800 + (supplementary >> 10), output);
                    hex4(0xdc00 + (supplementary & 0x3ff), output);
                }
                _ => unreachable!("Python escape widths are 1, 2, 6, or 12"),
            }
        }
        output.push(b'"');
    }

    fn encode(value: &SummaryValue, output: &mut Vec<u8>, depth: usize) -> Result<(), BoundError> {
        if depth > MAX_JSON_DEPTH {
            return Err(BoundError::InvalidInput(
                "summary nesting exceeds JSON transport depth",
            ));
        }
        match value {
            SummaryValue::Null => output.extend_from_slice(b"null"),
            SummaryValue::Bool(true) => output.extend_from_slice(b"true"),
            SummaryValue::Bool(false) => output.extend_from_slice(b"false"),
            SummaryValue::Number(number) => output.extend_from_slice(number.as_str().as_bytes()),
            SummaryValue::String(text) => encode_text(text, output),
            SummaryValue::Array(items) => {
                output.push(b'[');
                for (index, item) in items.iter().enumerate() {
                    if index != 0 {
                        output.extend_from_slice(b", ");
                    }
                    encode(item, output, depth + 1)?;
                }
                output.push(b']');
            }
            SummaryValue::Object(fields) => {
                output.push(b'{');
                for (index, (key, item)) in fields.iter().enumerate() {
                    if index != 0 {
                        output.extend_from_slice(b", ");
                    }
                    encode_text(key, output);
                    output.extend_from_slice(b": ");
                    encode(item, output, depth + 1)?;
                }
                output.push(b'}');
            }
        }
        Ok(())
    }

    struct Parser<'a> {
        input: &'a str,
        position: usize,
    }

    impl Parser<'_> {
        fn peek(&self) -> Option<u8> {
            self.input.as_bytes().get(self.position).copied()
        }

        fn whitespace(&mut self) {
            while matches!(self.peek(), Some(b' ' | b'\n' | b'\r' | b'\t')) {
                self.position += 1;
            }
        }

        fn take(&mut self, expected: u8) -> Result<(), BoundError> {
            self.whitespace();
            if self.peek() != Some(expected) {
                return Err(BoundError::InvalidInput("invalid summary JSON syntax"));
            }
            self.position += 1;
            Ok(())
        }

        fn value(&mut self, depth: usize) -> Result<SummaryValue, BoundError> {
            if depth > MAX_JSON_DEPTH {
                return Err(BoundError::InvalidInput(
                    "summary nesting exceeds JSON transport depth",
                ));
            }
            self.whitespace();
            match self.peek() {
                Some(b'"') => self.text().map(SummaryValue::String),
                Some(b'[') => {
                    self.position += 1;
                    self.whitespace();
                    let mut values = Vec::new();
                    if self.peek() != Some(b']') {
                        loop {
                            values.push(self.value(depth + 1)?);
                            self.whitespace();
                            if self.peek() != Some(b',') {
                                break;
                            }
                            self.position += 1;
                        }
                    }
                    self.take(b']')?;
                    Ok(SummaryValue::Array(values))
                }
                Some(b'{') => {
                    self.position += 1;
                    self.whitespace();
                    let mut fields = Vec::new();
                    let mut keys = std::collections::HashSet::new();
                    if self.peek() != Some(b'}') {
                        loop {
                            self.whitespace();
                            let key = self.text()?;
                            if !keys.insert(key.clone()) {
                                return Err(BoundError::InvalidInput("duplicate summary JSON key"));
                            }
                            self.take(b':')?;
                            fields.push((key, self.value(depth + 1)?));
                            self.whitespace();
                            if self.peek() != Some(b',') {
                                break;
                            }
                            self.position += 1;
                        }
                    }
                    self.take(b'}')?;
                    Ok(SummaryValue::Object(fields))
                }
                Some(b'-' | b'0'..=b'9') => {
                    let start = self.position;
                    while matches!(
                        self.peek(),
                        Some(b'-' | b'+' | b'0'..=b'9' | b'.' | b'e' | b'E')
                    ) {
                        self.position += 1;
                    }
                    JsonNumber::parse(&self.input[start..self.position]).map(SummaryValue::Number)
                }
                _ => {
                    for (token, value) in [
                        ("null", SummaryValue::Null),
                        ("true", SummaryValue::Bool(true)),
                        ("false", SummaryValue::Bool(false)),
                    ] {
                        if self.input[self.position..].starts_with(token) {
                            self.position += token.len();
                            return Ok(value);
                        }
                    }
                    Err(BoundError::InvalidInput("invalid summary JSON value"))
                }
            }
        }

        fn word(&mut self) -> Result<u32, BoundError> {
            let end = self
                .position
                .checked_add(4)
                .ok_or(BoundError::SizeOverflow)?;
            let text = self
                .input
                .get(self.position..end)
                .ok_or(BoundError::InvalidInput("invalid JSON Unicode escape"))?;
            let value = u32::from_str_radix(text, 16)
                .map_err(|_| BoundError::InvalidInput("invalid JSON Unicode escape"))?;
            self.position = end;
            Ok(value)
        }

        fn text(&mut self) -> Result<PythonText, BoundError> {
            self.take(b'"')?;
            let mut points = Vec::new();
            loop {
                let byte = self
                    .peek()
                    .ok_or(BoundError::InvalidInput("unterminated JSON string"))?;
                self.position += 1;
                match byte {
                    b'"' => return PythonText::try_from(points).map_err(BoundError::InvalidInput),
                    b'\\' => {
                        let escape = self
                            .peek()
                            .ok_or(BoundError::InvalidInput("unterminated JSON escape"))?;
                        self.position += 1;
                        points.push(match escape {
                            b'"' => 34,
                            b'\\' => 92,
                            b'/' => 47,
                            b'b' => 8,
                            b'f' => 12,
                            b'n' => 10,
                            b'r' => 13,
                            b't' => 9,
                            b'u' => {
                                let high = self.word()?;
                                if (0xd800..=0xdbff).contains(&high)
                                    && self.input[self.position..].starts_with("\\u")
                                {
                                    let saved = self.position;
                                    self.position += 2;
                                    let low = self.word()?;
                                    if (0xdc00..=0xdfff).contains(&low) {
                                        0x10000 + ((high - 0xd800) << 10) + low - 0xdc00
                                    } else {
                                        self.position = saved;
                                        high
                                    }
                                } else {
                                    high
                                }
                            }
                            _ => return Err(BoundError::InvalidInput("invalid JSON escape")),
                        });
                    }
                    0..=31 => {
                        return Err(BoundError::InvalidInput("unescaped JSON control character"));
                    }
                    32..=127 => points.push(u32::from(byte)),
                    _ => {
                        self.position -= 1;
                        let character = self.input[self.position..]
                            .chars()
                            .next()
                            .ok_or(BoundError::InvalidInput("invalid Unicode string"))?;
                        self.position += character.len_utf8();
                        points.push(u32::from(character));
                    }
                }
            }
        }
    }

    #[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
    #[serde(tag = "kind", content = "value", rename_all = "snake_case")]
    pub enum PathPart {
        Field(String),
        Index(usize),
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
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

    #[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
    #[serde(deny_unknown_fields)]
    pub struct Dropped {
        pub path: Vec<PathPart>,
        pub kind: DroppedKind,
        pub count: usize,
    }

    #[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
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
        input: SummaryValue,
        limit: usize,
        serializer: &S,
    ) -> Result<BoundedSummary, BoundError> {
        let original_bytes = serializer.serialize(&input)?.len();
        if fits_size(original_bytes, limit)? {
            return Ok(BoundedSummary {
                value: input,
                report: DropReport::default(),
                serialized_bytes: original_bytes,
            });
        }
        let core = mandatory_core(&input, serializer)?;
        let core_bytes = serializer.serialize(&core)?.len();
        let level = retention_level(
            u64::try_from(original_bytes.max(core_bytes)).map_err(|_| BoundError::SizeOverflow)?,
            u64::try_from(core_bytes).map_err(|_| BoundError::SizeOverflow)?,
            u64::try_from(limit).map_err(|_| BoundError::SizeOverflow)?,
        );
        if level == 2 {
            let mut value = diagnostic(&input);
            // A grouped fallback can itself exceed 16 KiB through repeated receipts
            // (#818's 48-group incident). The diagnostic has no citable proof; omit
            // all optional receipt display together, preserving origin and totals.
            if !fits_size(serializer.serialize(&value)?.len(), limit)? {
                value.remove("receipts");
            }
            let serialized_bytes = serializer.serialize(&value)?.len();
            if !fits_size(serialized_bytes, limit)? {
                return Err(BoundError::DiagnosticTooLarge {
                    required: serialized_bytes,
                    limit,
                });
            }
            return Ok(BoundedSummary {
                value,
                report: DropReport {
                    diagnostic: true,
                    dropped: vec![],
                },
                serialized_bytes,
            });
        }

        let mut value = input;
        let mut report = DropReport::default();
        mark_notices(&mut value);
        let mut removals = Vec::new();
        collect_removals(&value, &mut Vec::new(), &mut removals);
        removals.sort_by_key(|removal| (removal.rank, removal.order));
        // Search prefixes of the same total removal order. Rebuilding each
        // candidate in one traversal avoids quadratic serialization/removal for
        // a raw action near its cap, within the shared transport's timeout.
        let all = candidate_prefix(&value, &removals, serializer)?;
        let all_bytes = serializer.serialize(&all)?.len();
        let (retained, removed) = if !fits_size(all_bytes, limit)? {
            (core, removals.len())
        } else {
            let mut low = 0;
            let mut high = removals.len();
            while low < high {
                let middle = low + (high - low) / 2;
                let candidate = candidate_prefix(&value, &removals[..middle], serializer)?;
                if fits_size(serializer.serialize(&candidate)?.len(), limit)? {
                    high = middle;
                } else {
                    low = middle + 1;
                }
            }
            (candidate_prefix(&value, &removals[..low], serializer)?, low)
        };
        for removal in &removals[..removed] {
            if removal.kind != DroppedKind::SelectorCompaction
                || at_path_read(&retained, &removal.path)
                    .and_then(|summary| summary.field("selectors"))
                    != at_path_read(&value, &removal.path)
                        .and_then(|summary| summary.field("selectors"))
            {
                report.dropped.push(Dropped {
                    path: removal.path.clone(),
                    kind: removal.kind,
                    count: 1,
                });
            }
        }
        let serialized_bytes = serializer.serialize(&retained)?.len();
        // The upper endpoint was measured to fit, or this is the measured core.
        if !fits_size(serialized_bytes, limit)? {
            return Err(BoundError::InvalidInput(
                "serializer changed between measurements",
            ));
        }
        Ok(BoundedSummary {
            value: retained,
            report,
            serialized_bytes,
        })
    }

    fn fits_size(required: usize, limit: usize) -> Result<bool, BoundError> {
        Ok(fits_budget(
            u64::try_from(required).map_err(|_| BoundError::SizeOverflow)?,
            0,
            u64::try_from(limit).map_err(|_| BoundError::SizeOverflow)?,
        ))
    }

    const TRUNCATION_NOTICE: &str =
        "Truncated: selectors were compacted or entries were cut. Totals include all results.";

    fn text(value: &str) -> SummaryValue {
        SummaryValue::String(PythonText::from(value))
    }

    fn object(fields: &[(&str, SummaryValue)]) -> SummaryValue {
        SummaryValue::Object(
            fields
                .iter()
                .map(|(key, value)| (PythonText::from(*key), value.clone()))
                .collect(),
        )
    }

    fn diagnostic(input: &SummaryValue) -> SummaryValue {
        let execution = object(&[
            ("outcome", text("transport_failure")),
            ("phase", text("summary")),
            ("termination_reason", text("identity_exceeds_summary_limit")),
            ("test_started", SummaryValue::Bool(false)),
        ]);
        let mut result = object(&[
            ("status", text("infrastructure_error")),
            (
                "message",
                text(
                    "Test execution transport_failure in summary: identity_exceeds_summary_limit. No test verdict is available; inspect this action, not fleet availability.",
                ),
            ),
            ("tests", SummaryValue::Array(vec![])),
            (
                "totals",
                input
                    .field("totals")
                    .cloned()
                    .unwrap_or_else(|| object(&[])),
            ),
            (
                "note",
                text(
                    "Truncated: summary identity exceeds the envelope limit. No proof is available.",
                ),
            ),
        ]);
        if let Some(origin) = input
            .field("origin")
            .filter(|origin| **origin != SummaryValue::Null)
        {
            result.set("origin", origin.clone());
        }
        result.set(
            "execution",
            input.field("execution").cloned().unwrap_or(execution),
        );
        for name in ["action_key", "receipts"] {
            if let Some(value) = input.field(name) {
                result.set(name, value.clone());
            }
        }
        result
    }

    fn is_red(summary: &SummaryValue, entry: &SummaryValue) -> bool {
        let failed = entry
            .field("outcome")
            .is_some_and(|value| value.is_text("failed"));
        let error = entry
            .field("outcome")
            .is_some_and(|value| value.is_text("error"));
        let proof = summary.field("proof");
        let observer = proof
            .and_then(|proof| proof.field("mode"))
            .is_some_and(|value| value.is_text("head-observer"));
        let forced = summary.field("forcing").is_some() && proof.is_some();
        let symptom = summary
            .field("declared_symptom")
            .and_then(|declaration| declaration.field("node"));
        let planned_error = match (
            proof.and_then(|proof| proof.field("observer_sha256")),
            entry.field("test"),
        ) {
            (Some(SummaryValue::Object(observers)), Some(SummaryValue::String(node))) => observers
                .iter()
                .any(|(selected, _)| observer_selects(selected, node)),
            _ => false,
        };
        if mandatory_entry(
            observer,
            forced,
            failed,
            error,
            planned_error,
            symptom.is_some() && symptom == entry.field("test"),
        ) {
            return true;
        }
        // Worker group envelopes repeat their display entries at the root.
        // Protect the same RED in both views, rather than retaining its proof
        // key while losing the actual root-level result a consumer reads.
        if failed || error {
            if let Some(SummaryValue::Array(groups)) = summary.field("groups") {
                return groups.iter().any(|group| {
                    matches!(group.field("tests"), Some(SummaryValue::Array(tests)) if tests.contains(entry))
                        && is_red(group, entry)
                });
            }
        }
        false
    }

    // SEAM(node-id): use #824's shared selector predicate when its port lands.
    // This is selection containment for protecting planned RED, not validation.
    fn observer_selects(selected: &PythonText, node: &PythonText) -> bool {
        let requested = selected.codepoints();
        let collected = node.codepoints();
        requested == collected
            || !requested.windows(2).any(|pair| pair == [58, 58])
                && collected.starts_with(&[requested, &[58, 58]].concat())
            || !requested.contains(&91)
                && collected.starts_with(&[requested, &[91]].concat())
                && collected.last() == Some(&93)
    }

    fn compact_selectors(
        summary: &mut SummaryValue,
        serializer: &impl SummarySerializer,
    ) -> Result<(), BoundError> {
        let Some(SummaryValue::Array(selectors)) = summary.field("selectors") else {
            return Ok(());
        };
        let selected = selectors
            .iter()
            .map(|item| match item {
                SummaryValue::String(value) => Ok(value.clone()),
                _ => Err(BoundError::InvalidInput(
                    "selectors must contain Python strings",
                )),
            })
            .collect::<Result<Vec<_>, _>>()?;
        let identity = crate::identity::selector_identity(&selected);
        let compact = object(&[
            ("schema", text(&identity.schema)),
            (
                "count",
                SummaryValue::Number(JsonNumber(identity.count.to_string())),
            ),
            ("sha256", text(&identity.sha256)),
            (
                "prefix",
                SummaryValue::Array(
                    identity
                        .prefix
                        .into_iter()
                        .map(SummaryValue::String)
                        .collect(),
                ),
            ),
        ]);
        if serializer.serialize(&compact)?.len()
            < serializer
                .serialize(&SummaryValue::Array(selectors.clone()))?
                .len()
        {
            summary.set("selectors", compact);
        }
        Ok(())
    }

    fn mandatory_core(
        input: &SummaryValue,
        serializer: &impl SummarySerializer,
    ) -> Result<SummaryValue, BoundError> {
        let mut core = input.clone();
        compact_selectors(&mut core, serializer)?;
        if let Some(SummaryValue::Array(entries)) = input.field("tests") {
            core.set(
                "tests",
                SummaryValue::Array(
                    entries
                        .iter()
                        .filter(|entry| is_red(input, entry))
                        .cloned()
                        .collect(),
                ),
            );
        }
        if core.field("note").is_some() {
            core.set("note", text(""));
        }
        if let Some(SummaryValue::Array(runs)) = core.field_mut("runs") {
            for run in runs {
                if run.field("failed").is_some() {
                    run.set("failed", SummaryValue::Array(vec![]));
                }
            }
        }
        if let Some(SummaryValue::Array(groups)) = input.field("groups") {
            core.set(
                "groups",
                SummaryValue::Array(
                    groups
                        .iter()
                        .map(|group| mandatory_core(group, serializer))
                        .collect::<Result<_, _>>()?,
                ),
            );
        }
        if let Some(base) = input.field("base") {
            core.set("base", mandatory_core(base, serializer)?);
        }
        Ok(core)
    }

    fn mark_notices(summary: &mut SummaryValue) {
        summary.set("note", text(TRUNCATION_NOTICE));
        if let Some(SummaryValue::Array(groups)) = summary.field_mut("groups") {
            for group in groups {
                mark_notices(group);
            }
        }
        if let Some(base) = summary.field_mut("base") {
            mark_notices(base);
        }
    }

    struct Removal {
        path: Vec<PathPart>,
        kind: DroppedKind,
        rank: u8,
        order: usize,
    }

    fn collect_removals(
        summary: &SummaryValue,
        path: &mut Vec<PathPart>,
        removals: &mut Vec<Removal>,
    ) {
        if matches!(summary.field("selectors"), Some(SummaryValue::Array(_))) {
            removals.push(Removal {
                path: path.clone(),
                kind: DroppedKind::SelectorCompaction,
                rank: 0,
                order: removals.len(),
            });
        }
        if let Some(SummaryValue::Array(entries)) = summary.field("tests") {
            for (index, entry) in entries.iter().enumerate().rev() {
                let outcome = entry.field("outcome");
                let rank = display_priority(
                    outcome.is_some_and(|value| value.is_text("passed")),
                    is_red(summary, entry),
                );
                if rank == 255 {
                    continue;
                }
                let kind = if rank == 1 {
                    DroppedKind::PassingEntry
                } else if outcome.is_some_and(|value| value.is_text("error")) {
                    DroppedKind::ErrorEntry
                } else {
                    DroppedKind::FailureEntry
                };
                let mut item_path = path.clone();
                item_path.extend([PathPart::Field("tests".to_owned()), PathPart::Index(index)]);
                removals.push(Removal {
                    path: item_path,
                    kind,
                    rank,
                    order: removals.len(),
                });
            }
        }
        if let Some(SummaryValue::Array(runs)) = summary.field("runs") {
            for (run, entry) in runs.iter().enumerate().rev() {
                if let Some(SummaryValue::Array(failed)) = entry.field("failed") {
                    for index in (0..failed.len()).rev() {
                        let mut item_path = path.clone();
                        item_path.extend([
                            PathPart::Field("runs".to_owned()),
                            PathPart::Index(run),
                            PathPart::Field("failed".to_owned()),
                            PathPart::Index(index),
                        ]);
                        removals.push(Removal {
                            path: item_path,
                            kind: DroppedKind::RunFailure,
                            rank: 4,
                            order: removals.len(),
                        });
                    }
                }
            }
        }
        if let Some(SummaryValue::Array(groups)) = summary.field("groups") {
            for (index, group) in groups.iter().enumerate() {
                path.extend([PathPart::Field("groups".to_owned()), PathPart::Index(index)]);
                collect_removals(group, path, removals);
                path.truncate(path.len() - 2);
            }
        }
        if let Some(base) = summary.field("base") {
            path.push(PathPart::Field("base".to_owned()));
            collect_removals(base, path, removals);
            path.pop();
        }
    }

    fn at_path_read<'a>(
        mut value: &'a SummaryValue,
        path: &[PathPart],
    ) -> Option<&'a SummaryValue> {
        for part in path {
            value = match part {
                PathPart::Field(name) => value.field(name)?,
                PathPart::Index(index) => match value {
                    SummaryValue::Array(items) => items.get(*index)?,
                    _ => return None,
                },
            };
        }
        Some(value)
    }

    fn candidate_prefix(
        input: &SummaryValue,
        removals: &[Removal],
        serializer: &impl SummarySerializer,
    ) -> Result<SummaryValue, BoundError> {
        use std::collections::HashSet;
        let drops = removals
            .iter()
            .filter(|item| item.kind != DroppedKind::SelectorCompaction)
            .map(|item| item.path.clone())
            .collect::<HashSet<_>>();
        let compact = removals
            .iter()
            .filter(|item| item.kind == DroppedKind::SelectorCompaction)
            .map(|item| item.path.clone())
            .collect::<HashSet<_>>();
        fn walk(
            summary: &mut SummaryValue,
            path: &mut Vec<PathPart>,
            drops: &HashSet<Vec<PathPart>>,
            compact: &HashSet<Vec<PathPart>>,
            serializer: &impl SummarySerializer,
        ) -> Result<(), BoundError> {
            if compact.contains(path) {
                compact_selectors(summary, serializer)?;
            }
            if let Some(SummaryValue::Array(entries)) = summary.field_mut("tests") {
                path.push(PathPart::Field("tests".into()));
                let mut index = 0;
                entries.retain(|_| {
                    path.push(PathPart::Index(index));
                    index += 1;
                    let keep = !drops.contains(path);
                    path.pop();
                    keep
                });
                path.pop();
            }
            if let Some(SummaryValue::Array(runs)) = summary.field_mut("runs") {
                path.push(PathPart::Field("runs".into()));
                for (run_index, run) in runs.iter_mut().enumerate() {
                    path.push(PathPart::Index(run_index));
                    path.push(PathPart::Field("failed".into()));
                    if let Some(SummaryValue::Array(failed)) = run.field_mut("failed") {
                        let mut index = 0;
                        failed.retain(|_| {
                            path.push(PathPart::Index(index));
                            index += 1;
                            let keep = !drops.contains(path);
                            path.pop();
                            keep
                        });
                    }
                    path.truncate(path.len() - 2);
                }
                path.pop();
            }
            if let Some(SummaryValue::Array(groups)) = summary.field_mut("groups") {
                path.push(PathPart::Field("groups".into()));
                for (index, group) in groups.iter_mut().enumerate() {
                    path.push(PathPart::Index(index));
                    walk(group, path, drops, compact, serializer)?;
                    path.pop();
                }
                path.pop();
            }
            if let Some(base) = summary.field_mut("base") {
                path.push(PathPart::Field("base".into()));
                walk(base, path, drops, compact, serializer)?;
                path.pop();
            }
            Ok(())
        }
        let mut value = input.clone();
        walk(&mut value, &mut Vec::new(), &drops, &compact, serializer)?;
        Ok(value)
    }
}

#[cfg(not(verus_keep_ghost))]
pub(crate) use runtime::encode_text;
#[cfg(not(verus_keep_ghost))]
pub use runtime::{
    BoundError, BoundedSummary, DropReport, Dropped, DroppedKind, JsonNumber, PathPart,
    PythonEnvelope, PythonText, SummarySerializer, SummaryValue, bounded,
};

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

/// #823: the total display priority removes passing entries first and never RED.
/// Traversal order breaks ties, so every removable entry has a unique position.
#[cfg_attr(
    verus_keep_ghost,
    vstd::prelude::verus_spec(rank => ensures
        (rank == 255) == red,
        (!red && passing) ==> rank == 1,
        (!red && !passing) ==> rank == 2,
        rank == 1 || rank == 2 || rank == 255,
    )
)]
pub fn display_priority(passing: bool, red: bool) -> u8 {
    if red {
        255
    } else if passing {
        1
    } else {
        2
    }
}

/// #823: planned assertion RED, planned observer errors and declared symptom RED
/// are mandatory whenever the original typed facts identify them as such.
#[cfg_attr(
    verus_keep_ghost,
    vstd::prelude::verus_spec(required => ensures
        required == (failed && (observer || forced)
            || error && ((observer || forced) && planned || symptom)),
        failed && (observer || forced) ==> required,
        error && planned && (observer || forced) ==> required,
        error && symptom ==> required,
        !failed && !error ==> !required,
    )
)]
pub fn mandatory_entry(
    observer: bool,
    forced: bool,
    failed: bool,
    error: bool,
    planned: bool,
    symptom: bool,
) -> bool {
    failed && (observer || forced) || error && ((observer || forced) && planned || symptom)
}

/// #823: every serializer atom and delimiter is accumulated without wraparound.
#[cfg_attr(
    verus_keep_ghost,
    vstd::prelude::verus_spec(total => requires
        (left as int) + (right as int) <= u64::MAX as int,
        ensures total as int == (left as int) + (right as int),
    )
)]
pub fn size_sum(left: u64, right: u64) -> u64 {
    left + right
}
