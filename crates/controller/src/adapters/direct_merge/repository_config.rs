use crate::{Controller, Repository, Result};
use serde_json::json;
use tl3_store::{Commit, RowChange, Snapshot};

/// #232/#830: repository rows are seeded by bootstrap, then reconciled before production dispatch.
impl Controller {
    pub fn reconcile_repository_delivery(
        &mut self,
        repos: &[tl3_config::Repository],
    ) -> Result<()> {
        let snapshot = self.store.load()?;
        let mut commit = Commit::new(
            format!("controller:delivery:{}", snapshot.revision.get()),
            json!({"repository_delivery":true}),
            &snapshot,
        );
        for row in snapshot.rows.iter().filter(|row| row.table == "repos") {
            let identity: Repository = crate::decode_stored(row.value.clone())?;
            let Some(configured) = repos.iter().find(|repo| {
                repo.installation_id == identity.installation
                    && repo.owner.eq_ignore_ascii_case(&identity.owner)
                    && repo.name.eq_ignore_ascii_case(&identity.name)
            }) else {
                continue;
            };
            let mut value = row.value.clone();
            if configured.delivery == tl3_config::Delivery::Direct {
                value["delivery"] = json!("direct");
            } else if let Some(object) = value.as_object_mut() {
                object.remove("delivery");
            }
            if value != row.value {
                commit.rows.push(RowChange::Put {
                    table: row.table.clone(),
                    id: row.id.clone(),
                    value,
                });
            }
        }
        if !commit.rows.is_empty() {
            self.store.commit(&commit)?;
        }
        Ok(())
    }
}

pub(super) fn direct(snapshot: &Snapshot, repo: &Repository) -> Result<bool> {
    for row in snapshot.rows.iter().filter(|row| row.table == "repos") {
        if crate::decode_stored::<Repository>(row.value.clone())? == *repo {
            let mode = row
                .value
                .get("delivery")
                .cloned()
                .map(serde_json::from_value::<tl3_config::Delivery>)
                .transpose()?;
            return Ok(mode == Some(tl3_config::Delivery::Direct));
        }
    }
    Ok(false)
}
