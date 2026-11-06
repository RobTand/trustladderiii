use super::*;
pub(crate) fn maker_inputs(
    brief: &serde_json::Value,
    plan: &serde_json::Value,
) -> Result<serde_json::Value> {
    use crate::Error;
    use serde_json::{Value, json};
    if plan["kind"] != "leaf" {
        return Err(Error::Invalid("making requires an accepted leaf plan"));
    }
    let goals = crate::goals::identified(&brief["goals"])?;
    let checks = plan["checks"].as_array().ok_or(Error::Invalid("accepted plan needs checks"))?.iter().enumerate().map(|(index, check)| {
        let ids = check["goal_ids"].as_array().ok_or(Error::Invalid("plan check needs goal indexes"))?.iter().map(|id| {
            id.as_u64().and_then(|id| usize::try_from(id).ok()).and_then(|id| goals.get(id)).map(|g| g["id"].clone()).ok_or(Error::Invalid("plan check names an unknown goal"))
        }).collect::<Result<Vec<_>>>()?;
        let mut planned = json!({"name":format!("check-{}",index + 1), "kind":check["kind"], "tests":check["node_ids"], "goal_ids":ids});
        if let Some(forcing) = check.get("forcing") {
            planned["forcing"] = forcing.clone();
        }
        if let Some(symptom) = check.get("declared_symptom") {
            planned["declared_symptom"] = symptom.clone();
        }
        if let Some(evidence) = check.get("proof") {
            planned["proof"] = evidence.clone();
        }
        Ok(planned)
    }).collect::<Result<Vec<Value>>>()?;
    Ok(
        json!({"brief":brief["brief"], "goals":goals, "plan":{"files":plan["files"], "checks":checks}}),
    )
}
