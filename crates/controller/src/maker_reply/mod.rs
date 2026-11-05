//! Maker repair decisions; orchestration, budget and evidence sampling stay in Python.
pub mod decision;
mod proof;

use crate::adapters::workers::reproducing;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Default, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
enum Operation {
    #[default]
    Check,
    Handoff,
}

/// This command reads only these facts from the full role documents. WR2/WR3 own
/// the complete role contracts; their additional fields deliberately pass through.
#[derive(Deserialize, JsonSchema)]
struct Request {
    #[serde(default)]
    operation: Operation,
    inputs: Inputs,
    reply: Option<Reply>,
    head: Option<String>,
    tree: Option<String>,
}

#[derive(Default, Deserialize, JsonSchema)]
struct Inputs {
    #[serde(default)]
    goals: Vec<Goal>,
    #[serde(default)]
    current_review_findings: Vec<Finding>,
    check_base_sha: Option<String>,
    #[serde(default)]
    plan: Plan,
}

#[derive(Deserialize, JsonSchema)]
struct Goal {
    id: String,
    #[serde(default = "maker_stage")]
    stage: String,
}
fn maker_stage() -> String {
    "maker".into()
}
#[derive(Deserialize, JsonSchema)]
struct Finding {
    id: String,
}
#[derive(Default, Deserialize, JsonSchema)]
struct Plan {
    #[serde(default)]
    checks: Vec<PlannedCheck>,
}
#[derive(Deserialize, JsonSchema)]
struct PlannedCheck {
    name: String,
    tests: Vec<String>,
    kind: String,
    forcing: Option<Value>,
    declared_symptom: Option<Value>,
}
#[derive(Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum Reply {
    Done {
        self_check: Vec<SelfCheck>,
        #[serde(default)]
        reproductions: Vec<Reproduction>,
    },
    NeedsFiles {
        files: Vec<RequestedFile>,
    },
    CantMeet,
    Blocked,
    OutOfRoom,
}
#[derive(Deserialize, JsonSchema)]
struct SelfCheck {
    goal_id: String,
    status: String,
}
#[derive(Deserialize, JsonSchema)]
struct Reproduction {
    finding_id: String,
    tests: Option<Vec<String>>,
    // Retain the raw declaration until the harness-compatible diagnostic has
    // been selected, then parse the canonical contracts::ProofDeclaration.
    #[serde(default, deserialize_with = "crate::contracts::bounded::present")]
    proof: Option<Value>,
}
#[derive(Deserialize, JsonSchema)]
struct RequestedFile {
    path: String,
}
#[derive(Serialize, JsonSchema)]
struct Handoff<'a> {
    base_sha: &'a str,
    head_sha: &'a str,
    head_tree: &'a str,
    status: &'static str,
    owner: &'static str,
    checks: Vec<HandoffCheck<'a>>,
}
#[derive(Serialize, JsonSchema)]
struct HandoffCheck<'a> {
    name: &'a str,
    tests: &'a [String],
    mode: &'static str,
}

fn check(inputs: &Inputs, reply: &Reply) -> Result<(), String> {
    match reply {
        Reply::Done {
            self_check,
            reproductions,
        } => {
            if !reproducing::exact_coverage(
                inputs.goals.iter().map(|goal| goal.id.as_str()),
                self_check.iter().map(|check| check.goal_id.as_str()),
            ) {
                return Err("self-check must cover every goal exactly once".into());
            }
            let stages: BTreeMap<_, _> = inputs
                .goals
                .iter()
                .map(|goal| (&goal.id, &goal.stage))
                .collect();
            for check in self_check {
                let maker = stages[&check.goal_id] == "maker";
                if !decision::stage_valid(maker, check.status == "met", check.status == "pending") {
                    return Err(if maker {
                        "done requires every maker goal to be met"
                    } else {
                        "downstream goals must remain pending for their owning stage"
                    }
                    .into());
                }
            }
            // Python first deduplicates the current finding ids.
            let findings: BTreeSet<_> = inputs
                .current_review_findings
                .iter()
                .map(|finding| finding.id.as_str())
                .collect();
            if !reproducing::exact_coverage(
                findings.into_iter(),
                reproductions.iter().map(|item| item.finding_id.as_str()),
            ) {
                return Err(
                    "reproductions must cover each current review finding exactly once".into(),
                );
            }
            for item in reproductions {
                if let Some(tests) = &item.tests {
                    let declaration = item
                        .proof
                        .as_ref()
                        .ok_or("A reproduction needs proof.red_message for its assertion RED.")?;
                    proof::parse(declaration)?;
                    if item.finding_id.is_empty() || tests.is_empty() {
                        return Err("a check needs a name, a kind and named tests".into());
                    }
                    if tests.iter().collect::<BTreeSet<_>>().len() != tests.len() {
                        return Err("a check repeats a test".into());
                    }
                    for test in tests {
                        if !reproducing::exact_pytest_node_id(test) {
                            return Err(format!(
                                "not a pytest selector: {}",
                                proof::python_repr(test)
                            ));
                        }
                    }
                }
            }
        }
        Reply::NeedsFiles { files } => {
            for file in files {
                requested_file(&file.path)?;
            }
        }
        Reply::CantMeet | Reply::Blocked | Reply::OutOfRoom => {}
    }
    Ok(())
}

