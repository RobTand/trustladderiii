use crate::{
    Controller, Error, Result,
    executor::{Observation, Upstream},
    put,
};
use serde_json::json;
use tl3_store::{Commit, RowChange, Snapshot};

const TABLE: &str = "github_message_reconciliations";

impl Controller {
    pub(crate) fn queue_message_reconciliation(
        &mut self,
        snapshot: &Snapshot,
        key: &str,
        observation: &Observation,
    ) -> Result<bool> {
        let Observation::GitHubComment {
            repository,
            comment,
            ..
        } = observation
        else {
            return Ok(false);
        };
        if !self.bootstrap.github_repositories().contains(repository)
            || tl3_github::GitHub::comment_effect_key(&comment.body).is_none()
            || self
                .adapters
                .get(Upstream::GitHub)?
                .app_login(repository.installation)
                .as_deref()
                != Some(comment.user.login.as_str())
        {
            return Ok(false);
        }
        let id = format!(
            "{}/{}/{}/{}",
            repository.owner.to_ascii_lowercase(),
            repository.name.to_ascii_lowercase(),
            repository.installation,
            comment.id
        );
        let mut commit = Commit::new(format!("observe:{key}"), json!(observation), snapshot);
        commit.rows.push(put(TABLE, id, json!(observation)));
        self.store.commit(&commit)?;
        self.retry_held_effects(Upstream::GitHub);
        Ok(true)
    }

    pub(crate) fn reconcile_messages(&mut self, snapshot: &Snapshot) -> Result<bool> {
        for row in snapshot.rows.iter().filter(|row| row.table == TABLE) {
            let Observation::GitHubComment {
                repository,
                issue,
                comment,
            } = crate::decode_stored(row.value.clone())?
            else {
                return Err(Error::Invalid("invalid message reconciliation record"));
            };
            if let Err(error) =
                self.adapters
                    .get(Upstream::GitHub)?
                    .reconcile_message(&repository, issue, &comment)
            {
                eprintln!("GitHub message reconciliation failed: {error}");
                continue;
            }
            let mut commit = Commit::new(
                format!(
                    "github-message-reconciled:{}:{}",
                    row.id,
                    snapshot.revision.get()
                ),
                json!({"message_reconciled":row.id}),
                snapshot,
            );
            commit.rows.push(RowChange::Delete {
                table: TABLE.into(),
                id: row.id.clone(),
            });
            self.store.commit(&commit)?;
            return Ok(true);
        }
        Ok(false)
    }
}
