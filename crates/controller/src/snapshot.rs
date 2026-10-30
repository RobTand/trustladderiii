use crate::{Error, Result};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use tl3_kernel::World;
use tl3_store::{RowChange, Snapshot};

pub(crate) type Rows = BTreeMap<(String, String), Value>;

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
    let world: World = serde_json::from_value(value)?;
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
