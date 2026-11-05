//! Maker decisions shared by controller acceptance and the live repair turn.
use serde_json::Value;

pub fn validate(_inputs: &Value, _reply: &Value) -> Result<(), String> {
    Ok(())
}

pub fn checking_handoff(_inputs: &Value, _head: &str, _tree: &str) -> Value {
    Value::Null
}
