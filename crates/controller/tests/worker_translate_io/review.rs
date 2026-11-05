use super::super::super::*;
use io::Store;
pub const REPORT_RULE: &str="test-report-integrity";
pub const SCOPE_RULE: &str="request-scope";
pub(crate) fn criteria(
    store: &mut Store,
    snapshot: &tl3_store::Snapshot,
    node: tl3_kernel::NodeId,
) -> Result<Value> {
    let goals = crate::goals::load(snapshot, node)?.ok_or(Error::Invalid("review needs goals"))?;
    let node = node.0.to_string();
    let materials = store
        .worker_materials(&node)?
        .ok_or(Error::Invalid("review materials missing"))?;
    let mut rules = materials
        .get("standing_rules")
        .cloned()
        .unwrap_or(json!([]));
    let rules_list = rules
        .as_array_mut()
        .ok_or(Error::Invalid("standing rules must be a list"))?;
    rules_list.retain(|rule| rule["id"] != REPORT_RULE && rule["id"] != SCOPE_RULE);
    rules_list.push(json!({"id": REPORT_RULE, "description": "Protect test reports. Block code that writes directly to the test process's file descriptors, replaces its output streams, emits text shaped like test-report events, or reaches into pytest hooks, plugins, reporter or configuration internals. Ordinary output from the program under test is allowed. In-process report custody is accepted for slice one."}));
    rules_list.push(json!({"id": SCOPE_RULE, "description": "Keep edits within the supplied request goals. Block an unrelated edit, including inside a planned file, only when it serves none of the supplied goals. Necessary supporting code and tests are in scope. Imports, test fixtures, docstrings and type hints for new or changed code, formatting the change forces, and small helper refactors the change needs are in scope. When unsure whether an edit serves the goals, record a non-blocking finding instead of blocking. Cite this standing rule, identify the unrelated hunk and quote a whole changed diff line from that hunk on its own line in evidence, including its leading plus or minus, from scope_diff when supplied or otherwise diff. Explain why the hunk serves no supplied goal. The quoted changed content must have at least 8 non-whitespace characters; otherwise record a non-blocking finding. Do not invent goals or demand additional work."}));
    Ok(json!({"goals": goals, "standing_rules": rules}))
}
#[derive(serde::Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CitationKind {
    Goal,
    StandingRule,
}

#[derive(serde::Deserialize)]
pub(crate) struct Citation {
    pub kind: CitationKind,
    pub id: String,
}

#[derive(serde::Deserialize)]
pub(crate) struct BlockingFinding {
    pub citation: Citation,
    pub problem: String,
    pub evidence: String,
}

pub(crate) fn validate_reply(reply: &Value, inputs: &Value) -> Result<Vec<BlockingFinding>> {
    crate::reviewer::validate_findings(inputs, reply)
}

