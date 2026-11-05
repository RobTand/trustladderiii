//! Deterministic planner decisions shared by controller acceptance and check-plan.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum Request {
    Check { inputs: Value, reply: Value, #[serde(default)] facts: Vec<Fact> },
    Constrain { draft: Value, mismatch: Value, correction: Value, #[serde(default)] facts: Vec<Fact> },
    SchemaReply { reply: Value },
    Declarations,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Fact {
    pub read: Value,
    pub value: Value,
}

/// Pure request boundary. A missing fact returns the next ordered read.
pub fn check(_request: Request) -> Value {
    serde_json::json!({"kind":"stub"})
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn planner_schema_reference_proof_omissions() {
        let reply = json!({"kind":"leaf","checks":[{"kind":"demonstrating"}]});
        assert_eq!(check(Request::SchemaReply { reply }), json!({
            "reply":{"kind":"leaf","checks":[{"kind":"demonstrating","proof":{
                "subjects":[],"observer_support":[],"red_message":"undeclared assertion message"}}]},
            "missing":{"0":["observer_support","red_message","subjects"]}}));
    }

    #[test]
    fn planner_rejects_unknown_memory_before_reading_facts() {
        let output = check(Request::Check { inputs: json!({}), reply: json!({
            "kind":"cant_meet","files":[],"report":"no","memory_ids":["unknown"]}), facts: vec![] });
        assert_eq!(output, json!({"kind":"invalid","message":"reply cites a memory it was not given"}));
    }
}
