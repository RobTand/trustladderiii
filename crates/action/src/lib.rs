//! The action's implemented policy operations. Execution remains in later slices.
pub mod plugin;
mod schema;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tl3_contracts::rules::{self, Policy, RenderContext, Role, TerminalForm, Violation};
use tl3_redaction::structured::{Redactor, Shaped};

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PolicyRequest {
    pub state: Redactor,
    pub operation: PolicyOperation,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum PolicyOperation {
    RepairPrompt {
        message: String,
    },
    SchemaRows {
        errors: Vec<schema::ValidationFact>,
        root_schema: Option<serde_json::Value>,
        role: Role,
    },
    Membership {
        allowed: rules::Allowed,
        value: serde_json::Value,
    },
    AdmitPlan {
        checks: Vec<tl3_contracts::Check>,
        goal_count: usize,
        required_goals: Vec<tl3_contracts::GoalId>,
    },
    Text {
        value: String,
        limit: usize,
    },
    Window {
        value: String,
        start: usize,
        end: usize,
    },
    Shape {
        input_json: String,
        limit: usize,
    },
    Reshape {
        shaped: Shaped,
        limit: usize,
    },
    Clean {
        shaped: Shaped,
        limit: usize,
    },
    Payload {
        input_json: String,
        limit: usize,
    },
    Size {
        shaped: Shaped,
    },
    MeasureJson {
        input_json: String,
    },
    SensitivePath {
        input_json: String,
    },
    FileMarker {
        path: String,
    },
    Render {
        rows: Vec<Violation>,
        role: Role,
        context: RenderContext,
    },
    Validate {
        rows: Vec<Violation>,
        role: Role,
    },
    Permits {
        rows: Vec<Violation>,
        before: serde_json::Value,
        after: serde_json::Value,
        role: Role,
    },
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PolicyResponse {
    pub state: Redactor,
    pub result: PolicyResult,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum PolicyResult {
    Text(String),
    Json(String),
    Shaped(Shaped),
    Size(usize),
    Path(Option<String>),
    Permitted(bool),
    Valid,
    Rows(Vec<Violation>),
}
fn cant_meet(reply: &serde_json::Value) -> bool {
    reply.get("kind").and_then(serde_json::Value::as_str) == Some("cant_meet")
}
const TERMINALS: &[TerminalForm] = &[
    TerminalForm {
        role: Role::Planner,
        predicate: cant_meet,
        phrase: "Return a reply with kind \"cant_meet\".",
    },
    TerminalForm {
        role: Role::Maker,
        predicate: cant_meet,
        phrase: "Return a reply with kind \"cant_meet\".",
    },
];
fn row_policy() -> Policy<'static> {
    Policy {
        terminals: TERMINALS,
        source_editors: &[Role::Maker],
        schema_membership: Some(schema::membership),
    }
}

pub fn policy(request: PolicyRequest) -> Result<PolicyResponse, String> {
    let mut state = request.state;
    let result = match request.operation {
        PolicyOperation::RepairPrompt { message } => {
            let complete = format!(
                "Your reply failed validation: {message}\nTools are unavailable for this turn. Return one corrected JSON object matching your reply schema and nothing else."
            );
            let safe = state.window(&complete, 0, complete.chars().count())?;
            if safe.len() > rules::MESSAGE_BYTES {
                return Err("complete repair prompt exceeds publication byte budget".into());
            }
            PolicyResult::Text(safe)
        }
        PolicyOperation::SchemaRows {
            errors,
            root_schema,
            role,
        } => PolicyResult::Rows(schema::refusal_rows(&errors, root_schema.as_ref(), role)?),
        PolicyOperation::Membership { allowed, value } => {
            PolicyResult::Permitted(allowed.accepts(&value, &row_policy())?)
        }
        PolicyOperation::AdmitPlan {
            checks,
            goal_count,
            required_goals,
        } => {
            tl3_contracts::checks::admit_plan(&checks, goal_count, &required_goals)?;
            for check in &checks {
                for test in &check.tests {
                    tl3_harness_core::node_id::NodeId::parse(
                        test,
                        tl3_harness_core::node_id::Mode::Exact,
                    )
                    .map_err(|_| "planner checks need exact pytest selectors")?;
                }
            }
            PolicyResult::Valid
        }
        PolicyOperation::Text { value, limit } => PolicyResult::Text(state.text(&value, limit)?),
        PolicyOperation::Window { value, start, end } => {
            PolicyResult::Text(state.window(&value, start, end)?)
        }
        PolicyOperation::Shape { input_json, limit } => {
            PolicyResult::Shaped(state.shape(&input_json, limit)?)
        }
        PolicyOperation::Reshape { shaped, limit } => {
            PolicyResult::Shaped(state.reshape(&shaped, limit)?)
        }
        PolicyOperation::Clean { shaped, limit } => {
            PolicyResult::Json(state.clean(&shaped, limit)?)
        }
        PolicyOperation::Payload { input_json, limit } => {
            PolicyResult::Json(state.payload(&input_json, limit)?)
        }
        PolicyOperation::Size { shaped } => PolicyResult::Size(shaped.size()?),
        PolicyOperation::MeasureJson { input_json } => {
            PolicyResult::Size(tl3_redaction::structured::measure_json(&input_json)?)
        }
        PolicyOperation::SensitivePath { input_json } => {
            PolicyResult::Path(tl3_redaction::structured::sensitive_path_json(&input_json)?)
        }
        PolicyOperation::FileMarker { path } => {
            PolicyResult::Text(tl3_redaction::structured::file_marker(&path))
        }
        PolicyOperation::Render {
            rows,
            role,
            context,
        } => {
            let rendered = rules::render_all(&rows, role, context, &row_policy())?;
            let safe = state.window(&rendered, 0, rendered.chars().count())?;
            if safe.len() > rules::MESSAGE_BYTES {
                return Err("complete redacted refusal exceeds publication byte budget".into());
            }
            PolicyResult::Text(safe)
        }
        PolicyOperation::Validate { rows, role } => {
            for row in rows {
                row.validate(role, &row_policy())?;
            }
            PolicyResult::Valid
        }
        PolicyOperation::Permits {
            rows,
            before,
            after,
            role,
        } => PolicyResult::Permitted(rules::permits(&rows, &before, &after, role, &row_policy())?),
    };
    Ok(PolicyResponse { state, result })
}

pub fn generated() -> Vec<tl3_contracts::Generated> {
    fn file<T: JsonSchema>(name: &str) -> tl3_contracts::Generated {
        let schema = schemars::generate::SchemaSettings::draft2020_12()
            .into_generator()
            .into_root_schema_for::<T>();
        tl3_contracts::Generated {
            path: format!("workers/src/tl3_workers/schemas/{name}.json"),
            text: format!(
                "{}\n",
                serde_json::to_string_pretty(&schema).expect("schema serializes")
            ),
        }
    }
    vec![
        file::<PolicyRequest>("action-policy-request"),
        file::<PolicyResponse>("action-policy-response"),
        file::<plugin::PluginFact>("plugin-fact"),
        file::<plugin::PluginResponse>("plugin-response"),
    ]
}
