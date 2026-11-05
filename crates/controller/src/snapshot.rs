use crate::{Error, Result};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use tl3_kernel::World;
use tl3_store::{RowChange, Snapshot};

pub(crate) type Rows = BTreeMap<(String, String), Value>;

/// #771 / credit ledger: preserve the full-width balance delta in the existing
/// signed-bigint journal. Parts share one atomic commit. Ordinary deltas keep
/// their original reason and amount, and old journal rows remain readable.
pub(crate) fn credit_entries(
    node: usize,
    reason: &str,
    mut delta: i128,
) -> Vec<tl3_store::CreditEntry> {
    let mut entries = Vec::new();
    while delta != 0 {
        let amount = delta.clamp(i64::MIN as i128, i64::MAX as i128) as i64;
        let part = entries.len();
        entries.push(tl3_store::CreditEntry {
            node: node.to_string(),
            root: node.to_string(),
            reason: if part == 0 {
                reason.into()
            } else {
                format!("credit-part:{}:{part}", json!(reason))
            },
            amount,
        });
        delta -= amount as i128;
    }
    entries
}

pub(crate) fn encode(world: &World) -> Result<Rows> {
    let Value::Object(mut fields) = serde_json::to_value(world)? else {
        unreachable!()
    };
    let mut rows = Rows::new();
    for table in ["nodes", "gates", "file_sets"] {
        let Some(Value::Array(values)) = fields.remove(table) else {
            unreachable!()
        };
        for (index, value) in values.into_iter().enumerate() {
            rows.insert((table.into(), index.to_string()), value);
        }
    }
    rows.insert(
        ("kernel".into(), "meta".into()),
        json!({"format": 1, "world": fields}),
    );
    Ok(rows)
}

pub(crate) fn decode(snapshot: &Snapshot) -> Result<World> {
    let meta = snapshot
        .rows
        .iter()
        .find(|r| r.table == "kernel" && r.id == "meta")
        .ok_or(Error::Invalid(
            "controller database is uninitialized; start tl3d first",
        ))?;
    if meta.value["format"] != 1 {
        return Err(Error::Invalid("unsupported snapshot format"));
    }
    let mut value = meta.value["world"].clone();
    for table in ["nodes", "gates", "file_sets"] {
        let mut entries = BTreeMap::new();
        for row in snapshot.rows.iter().filter(|r| r.table == table) {
            let index = row
                .id
                .parse::<usize>()
                .map_err(|_| Error::Invalid("invalid row index"))?;
            if entries.insert(index, row.value.clone()).is_some() {
                return Err(Error::Invalid("duplicate row index"));
            }
        }
        let mut values = Vec::new();
        for (index, item) in entries {
            if index != values.len() {
                return Err(Error::Invalid("gap in kernel rows"));
            }
            values.push(item);
        }
        value[table] = Value::Array(values);
    }
    let world: World = crate::decode_stored(value)?;
    if world
        .file_sets()
        .iter()
        .any(|set| set.windows(2).any(|pair| pair[0] >= pair[1]))
    {
        return Err(Error::Invalid(
            "stored footprint must be sorted and deduplicated",
        ));
    }
    if !world.valid() || !world.is_settled() {
        return Err(Error::Invalid("invalid stored kernel world"));
    }
    Ok(world)
}

pub(crate) fn changes(before: &Rows, after: Rows) -> Vec<RowChange> {
    let mut changes = Vec::new();
    for ((table, id), value) in &after {
        if before.get(&(table.clone(), id.clone())) != Some(value) {
            changes.push(RowChange::Put {
                table: table.clone(),
                id: id.clone(),
                value: value.clone(),
            });
        }
    }
    for (table, id) in before.keys() {
        if !after.contains_key(&(table.clone(), id.clone())) {
            changes.push(RowChange::Delete {
                table: table.clone(),
                id: id.clone(),
            });
        }
    }
    changes
}

#[cfg(test)]
mod credit_tests {
    /// #771: full-width changes remain exact without altering old journal rows.
    #[test]
    fn journal_parts_preserve_wide_signed_deltas() {
        for delta in [
            0,
            1,
            -1,
            i64::MAX as i128 + 1,
            i64::MIN as i128 - 1,
            3 * u64::MAX as i128,
            -(3 * u64::MAX as i128),
        ] {
            let parts = super::credit_entries(0, "consume:input", delta);
            assert!(parts.len() <= 7);
            assert_eq!(
                parts.iter().map(|entry| entry.amount as i128).sum::<i128>(),
                delta
            );
            if let Some(first) = parts.first() {
                assert_eq!(first.reason, "consume:input");
            }
            let reasons: std::collections::BTreeSet<_> =
                parts.iter().map(|entry| &entry.reason).collect();
            assert_eq!(reasons.len(), parts.len());
        }
    }
}
