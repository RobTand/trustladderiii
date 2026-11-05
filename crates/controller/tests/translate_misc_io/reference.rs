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
        observation: Observation,
        commit: &mut Commit,
    ) -> Result<Option<Event>> {
        match observation {
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
        }
    }
}
pub(super) fn translate(
    controller: &mut Controller,
    snapshot: &Snapshot,
    observation: Observation,
    commit: &mut Commit,
) -> Result<Option<Event>> {
    Shell(controller).translate(snapshot, observation, commit)
}
