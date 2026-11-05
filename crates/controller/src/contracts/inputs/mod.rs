//! Typed controller-to-worker wire contracts (#730, WR3).
use serde_json::Value;

/// Parse external materials into the role contract before serializing a request.
pub fn role_wire(_role: &str, _value: Value) -> Result<Value, serde_json::Error> {
    Ok(serde_json::json!({}))
}

pub fn generated() -> Vec<super::Generated> {
    vec![]
}

#[cfg(test)]
mod tests;
