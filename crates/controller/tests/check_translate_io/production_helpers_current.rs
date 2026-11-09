use super::*;
pub(crate) fn maker_inputs(
    brief: &serde_json::Value,
    plan: &serde_json::Value,
) -> Result<serde_json::Value> {
    use crate::Error;
    use serde_json::json;
    if plan["kind"] != "leaf" {
        return Err(Error::Invalid("making requires an accepted leaf plan"));
    }
    let goals = crate::goals::identified(&brief["goals"])?;
    let checks = plan["checks"]
        .as_array()
        .ok_or(Error::Invalid("accepted plan needs checks"))?
        .iter()
        .map(|value| {
            serde_json::from_value::<tl3_contracts::Check>(value.clone()).map_err(Error::from)
        })
        .collect::<Result<Vec<_>>>()?;
    let required: Vec<_> = goals
        .iter()
        .enumerate()
        .filter(|(_, goal)| goal["stage"] != "publishing")
        .map(|(index, _)| tl3_contracts::GoalId(index as u64))
        .collect();
    tl3_contracts::checks::admit_plan(&checks, goals.len(), &required)
        .map_err(|_| Error::Invalid("accepted plan violates check goal admission"))?;
    for check in &checks {
        for test in &check.tests {
            tl3_harness_core::node_id::NodeId::parse(test, tl3_harness_core::node_id::Mode::Exact)
                .map_err(|_| Error::Invalid("accepted plan needs exact pytest selectors"))?;
        }
    }
    Ok(
        json!({"brief":brief["brief"], "goals":goals, "plan":{"files":plan["files"], "checks":checks}}),
    )
}
