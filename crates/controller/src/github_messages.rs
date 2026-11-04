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
            issue,
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
        let effect_key = tl3_github::GitHub::comment_effect_key(&comment.body)
            .ok_or(Error::Invalid("message marker missing"))?;
        let id = format!(
            "{}/{}/{}/{}/{}",
            repository.owner.to_ascii_lowercase(),
            repository.name.to_ascii_lowercase(),
            repository.installation,
            issue,
            effect_key
        );
        let mut commit = Commit::new(format!("observe:{key}"), json!(observation), snapshot);
        commit.rows.push(put(TABLE, id, json!(observation)));
        self.store.commit(&commit)?;
        self.retry_held_effects(Upstream::GitHub);
        Ok(true)
    }

    pub(crate) fn reconcile_messages(&mut self, snapshot: &Snapshot) -> Result<bool> {
        // Older versions keyed work by comment ID. Fold those durable rows before
        // doing any HTTP work, so restart/upgrade cannot retry once per copy.
        let mut migration = Commit::new(
            format!("coalesce-github-messages:{}", snapshot.revision.get()),
            json!({"coalesce_messages":true}),
            snapshot,
        );
        for row in snapshot.rows.iter().filter(|row| row.table == TABLE) {
            let Observation::GitHubComment {
                repository,
                issue,
                comment,
            } = crate::decode_stored(row.value.clone())?
            else {
                return Err(Error::Invalid("invalid message reconciliation record"));
            };
            let key = tl3_github::GitHub::comment_effect_key(&comment.body)
                .ok_or(Error::Invalid("cleanup marker missing"))?;
            let id = format!(
                "{}/{}/{}/{}/{}",
                repository.owner.to_ascii_lowercase(),
                repository.name.to_ascii_lowercase(),
                repository.installation,
                issue,
                key
            );
            if row.id != id {
                migration.rows.push(RowChange::Delete {
                    table: TABLE.into(),
                    id: row.id.clone(),
                });
                migration.rows.push(put(TABLE, id, row.value.clone()));
            }
        }
        if !migration.rows.is_empty() {
            self.store.commit(&migration)?;
            return Ok(true);
        }
        for row in snapshot.rows.iter().filter(|row| row.table == TABLE) {
            let Observation::GitHubComment {
                repository,
                issue,
                comment,
            } = crate::decode_stored(row.value.clone())?
            else {
                return Err(Error::Invalid("invalid message reconciliation record"));
            };
            let cleanup_key = format!("github-cleanup:{}", row.id);
            if crate::looks::outage_waiting(snapshot, &cleanup_key) {
                continue;
            }
            if let Err(error) =
                self.adapters
                    .get(Upstream::GitHub)?
                    .reconcile_message(&repository, issue, &comment)
            {
                let mut commit = Commit::new(
                    format!(
                        "github-cleanup-failed:{}:{}",
                        row.id,
                        snapshot.revision.get()
                    ),
                    json!({"cleanup_failed":row.id}),
                    snapshot,
                );
                let key = tl3_github::GitHub::comment_effect_key(&comment.body)
                    .ok_or(Error::Invalid("cleanup marker missing"))?;
                if let Some(effect) = self.store.effect(&key)? {
                    let delivery = crate::decode_stored(effect.payload)?;
                    self.maintenance_failure(
                        snapshot,
                        &delivery,
                        &cleanup_key,
                        "message_cleanup",
                        error,
                        &mut commit,
                    )?;
                } else {
                    // A marked message can outlive its historical local effect.
                    self.input_failed(
                        snapshot,
                        &cleanup_key,
                        Some(crate::input_errors::IssueTarget {
                            repository: repository.clone(),
                            issue,
                        }),
                        "GitHub message cleanup",
                        None,
                        Upstream::GitHub,
                        error,
                        &mut commit,
                    )?;
                    for change in &mut commit.rows {
                        if let RowChange::Put { table, value, .. } = change
                            && table == "incidents"
                        {
                            value["effect"] = json!(cleanup_key);
                        }
                    }
                }
                self.store.commit(&commit)?;
                return Ok(true);
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
            commit.rows.push(RowChange::Delete {
                table: "github_message_failures".into(),
                id: row.id.clone(),
            });
            self.finish_maintenance(snapshot, &cleanup_key, &mut commit)?;
            self.store.commit(&commit)?;
            return Ok(true);
        }
        Ok(false)
    }
}
