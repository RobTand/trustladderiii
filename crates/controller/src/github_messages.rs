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
                let failures = snapshot
                    .rows
                    .iter()
                    .find(|r| r.table == "github_message_failures" && r.id == row.id)
                    .and_then(|r| r.value.as_u64())
                    .unwrap_or(0)
                    .saturating_add(1);
                let mut commit = Commit::new(
                    format!(
                        "github-cleanup-failed:{}:{}",
                        row.id,
                        snapshot.revision.get()
                    ),
                    json!({"cleanup_failed":row.id}),
                    snapshot,
                );
                commit.rows.push(put(
                    "github_message_failures",
                    row.id.clone(),
                    json!(failures % 2),
                ));
                if failures >= 2 || matches!(error, Error::Permanent { .. }) {
                    let key = tl3_github::GitHub::comment_effect_key(&comment.body)
                        .ok_or(Error::Invalid("cleanup marker missing"))?;
                    if let Some(effect) = self.store.effect(&key)? {
                        let delivery = crate::decode_stored(effect.payload)?;
                        self.maintenance_incident(snapshot, &delivery, &cleanup_key, "message_cleanup",
                            format!("GitHub message cleanup failed for {}/{} issue {}. Clear to retry deduplication.",repository.owner,repository.name,issue),&mut commit)?;
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
            commit.rows.push(RowChange::Delete {
                table: "notice_incidents".into(),
                id: cleanup_key,
            });
            self.store.commit(&commit)?;
            return Ok(true);
        }
        Ok(false)
    }
}
