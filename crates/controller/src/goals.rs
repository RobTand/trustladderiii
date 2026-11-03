use crate::{Error, Result};
use serde_json::{Value, json};
use std::collections::BTreeSet;
use tl3_kernel::NodeId;
use tl3_store::Snapshot;

pub(crate) fn identified(value: &Value) -> Result<Vec<Value>> {
    let mut goals = value
        .as_array()
        .ok_or(Error::Invalid("goals must be a list"))?
        .clone();
    let mut ids = BTreeSet::new();
    for (index, goal) in goals.iter_mut().enumerate() {
        let goal = goal
            .as_object_mut()
            .ok_or(Error::Invalid("goal must be an object"))?;
        let id = goal
            .entry("id")
            .or_insert_with(|| json!(format!("goal-{}", index + 1)));
        let id = id
            .as_str()
            .filter(|id| !id.trim().is_empty())
            .ok_or(Error::Invalid("goal needs an id"))?;
        if !ids.insert(id.to_owned()) {
            return Err(Error::Invalid("goal ids must be unique"));
        }
    }
    Ok(goals)
}

pub(crate) fn load(snapshot: &Snapshot, node: NodeId) -> Result<Option<Vec<Value>>> {
    snapshot
        .rows
        .iter()
        .find(|row| row.table == "goals" && row.id == node.0.to_string())
        .map(|row| identified(&row.value))
        .transpose()
}

/// Recognized notation describes a branch constraint, never a literal branch name.
pub(crate) fn branch_satisfies(
    required: &str,
    branch: &str,
    issue: Option<u64>,
    node: Option<u64>,
    prefix: bool,
) -> bool {
    if !required.contains('<') {
        return if prefix {
            branch.starts_with(required)
        } else {
            branch == required
        };
    }
    let optional_word = required.strip_suffix("[-<word>]");
    let mut rest = optional_word.unwrap_or(required);
    let mut pattern = String::from("^");
    while let Some((literal, tail)) = rest.split_once('<') {
        pattern.push_str(&regex::escape(literal));
        let Some((slot, remaining)) = tail.split_once('>') else {
            return false;
        };
        match slot {
            "issue" => match issue {
                Some(issue) => pattern.push_str(&issue.to_string()),
                None => return false,
            },
            "node" => match node {
                Some(node) => pattern.push_str(&node.to_string()),
                None => return false,
            },
            "word" => pattern.push_str("[A-Za-z0-9][A-Za-z0-9_-]*"),
            _ => return false,
        }
        rest = remaining;
    }
    pattern.push_str(&regex::escape(rest));
    if optional_word.is_some() {
        pattern.push_str("(?:-[A-Za-z0-9][A-Za-z0-9_-]*)?");
    }
    if !prefix {
        pattern.push('$');
    }
    regex::Regex::new(&pattern).is_ok_and(|pattern| pattern.is_match(branch))
}

/// This produces audit objections, never an ingest protocol failure or a weaker goal.
pub(crate) fn branch_objections(context: &Value, goals: &Value) -> Result<Vec<String>> {
    let Some(branch) = context["branch"].as_str() else {
        return Ok(vec![]);
    };
    let mut errors = Vec::new();
    for goal in goals.as_array().into_iter().flatten() {
        for field in ["branch", "branch_prefix"] {
            let Some(required) = goal["publication"][field].as_str() else {
                continue;
            };
            let satisfied = branch_satisfies(
                required,
                branch,
                context["issue"].as_u64(),
                context["node"].as_u64(),
                field == "branch_prefix",
            );
            if !satisfied {
                errors.push(format!("Request-scope branch conflict: {field} requires {}; configured template {} generates {}. Preserve the explicit requirement and its source. Change the request scope or repository convention before planning.",
                    crate::evidence::label(required)?, crate::evidence::label(context["template"].as_str().unwrap_or("unknown"))?, crate::evidence::label(branch)?));
            }
        }
    }
    Ok(errors)
}

/// Ownership comes from the accepted intake, never from a maker's self-report.
pub(crate) fn handover(goals: &[Value], checks: &[Value]) -> Result<()> {
    let expected: BTreeSet<_> = goals.iter().map(|goal| goal["id"].as_str()).collect();
    let supplied: BTreeSet<_> = checks
        .iter()
        .map(|check| check["goal_id"].as_str())
        .collect();
    if expected != supplied || goals.len() != checks.len() {
        return Err(Error::Invalid(
            "maker must account for every obligation exactly once",
        ));
    }
    for goal in goals {
        let check = checks
            .iter()
            .find(|check| check["goal_id"] == goal["id"])
            .unwrap();
        let status = match goal["stage"].as_str().unwrap_or("maker") {
            "maker" => "met",
            "checking" | "publishing" => "pending",
            _ => return Err(Error::Invalid("unknown obligation stage")),
        };
        if check["status"] != status {
            return Err(Error::Invalid(
                "maker reply contradicts accepted stage ownership",
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod stage_tests {
    use super::*;

    #[test]
    fn a_maker_cannot_relabel_a_scope_or_implementation_obligation() {
        let goals = vec![
            json!({"id":"repair"}),
            json!({"id":"scope"}),
            json!({"id":"pr","stage":"publishing"}),
        ];
        let checks = vec![
            json!({"goal_id":"repair","status":"met"}),
            json!({"goal_id":"scope","status":"met"}),
            json!({"goal_id":"pr","status":"pending"}),
        ];
        handover(&goals, &checks).unwrap();
        for id in ["repair", "scope"] {
            let mut forged = checks.clone();
            let check = forged.iter_mut().find(|c| c["goal_id"] == id).unwrap();
            check["status"] = json!("pending");
            check["stage"] = json!("publishing");
            assert!(handover(&goals, &forged).is_err());
        }
        let mut claimed = checks;
        claimed[2]["status"] = json!("met");
        assert!(handover(&goals, &claimed).is_err());
    }
}
