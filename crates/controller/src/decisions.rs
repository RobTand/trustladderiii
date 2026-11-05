//! Accepted answers are durable request data. Live contexts reference a bounded
//! set; workers receive only the applicable records, never the history.
use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tl3_kernel::NodeId;
use tl3_store::{Commit, RowChange, Snapshot};

pub(crate) const MAX_LIVE_DECISIONS: usize = 64;

pub(crate) use crate::contracts::inputs::{Context, Decision};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct References {
    schema_version: u32,
    request_version: u64,
    decision_ids: Vec<String>,
}

fn value<'a>(
    snapshot: &'a Snapshot,
    changes: &'a [RowChange],
    table: &str,
    id: &str,
) -> Option<&'a Value> {
    changes
        .iter()
        .rev()
        .find_map(|row| match row {
            RowChange::Put {
                table: t,
                id: i,
                value,
            } if t == table && i == id => Some(value),
            _ => None,
        })
        .or_else(|| {
            snapshot
                .rows
                .iter()
                .find(|r| r.table == table && r.id == id)
                .map(|r| &r.value)
        })
}

impl Context {
    pub fn new(node: NodeId) -> Self {
        Self {
            schema_version: 1,
            request_version: node.0,
            decisions: vec![],
        }
    }

    pub fn load(snapshot: &Snapshot, node: NodeId) -> Result<Self> {
        Self::read(snapshot, &[], node)
    }

    fn read(snapshot: &Snapshot, changes: &[RowChange], node: NodeId) -> Result<Self> {
        let Some(stored) = value(snapshot, changes, "request_decisions", &node.0.to_string())
        else {
            return Ok(Self::new(node));
        };
        // Version 1 is normalized once on the next write; no history is copied
        // into a version 2 request context.
        let context: Self = if stored["schema_version"] == 1 {
            crate::decode_stored(stored.clone())?
        } else {
            let refs: References = crate::decode_stored(stored.clone())?;
            if refs.schema_version != 2
                || refs.request_version != node.0
                || refs.decision_ids.len() > MAX_LIVE_DECISIONS
            {
                return Err(Error::Invalid("invalid request decision references"));
            }
            let decisions = refs
                .decision_ids
                .iter()
                .map(|id| {
                    let stored = value(snapshot, changes, "decisions", id)
                        .ok_or(Error::Invalid("missing referenced decision"))?;
                    let decision: Decision = crate::decode_stored(stored.clone())?;
                    if decision.id != *id {
                        return Err(Error::Invalid("decision identity mismatch"));
                    }
                    Ok(decision)
                })
                .collect::<Result<Vec<_>>>()?;
            Self {
                schema_version: 1,
                request_version: node.0,
                decisions,
            }
        };
        if context.schema_version != 1
            || context.request_version != node.0
            || context.decisions.len() > MAX_LIVE_DECISIONS
        {
            return Err(Error::Invalid(
                "unsupported or oversized request decision context",
            ));
        }
        Ok(context)
    }

    pub fn applicable(mut self) -> Self {
        self.decisions
            .retain(|decision| decision.superseded_by.is_none());
        self
    }

    pub fn store(&self, snapshot: &Snapshot, commit: &mut Commit) -> Result<()> {
        let decision_ids: Vec<_> = self
            .decisions
            .iter()
            .filter(|d| d.superseded_by.is_none())
            .map(|d| d.id.clone())
            .collect();
        if decision_ids.len() > MAX_LIVE_DECISIONS {
            return Err(Error::Invalid("too many applicable request decisions"));
        }
        for decision in &self.decisions {
            let encoded = serde_json::to_value(decision)?;
            if value(snapshot, &commit.rows, "decisions", &decision.id) != Some(&encoded) {
                commit
                    .rows
                    .push(crate::put("decisions", decision.id.clone(), encoded));
            }
        }
        commit.rows.push(crate::put(
            "request_decisions",
            self.request_version.to_string(),
            serde_json::to_value(References {
                schema_version: 2,
                request_version: self.request_version,
                decision_ids,
            })?,
        ));
        Ok(())
    }
}

/// Read pending writes too: an answer and a replacement may share one commit.
pub(crate) fn inherit(
    snapshot: &Snapshot,
    commit: &mut Commit,
    original: NodeId,
    replacement: NodeId,
) -> Result<()> {
    let mut context = Context::read(snapshot, &commit.rows, original)?;
    context.store(snapshot, commit)?;
    context.request_version = replacement.0;
    context.store(snapshot, commit)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn snapshot(context: Value) -> Snapshot {
        Snapshot {
            revision: tl3_store::Revision::ZERO,
            epoch: tl3_store::Epoch::ZERO,
            rows: vec![tl3_store::WorldRow {
                table: "request_decisions".into(),
                id: "0".into(),
                value: context,
                version: tl3_store::Revision::ZERO,
                arrival: 0,
            }],
            pending_effects: vec![],
            credit: vec![],
            interned: vec![],
        }
    }

    #[test]
    fn replacement_keeps_references_instead_of_copying_history() {
        let fixture: Value =
            serde_json::from_str(include_str!("../tests/fixtures/decisions/pq2662.json")).unwrap();
        let snapshot = snapshot(fixture);
        let mut commit = tl3_store::Commit::new("replace", json!({}), &snapshot);
        inherit(&snapshot, &mut commit, NodeId(0), NodeId(1)).unwrap();
        let context = commit
            .rows
            .iter()
            .find_map(|r| match r {
                tl3_store::RowChange::Put { table, id, value }
                    if table == "request_decisions" && id == "1" =>
                {
                    Some(value)
                }
                _ => None,
            })
            .unwrap();
        assert!(
            context.get("decisions").is_none(),
            "request versions must not copy decision history"
        );
        assert_eq!(context["decision_ids"], json!(["pq2662-q10"]));
        assert_eq!(
            commit
                .rows
                .iter()
                .filter(
                    |r| matches!(r, tl3_store::RowChange::Put { table, .. } if table == "decisions")
                )
                .count(),
            1
        );
    }

    #[test]
    fn live_decision_context_is_bounded() {
        let mut fixture: Value =
            serde_json::from_str(include_str!("../tests/fixtures/decisions/pq2662.json")).unwrap();
        let decision = fixture["decisions"][0].clone();
        fixture["decisions"] = json!(
            (0..65)
                .map(|id| {
                    let mut d = decision.clone();
                    d["id"] = json!(format!("decision-{id}"));
                    d
                })
                .collect::<Vec<_>>()
        );
        assert!(
            Context::load(&snapshot(fixture), NodeId(0)).is_err(),
            "live decisions must have an explicit bound"
        );
    }
}
