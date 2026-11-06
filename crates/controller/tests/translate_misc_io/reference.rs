use super::*;
struct Shell<'a>(&'a mut Controller);
impl std::ops::Deref for Shell<'_> {
    type Target = Controller;
    fn deref(&self) -> &Controller {
        self.0
    }
}
impl std::ops::DerefMut for Shell<'_> {
    fn deref_mut(&mut self) -> &mut Controller {
        self.0
    }
}
impl Shell<'_> {
    fn translate(
        &mut self,
        snapshot: &Snapshot,
        world: &mut World,
        observation: Observation,
        commit: &mut Commit,
    ) -> Result<Option<Event>> {
        if let Observation::GitHubComment {
            repository,
            issue,
            comment,
        } = &observation
            && self.operations_comment(snapshot, repository, *issue, comment, commit)?
        {
            return Ok(None);
        }

        match observation {
            Observation::Prerequisite(fact) => {
                self.external_dependency_fact(snapshot, world, fact, commit)?;
                return Ok(None);
            }
            // Older controllers queued ScratchSaved after writing it with completion.
            Observation::Classified { key, reply } => {
                return self.classified(snapshot, world, &key, reply, commit);
            }
            Observation::Postmortem {
                key,
                reply,
                failure,
                judge_failure,
            } => {
                self.postmortem_finished(snapshot, &key, reply, failure, judge_failure, commit)?;
                return Ok(None);
            }
            Observation::LinkedRequest {
                repository,
                issue,
                original,
            } => {
                return self.linked_request(snapshot, repository, issue, original, commit);
            }
            Observation::Steering { node, command } => {
                return self.command_event(snapshot, world, node, command, commit);
            }
            Observation::GitHubComment {
                repository,
                issue,
                comment,
            } => {
                if comment.user.id == self.bootstrap.rob
                    && self.bootstrap.repos.contains(&repository)
                {
                    let id = format!(
                        "{}/{}/{}/{}",
                        repository.owner.to_ascii_lowercase(),
                        repository.name.to_ascii_lowercase(),
                        repository.installation,
                        comment.id
                    );
                    if !self.store.comment_retired(&id)?
                        && !snapshot
                            .rows
                            .iter()
                            .any(|row| row.table == "github_comments" && row.id == id)
                    {
                        let value =
                            json!({"repository": repository, "issue": issue, "comment": comment});
                        commit
                            .rows
                            .push(put("github_comments", id.clone(), value.clone()));
                        commit.rows.push(put("unclassified_comments", id, value));
                    }
                }
                return Ok(None);
            }
            _ => unreachable!("outside the assigned observation shapes"),
        }
    }
}
pub(super) fn translate(
    controller: &mut Controller,
    snapshot: &Snapshot,
    world: &mut World,
    observation: Observation,
    commit: &mut Commit,
) -> Result<Option<Event>> {
    Shell(controller).translate(snapshot, world, observation, commit)
}
