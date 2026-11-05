use super::*;
struct Shell<'a>(&'a mut Controller);
impl std::ops::Deref for Shell<'_> {
    type Target = Controller;
    fn deref(&self) -> &Controller { self.0 }
}
impl std::ops::DerefMut for Shell<'_> {
    fn deref_mut(&mut self) -> &mut Controller { self.0 }
}
impl Shell<'_> {
    fn translate(&mut self, world: &World, observation: Observation, commit: &mut Commit) -> Result<Option<Event>> {
        let event = match observation {
            Observation::Rebased {
                node,
                generation,
                sha,
            } => {
                let current = world
                    .nodes
                    .get(node.0 as usize)
                    .ok_or(Error::Invalid("rebase observation has no request"))?;
                use tl3_controller_core::translate_arms::{
                    RebaseDisposition, RebasePhase, rebase_reply,
                };
                let phase = if current.phase == tl3_kernel::Phase::Rebasing {
                    RebasePhase::Rebasing
                } else {
                    RebasePhase::Other
                };
                if rebase_reply(phase, current.generation, generation) == RebaseDisposition::Ignore
                {
                    return Ok(None);
                }
                commit.rows.push(RowChange::Delete {
                    table: "rebase_conflicts".into(),
                    id: node.0.to_string(),
                });
                Event::Reply {
                    node,
                    generation,
                    reply: Reply::RebaseClean {
                        head: self.head(node, &sha)?,
                    },
                }
            }
            _ => unreachable!("the arm's own observation"),
        };
        Ok(Some(event))
    }
    fn head(&mut self, node: NodeId, sha: &str) -> Result<HeadId> {
        tl3_worktrees::CommitId::new(sha)?;
        Ok(HeadId(
            self.store.intern_commit(&node.0.to_string(), sha)?.get() as u64,
        ))
    }
}
pub(super) fn translate(controller: &mut Controller, world: &World, observation: Observation, commit: &mut Commit) -> Result<Option<Event>> {
    Shell(controller).translate(world, observation, commit)
}