/// The controller can call this with the same inputs it sends the worker.
pub fn validate(inputs: &Value, reply: &Value) -> Result<(), String> {
    let inputs =
        serde_json::from_value(inputs.clone()).map_err(|_| "invalid maker decision inputs")?;
    let reply =
        serde_json::from_value(reply.clone()).map_err(|_| "invalid maker decision reply")?;
    check(&inputs, &reply)
}

fn handoff<'a>(inputs: &'a Inputs, head: &'a str, tree: &'a str) -> Option<Handoff<'a>> {
    let base = inputs
        .check_base_sha
        .as_deref()
        .filter(|base| !base.is_empty())?;
    let checks: Vec<_> = inputs
        .plan
        .checks
        .iter()
        .filter(|check| {
            check.kind == "demonstrating"
                && check.forcing.is_none()
                && check.declared_symptom.is_none()
        })
        .map(|check| HandoffCheck {
            name: &check.name,
            tests: &check.tests,
            mode: "base_subject_head_observer",
        })
        .collect();
    if checks.is_empty() {
        return None;
    }
    Some(Handoff {
        base_sha: base,
        head_sha: head,
        head_tree: tree,
        status: "pending",
        owner: "checking",
        checks,
    })
}

pub fn checking_handoff(inputs: &Value, head: &str, tree: &str) -> Value {
    let inputs: Inputs = serde_json::from_value(inputs.clone()).expect("typed maker inputs");
    serde_json::to_value(handoff(&inputs, head, tree)).expect("handoff serializes")
}

/// One strict transport, including the pure projection at its original loop point.
pub fn decide(request: &Value) -> crate::Result<Value> {
    let request: Request = serde_json::from_value(request.clone())
        .map_err(|_| crate::Error::Invalid("invalid maker checker request"))?;
    match request.operation {
        Operation::Check => {
            let reply = request
                .reply
                .as_ref()
                .ok_or(crate::Error::Invalid("missing maker reply"))?;
            let violations: Vec<_> = check(&request.inputs, reply).err().into_iter().collect();
            Ok(json!({"violations": violations}))
        }
        Operation::Handoff => {
            let head = request
                .head
                .as_deref()
                .ok_or(crate::Error::Invalid("missing maker head"))?;
            let tree = request
                .tree
                .as_deref()
                .ok_or(crate::Error::Invalid("missing maker tree"))?;
            Ok(json!({"handoff": handoff(&request.inputs, head, tree)}))
        }
    }
}

fn requested_file(path: &str) -> Result<(), &'static str> {
    if path.is_empty() || path.contains(['\0', '\\']) || path.starts_with('/') {
        return Err("Use a relative worktree path");
    }
    if path.split('/').any(|part| part == "..") {
        return Err("Parent traversal is forbidden");
    }
    let parts: Vec<_> = path
        .split('/')
        .filter(|part| !part.is_empty() && *part != ".")
        .collect();
    if parts.contains(&".git") {
        return Err("Git metadata paths are forbidden");
    }
    if path.len() > 3072 || parts.iter().any(|part| part.len() > 255) {
        return Err("Path is too long");
    }
    if parts.is_empty() {
        return Err("expected a requested file");
    }
    Ok(())
}

pub fn generated() -> crate::contracts::Generated {
    let mut schema = schemars::schema_for!(Request).to_value();
    schema.as_object_mut().unwrap().insert(
        "$id".into(),
        format!("{}maker-check.json", crate::contracts::SCHEMA_BASE).into(),
    );
    crate::contracts::Generated {
        path: format!("{}/maker-check.json", crate::contracts::SCHEMA_DIRECTORY),
        text: format!("{}\n", serde_json::to_string_pretty(&schema).unwrap()),
    }
}
