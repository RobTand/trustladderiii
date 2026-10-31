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
