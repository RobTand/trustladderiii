//! One reviewer validator for controller acceptance and worker repair.
use serde_json::Value;

pub fn validate(_inputs: &Value, _reply: &Value) -> crate::Result<()> {
    Err(crate::Error::Invalid("reviewer validator not implemented"))
}
