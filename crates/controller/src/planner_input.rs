use crate::{Error, Result, decode_stored, put};
use serde_json::Value;
use tl3_kernel::{Event, NodeId, PlannerResult, ReplanBudget};
use tl3_store::{Commit, RowChange, Snapshot};

#[derive(Clone, Copy, Debug)]
pub(crate) enum KernelInput {
    Event(Event),
    Planner { node: NodeId, generation: u64, result: PlannerResult },
}

impl From<Event> for KernelInput {
    fn from(event: Event) -> Self { Self::Event(event) }
}

/// Pending deletion is authoritative, just like a pending replacement row.
pub(crate) fn request_row(snapshot: &Snapshot, commit: &Commit, node: NodeId) -> Result<Value> {
    let id = node.0.to_string();
    for change in commit.rows.iter().rev() {
        match change {
            RowChange::Put { table, id: key, value } if table == "requests" && *key == id => {
                return Ok(value.clone());
            }
            RowChange::Delete { table, id: key } if table == "requests" && *key == id => {
                return Err(Error::Invalid("deleted request binding"));
            }
            _ => {}
        }
    }
    snapshot.rows.iter().find(|row| row.table == "requests" && row.id == id)
        .map(|row| row.value.clone()).ok_or(Error::Invalid("missing request binding"))
}

pub(crate) fn load_budget(snapshot: &Snapshot, commit: &Commit, node: NodeId) -> Result<ReplanBudget> {
    let row = request_row(snapshot, commit, node)?;
    let budget = match row.get("replan_budget") {
        Some(value) => decode_stored::<ReplanBudget>(value.clone())?,
        None => {
            // Inspect raw legacy storage before World decoding drops the old field.
            let legacy = snapshot.rows.iter().find(|row| row.table == "nodes" && row.id == node.0.to_string())
                .and_then(|row| row.value.get("empty_allowed_replanned"));
            let consumed = match legacy {
                Some(Value::Bool(consumed)) => *consumed,
                Some(_) => return Err(Error::Invalid("invalid legacy replan budget")),
                None => false,
            };
            ReplanBudget::import(node, consumed)
        }
    };
    if !budget.bound_to(node) { return Err(Error::Invalid("replan budget binding mismatch")); }
    Ok(budget)
}

pub(crate) fn save_budget(snapshot: &Snapshot, commit: &mut Commit, node: NodeId, budget: ReplanBudget) -> Result<()> {
    if !budget.bound_to(node) { return Err(Error::Invalid("replan budget binding mismatch")); }
    let mut row = request_row(snapshot, commit, node)?;
    let fields = row.as_object_mut().ok_or(Error::Invalid("invalid request binding"))?;
    fields.insert("replan_budget".into(), serde_json::to_value(budget)?);
    commit.rows.push(put("requests", node.0.to_string(), row));
    Ok(())
}
