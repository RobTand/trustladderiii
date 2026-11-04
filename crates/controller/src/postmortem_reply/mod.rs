//! One postmortem validator for controller acceptance and in-worker repair (#782).

mod decision;
use decision::{Operation, Recommendation, Reply, decide};
use serde_json::Value;

pub(crate) use decision::MAX_RECOMMENDATIONS;
const SCHEMA_ERROR: &str = "worker reply breaks role schema";

/// Preserve the worker's semantic diagnostics, then enforce the full wire schema.
pub fn validate(reply: &Value) -> crate::Result<()> {
    recommendations(reply)?;
    crate::adapters::workers::protocol::validate_reply("postmortem", reply)
}

pub(crate) fn recommendations(reply: &Value) -> crate::Result<&Vec<Value>> {
    let items = reply["postmortem"]["recommendations"]
        .as_array()
        .ok_or(crate::Error::Invalid(
            "a postmortem needs its recommendations",
        ))?;
    decide(parse(items)?).map_err(|violation| crate::Error::Invalid(violation.message()))?;
    Ok(items)
}

fn parse(items: &[Value]) -> crate::Result<Reply> {
    if items.len() > MAX_RECOMMENDATIONS {
        return Ok(Reply::TooMany);
    }
    let mut parsed = [None; MAX_RECOMMENDATIONS];
    for (slot, item) in parsed.iter_mut().zip(items) {
        let operation = match item["operation"].as_str() {
            Some("add") => Operation::Add,
            Some("remove") => Operation::Remove,
            Some("replace") => Operation::Replace,
            _ => return Err(crate::Error::Invalid(SCHEMA_ERROR)),
        };
        let nonempty = |field: &str| {
            item[field]
                .as_str()
                .map(|text| !text.is_empty())
                .ok_or(crate::Error::Invalid(SCHEMA_ERROR))
        };
        *slot = Some(Recommendation {
            operation,
            before_nonempty: nonempty("before")?,
            after_nonempty: nonempty("after")?,
        });
    }
    Ok(Reply::Recommendations(parsed))
}
