//! #823: bounded strict transport for the worker's shared harness decisions.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::io::{Read, Write};
use tl3_harness_core::{identity, summary};

#[derive(Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum SummaryRequest {
    Bounded { summary_json: String, limit: usize },
}

#[derive(Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum IdentityRequest {
    Identity {
        selectors: Vec<Vec<u32>>,
    },
    Matches {
        selectors: Vec<Vec<u32>>,
        candidate: Candidate,
    },
}

#[derive(Serialize)]
struct Response<T> {
    result: T,
}

#[derive(Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SummaryResult {
    summary_json: String,
    report: summary::DropReport,
}

pub fn wire_schema() -> serde_json::Value {
    serde_json::json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "requests": { "summary": schemars::schema_for!(SummaryRequest), "identity": schemars::schema_for!(IdentityRequest) },
        "responses": {
            "bounded": schemars::schema_for!(SummaryResult),
            "identity": schemars::schema_for!(identity::SelectorIdentity),
            "matches": schemars::schema_for!(bool),
        },
    })
}

// Explicit code points prevent JSON decoding from merging a Python surrogate pair.
#[derive(Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Candidate {
    Null,
    Boolean { value: bool },
    Number { json: String },
    Text { codepoints: Vec<u32> },
    Array { items: Vec<Candidate> },
    Object { fields: Vec<(String, Candidate)> },
    Invalid,
}

impl Candidate {
    fn value(self) -> Option<summary::SummaryValue> {
        use summary::{JsonNumber, PythonText, SummaryValue};
        Some(match self {
            Self::Null => SummaryValue::Null,
            Self::Boolean { value } => SummaryValue::Bool(value),
            Self::Number { json } => SummaryValue::Number(JsonNumber::parse(&json).ok()?),
            Self::Text { codepoints } => {
                SummaryValue::String(PythonText::try_from(codepoints).ok()?)
            }
            Self::Array { items } => {
                SummaryValue::Array(items.into_iter().map(Self::value).collect::<Option<_>>()?)
            }
            Self::Object { fields } => {
                let mut names = std::collections::BTreeSet::new();
                if fields.iter().any(|(name, _)| !names.insert(name.clone())) {
                    return None;
                }
                SummaryValue::Object(
                    fields
                        .into_iter()
                        .map(|(name, value)| {
                            Some((PythonText::from(name.as_str()), value.value()?))
                        })
                        .collect::<Option<_>>()?,
                )
            }
            Self::Invalid => return None,
        })
    }
}

fn candidate(value: summary::SummaryValue) -> identity::Selection {
    use summary::SummaryValue;
    if let SummaryValue::Array(items) = &value {
        return match items
            .iter()
            .map(|item| match item {
                SummaryValue::String(text) => Some(text.clone()),
                _ => None,
            })
            .collect::<Option<Vec<_>>>()
        {
            Some(values) => identity::Selection::Full(values),
            None => identity::Selection::Invalid,
        };
    }
    let SummaryValue::Object(fields) = &value else {
        return identity::Selection::Invalid;
    };
    if fields.len() != 4 {
        return identity::Selection::Invalid;
    }
    let scalar = |name| match value.field(name) {
        Some(SummaryValue::String(text)) => text
            .codepoints()
            .iter()
            .copied()
            .map(char::from_u32)
            .collect::<Option<String>>(),
        _ => None,
    };
    let count = match value.field("count") {
        Some(SummaryValue::Number(number)) => number.as_str().parse::<usize>().ok(),
        _ => None,
    };
    let prefix = match value.field("prefix") {
        Some(SummaryValue::Array(items)) => items
            .iter()
            .map(|item| match item {
                SummaryValue::String(text) => Some(text.clone()),
                _ => None,
            })
            .collect::<Option<Vec<_>>>(),
        _ => None,
    };
    match (scalar("schema"), count, scalar("sha256"), prefix) {
        (Some(schema), Some(count), Some(sha256), Some(prefix)) => {
            identity::Selection::Identity(identity::SelectorIdentity {
                schema,
                count,
                sha256,
                prefix,
            })
        }
        _ => identity::Selection::Invalid,
    }
}

pub fn decide(kind: &str, document: serde_json::Value) -> crate::Result<serde_json::Value> {
    match kind {
        "summary" => {
            let request: SummaryRequest = serde_json::from_value(document)
                .map_err(|_| crate::Error::Invalid("invalid summary facts"))?;
            let SummaryRequest::Bounded {
                summary_json,
                limit,
            } = request;
            let input = summary::SummaryValue::from_python_json(&summary_json)
                .map_err(|_| crate::Error::Invalid("invalid summary document"))?;
            let bounded = summary::bounded(input, limit, &summary::PythonEnvelope)
                .map_err(|_| crate::Error::Invalid("summary cannot fit its envelope"))?;
            let bytes =
                summary::SummarySerializer::serialize(&summary::PythonEnvelope, &bounded.value)
                    .map_err(|_| crate::Error::Invalid("invalid summary document"))?;
            Ok(serde_json::to_value(Response {
                result: SummaryResult {
                    summary_json: String::from_utf8(bytes)
                        .map_err(|_| crate::Error::Invalid("invalid summary encoding"))?,
                    report: bounded.report,
                },
            })?)
        }
        "identity" => {
            let request: IdentityRequest = serde_json::from_value(document)
                .map_err(|_| crate::Error::Invalid("invalid selector identity facts"))?;
            let (selectors, comparison) = match request {
                IdentityRequest::Identity { selectors } => (selectors, None),
                IdentityRequest::Matches {
                    selectors,
                    candidate,
                } => (selectors, Some(candidate)),
            };
            let selectors = selectors
                .into_iter()
                .map(summary::PythonText::try_from)
                .collect::<Result<Vec<_>, _>>()
                .map_err(crate::Error::Invalid)?;
            if let Some(source) = comparison {
                Ok(serde_json::to_value(Response {
                    result: identity::selection_matches(
                        &source
                            .value()
                            .map(candidate)
                            .unwrap_or(identity::Selection::Invalid),
                        &selectors,
                    ),
                })?)
            } else {
                Ok(serde_json::to_value(Response {
                    result: identity::selector_identity(&selectors),
                })?)
            }
        }
        _ => Err(crate::Error::Invalid("unsupported harness decision")),
    }
}

pub fn command(kind: Option<&str>) -> crate::Result<()> {
    if kind == Some("schema") {
        let mut output = std::io::stdout().lock();
        serde_json::to_writer_pretty(&mut output, &wire_schema())?;
        writeln!(output)?;
        return Ok(());
    }
    let Some(kind @ ("summary" | "identity")) = kind else {
        return Err(crate::Error::Invalid(
            "expected check-harness summary or identity",
        ));
    };
    let mut bytes = Vec::new();
    std::io::stdin()
        .take(32 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > 32 * 1024 * 1024 {
        return Err(crate::Error::Invalid("harness request is too large"));
    }
    let document = crate::adapters::workers::json::parse(&bytes)
        .map_err(|_| crate::Error::Invalid("invalid check-harness JSON"))?;
    let result = decide(kind, document)?;
    serde_json::to_writer(std::io::stdout().lock(), &result)?;
    std::io::stdout().write_all(b"\n")?;
    Ok(())
}
