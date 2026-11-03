//! Stage obligations and the recorded proof handed to Publishing (#552, #526).
use crate::{Error, Result, executor::Delivery};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tl3_kernel::NodeId;
use tl3_store::{Commit, RowChange, Snapshot};

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PullRequestStatus {
    #[default]
    NotCaptured,
    Open,
    Closed,
    Merged,
}

/// Field names and shapes match #526; stage ownership preserves all original goals.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Evidence {
    pub pull_request_status: PullRequestStatus,
    pub stop_reason: Option<String>,
    pub checks: Vec<Value>,
    pub reviews: Vec<Value>,
    pub audit: Option<Value>,
    pub queue: Vec<Value>,
    pub scratch: Vec<String>,
    pub scratch_history: Vec<Value>,
    pub goals: Vec<Value>,
    pub remaining_failures: Vec<Value>,
    pub artifacts: Vec<String>,
    pub maker_handover: Option<Value>,
}

pub(crate) fn capture(snapshot: &Snapshot, commit: &Commit, node: NodeId) -> Result<Evidence> {
    let id = node.0.to_string();
    let prefix = format!("{id}:");
    let mut rows = std::collections::BTreeMap::new();
    let relevant = |table: &str, row: &str| match table {
        "goals" | "maker_replies" => row == id,
        "check_reports" | "review_evidence" => row.starts_with(&prefix),
        _ => false,
    };
    for row in &snapshot.rows {
        if relevant(&row.table, &row.id) {
            rows.insert((row.table.as_str(), row.id.as_str()), &row.value);
        }
    }
    for row in &commit.rows {
        match row {
            RowChange::Put { table, id, value } if relevant(table, id) => {
                rows.insert((table, id), value);
            }
            _ => {}
        }
    }
    let mut facts = Evidence::default();
    for ((table, _), value) in rows {
        match table {
            "goals" => facts.goals = serde_json::from_value(value.clone())?,
            "maker_replies" => {
                if value["kind"] == "done"
                    && value["self_check"]
                        .as_array()
                        .is_some_and(|checks| crate::goals::handover(&facts.goals, checks).is_ok())
                {
                    facts.maker_handover = Some(json!({"kind":value["kind"], "head":value["head"],
                        "self_check":value["self_check"]}));
                }
            }
            "check_reports" => facts.checks.push(value.clone()),
            "review_evidence" => facts.reviews.push(value.clone()),
            _ => unreachable!(),
        }
    }
    Ok(facts)
}

fn publication_goals(delivery: &Delivery) -> impl Iterator<Item = &Value> {
    delivery
        .evidence
        .as_ref()
        .into_iter()
        .flat_map(|e| &e.goals)
        .filter(|goal| goal["stage"] == "publishing")
}

pub(crate) fn publication_lines(delivery: &Delivery, body: &str) -> String {
    let mut body = body.to_owned();
    for goal in publication_goals(delivery) {
        for line in goal["publication"]["body_lines"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
        {
            if !body.lines().any(|existing| existing == line) {
                body.push('\n');
                body.push_str(line);
                body.push('\n');
            }
        }
    }
    body
}

pub(crate) fn validate_publication(
    delivery: &Delivery,
    branch: &str,
    draft: bool,
    body: &str,
) -> Result<()> {
    for goal in publication_goals(delivery) {
        let requirements = &goal["publication"];
        let branch_ok = requirements["branch"]
            .as_str()
            .is_none_or(|required| branch == required)
            && requirements["branch_prefix"]
                .as_str()
                .is_none_or(|prefix| branch.starts_with(prefix));
        let lines_ok = requirements["body_lines"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .all(|line| body.lines().filter(|existing| *existing == line).count() == 1);
        if !branch_ok || draft || !lines_ok {
            return Err(Error::Permanent {
                class: "pr_obligation",
                source: format!(
                    "Publishing obligation {} is unmet (branch={}, non_draft={}, body_lines={})",
                    goal["id"].as_str().unwrap_or("unnamed"),
                    branch_ok,
                    !draft,
                    lines_ok
                )
                .into(),
            });
        }
    }
    Ok(())
}

/// Only trusted controller results on the approved head can satisfy publication proof.
pub(crate) fn publication(delivery: &Delivery, _request: &str) -> Result<String> {
    let required = publication_goals(delivery).any(|g| g["publication"]["proof"] == true);
    let checks: Vec<_> = delivery
        .evidence
        .as_ref()
        .into_iter()
        .flat_map(|e| &e.checks)
        .filter(|record| record["head_sha"].as_str() == delivery.head_sha.as_deref())
        .collect();
    if !required {
        return Ok(String::new());
    }
    let complete = !checks.is_empty()
        && checks.iter().all(|record| {
            record["report"] == true
                && record["pass"] == true
                && record["results"].as_array().is_some_and(|results| {
                    !results.is_empty()
                        && results.iter().all(|result| {
                            result["verdict"] == "pass"
                                && result["tests"].as_array().is_some_and(|tests| {
                                    !tests.is_empty()
                                        && tests.iter().all(|test| {
                                            (test["head"] == "passed"
                                                || (result["kind"] == "regression"
                                                    && test["pre_existing"] == true))
                                                && test["passed"] == true
                                                && (result.get("forcing").is_none()
                                                    || (test["base"] == "assertion"
                                                        && test["head_forced"] == "passed"))
                                        })
                                })
                        })
                })
        });
    let approved = delivery.evidence.as_ref().is_some_and(|e| {
        e.reviews.iter().any(|record| {
            record["head_sha"].as_str() == delivery.head_sha.as_deref()
                && record["verdict"] == "approve"
        })
    });
    if !complete || !approved {
        return Err(Error::Permanent {
            class: "pr_evidence",
            source: "Required independent proof and review for this head were not captured".into(),
        });
    }
    // Select only proof fields. Never publish forcing values or raw diagnostics.
    let proof: Vec<_> = checks.iter().map(|record| json!({"head_sha":record["head_sha"],
        "base_sha":record["base_sha"], "action_key":record["action_key"], "report":record["report"],
        "pass":record["pass"], "results":record["results"].as_array().unwrap().iter().map(|result| {
            json!({"name":result["name"], "kind":result["kind"], "verdict":result["verdict"],
                "forced":result.get("forcing").is_some(), "tests":result["tests"].as_array().unwrap().iter().map(|test| {
                    json!({"test":test["test"],"base":test["base"],"head":test["head"],"head_forced":test["head_forced"],"passed":test["passed"],"pre_existing":test["pre_existing"]})
                }).collect::<Vec<_>>()})
        }).collect::<Vec<_>>()})).collect();
    let encoded = serde_json::to_string(&json!({"checks":proof,"review":"approve"}))?
        .replace('`', "\\u0060")
        .replace('<', "\\u003c")
        .replace('@', "\\u0040");
    if encoded.len() > 24_000 {
        return Err(Error::Permanent {
            class: "pr_evidence",
            source: "Required proof exceeds the body budget; an artifact is required".into(),
        });
    }
    Ok(format!(
        "<!-- tl3:recorded-evidence:start -->\n## Recorded evidence\n\n```json\n{encoded}\n```\n<!-- tl3:recorded-evidence:end -->"
    ))
}

pub(crate) fn with_publication(body: &str, evidence: &str) -> String {
    if evidence.is_empty() || body.contains(evidence) {
        return body.to_owned();
    }
    format!("{body}\n\n{evidence}")
}
