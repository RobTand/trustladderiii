//! Accepted answers are request data, committed with steering and inherited by
//! replacements. Hindsight remains supplementary and never supplies authorization.
use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tl3_kernel::NodeId;
use tl3_store::Snapshot;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Context {
    pub schema_version: u32,
    pub request_version: u64,
    pub decisions: Vec<Decision>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Decision {
    pub id: String,
    pub source_comment: Option<String>,
    pub question: Value,
    pub answer: String,
    pub allowed_scope: Vec<String>,
    pub restrictions: Vec<String>,
    pub origin_request_version: u64,
    pub superseded_by: Option<String>,
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
        let context = snapshot
            .rows
            .iter()
            .find(|row| row.table == "request_decisions" && row.id == node.0.to_string())
            .map(|row| crate::decode_stored::<Self>(row.value.clone()))
            .transpose()?
            .unwrap_or_else(|| Self::new(node));
        if context.schema_version != 1 || context.request_version != node.0 {
            return Err(Error::Invalid("unsupported request decision context"));
        }
        Ok(context)
    }

    pub fn applicable(mut self) -> Self {
        self.decisions
            .retain(|decision| decision.superseded_by.is_none());
        self
    }
}

/// Read pending writes too: an answer and a replacement may share one commit.
pub(crate) fn inherit(
    snapshot: &Snapshot,
    commit: &mut tl3_store::Commit,
    original: NodeId,
    replacement: NodeId,
) -> Result<()> {
    let pending = commit.rows.iter().rev().find_map(|row| match row {
        tl3_store::RowChange::Put { table, id, value }
            if table == "request_decisions" && *id == original.0.to_string() =>
        {
            Some(value.clone())
        }
        _ => None,
    });
    let mut context = match pending {
        Some(value) => crate::decode_stored::<Context>(value)?,
        None => Context::load(snapshot, original)?,
    };
    context.request_version = replacement.0;
    commit.rows.push(crate::put(
        "request_decisions",
        replacement.0.to_string(),
        serde_json::to_value(context)?,
    ));
    Ok(())
}
