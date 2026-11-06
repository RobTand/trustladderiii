//! #845/Q27: declared authorization scope survives independently of exclusive locks.
use crate::{Error, Result, decode_stored};
use tl3_kernel::{Cont, Event, Node, NodeId, Phase, Reply, Resume, World};
use tl3_store::{Commit, InternKind, Interned, RowChange, Snapshot};

pub(crate) const TABLE: &str = "declared_scopes";

pub(crate) fn stored(snapshot: &Snapshot, node: NodeId) -> Result<Option<Vec<String>>> {
    snapshot
        .rows
        .iter()
        .find(|r| r.table == TABLE && r.id == node.0.to_string())
        .map(|row| decode_stored(row.value.clone()))
        .transpose()
}

pub(crate) fn paths(
    snapshot: &Snapshot,
    commit: Option<&Commit>,
    world: &World,
    node: NodeId,
    dictionary: &[Interned],
) -> Result<Vec<String>> {
    let pending = commit.and_then(|c| {
        c.rows.iter().rev().find(|row| match row {
            RowChange::Put { table, id, .. } | RowChange::Delete { table, id } => {
                table == TABLE && id == &node.0.to_string()
            }
        })
    });
    let declared = match pending {
        Some(RowChange::Put { value, .. }) => Some(decode_stored(value.clone())?),
        Some(RowChange::Delete { .. }) => None,
        None => stored(snapshot, node)?,
    };
    if let Some(paths) = declared {
        return Ok(paths);
    }
    let n = world
        .nodes
        .get(node.0 as usize)
        .ok_or(Error::Invalid("scope node missing"))?;
    world
        .files(n.footprint)
        .ok_or(Error::Invalid("unknown footprint"))?
        .map(|file| {
            dictionary
                .iter()
                .find(|i| i.kind == InternKind::Path && i.id == file.0 as i64)
                .map(|i| i.value.clone())
                .ok_or(Error::Invalid("path missing from intern dictionary"))
        })
        .collect()
}

/// #845: discard speculative scope writes unless the real reply changed the leaf.
/// A rejected PlanLeaf or extension can change phase, so also require its footprint.
pub(crate) fn finish_reply(before: Option<Node>, event: Event, after: &World, commit: &mut Commit) {
    let Event::Reply {
        node,
        generation,
        reply,
    } = event
    else {
        return;
    };
    let expected = match reply {
        Reply::PlanLeaf {
            footprint,
            fits: true,
        } => Some((footprint, true)),
        Reply::MakerNeedsFiles { files, fits: true } => Some((files, false)),
        _ => None,
    };
    let accepted = before
        .zip(after.nodes.get(node.0 as usize))
        .is_some_and(|(old, new)| {
            expected.is_some_and(|(footprint, plan)| {
                old.run == Some(generation)
                    && !footprint.is_empty()
                    && new.generation != old.generation
                    && if plan {
                        old.phase == (Phase::Planning { must_split: false })
                            && new.footprint == footprint
                            && matches!(new.phase, Phase::Making | Phase::Ready(Resume::Making))
                    } else {
                        old.phase == Phase::Making
                            && (matches!(new.phase, Phase::Making | Phase::Ready(Resume::Making))
                                || new.phase == Phase::AwaitingRob
                                    && new.question.is_some_and(|question| {
                                        question.cont == Cont::Ready(Resume::Making)
                                    }))
                    }
            })
        });
    if !accepted {
        commit.rows.retain(|row| {
            !matches!(row, RowChange::Put { table, id, .. }
            if table == TABLE && id == &node.0.to_string())
        });
    }
}
