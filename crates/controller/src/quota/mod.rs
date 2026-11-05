//! Q53 / #759: exception facts are sampled in Python, decisions belong to Rust.
pub mod decision;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Clone, Debug, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Link {
    pub response_status: Option<i64>,
    pub headers: BTreeMap<String, Value>,
    pub api_status: Option<i64>,
    pub api_code: Option<String>,
    pub judge: bool,
    pub judge_status: Option<i64>,
    pub judge_code: Option<String>,
    pub grade_judge: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Facts {
    pub now: String,
    pub role: String,
    pub links: Vec<Link>,
}

#[derive(Clone, Debug, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct QuotaEvent {
    pub event: String,
    pub role: String,
    pub facts: Facts,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

pub fn decide(_facts: &Facts) -> Option<u64> {
    None
}
pub fn reset(_link: &Link, _now: &str) -> Option<u64> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn quota_matches_python_shared_vectors() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let output = std::process::Command::new(root.join("workers/.venv/bin/python"))
            .arg(root.join("workers/tests/p5/quota_vectors.py"))
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let rows: Vec<Value> = serde_json::from_slice(&output.stdout).unwrap();
        assert!(rows.len() > 100);
        for row in rows {
            let facts: Facts = serde_json::from_value(row["facts"].clone()).unwrap();
            assert_eq!(serde_json::json!(decide(&facts)), row["expected"], "{row}");
        }
    }
}
