//! Role replies at the worker boundary (#730 C3).

use serde_json::Value;

pub struct RoleReply;

impl RoleReply {
    pub fn parse(_role: &str, _value: &Value) -> Result<Self, String> {
        Err("typed role replies are not implemented".into())
    }
}

pub fn generated() -> Vec<super::Generated> {
    Vec::new()
}
