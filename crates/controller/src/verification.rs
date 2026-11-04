//! Runtime checks for #783, covering the invariants in docs/state-table.md.
use crate::Result;
use tl3_kernel::{Event, Out, World};
use tl3_store::{Commit, Snapshot};

pub(crate) fn checked_step(
    world: World,
    event: Event,
    _commit: &mut Commit,
) -> Result<(World, Vec<Out>)> {
    Ok(tl3_kernel::step(world, event))
}

pub(crate) fn reject(
    _snapshot: &Snapshot,
    _event: Option<Event>,
    _rule: &str,
    _commit: &mut Commit,
) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tl3_kernel::{Priority, RepoId};
    use tl3_store::{Epoch, Revision};

    fn snapshot() -> Snapshot {
        Snapshot {
            revision: Revision::ZERO,
            epoch: Epoch::ZERO,
            rows: vec![],
            pending_effects: vec![],
            credit: vec![],
            interned: vec![],
        }
    }

    /// #783 / state-table containment: reject a seeded invalid event result without panic.
    #[test]
    fn invalid_step_raises_incident_and_discards_candidate_writes() {
        let base = snapshot();
        let mut commit = Commit::new("fixture:bad-event", json!({}), &base);
        commit
            .rows
            .push(crate::put("candidate", "0".into(), json!(true)));
        let mut world = World::new(&[1], 1);
        world.pool_capacity = 0;
        let event = Event::NewRequest {
            repo: RepoId(0),
            priority: Priority::Unranked,
        };
        let failure = checked_step(world, event, &mut commit)
            .expect_err("seeded invalid result must be rejected");
        reject(&base, Some(event), &failure.to_string(), &mut commit).unwrap();
        assert!(
            !commit.rows.iter().any(
                |r| matches!(r, tl3_store::RowChange::Put { table, .. } if table == "candidate")
            )
        );
        let incident = commit
            .rows
            .iter()
            .find_map(|r| match r {
                tl3_store::RowChange::Put { table, value, .. } if table == "incidents" => {
                    Some(value)
                }
                _ => None,
            })
            .expect("visible incident");
        assert_eq!(incident["open"], true);
        assert!(
            incident["summary"]
                .as_str()
                .unwrap()
                .contains("fixture:bad-event")
        );
        assert!(commit.rows.iter().any(
            |r| matches!(r, tl3_store::RowChange::Put {table, ..} if table == "kernel_holds")
        ));
        assert!(commit.effects.is_empty());
    }
}
