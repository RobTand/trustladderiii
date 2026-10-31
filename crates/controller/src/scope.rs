use crate::{Controller, Error, Result, adapters::worktrees::Real, executor::Delivery};
use serde_json::json;
use tl3_kernel::NodeId;
use tl3_store::{Commit, RowChange, Snapshot};

impl Controller {
    /// Call before the proposed head's first check; report only the returned head to the kernel.
    /// Git's receipt covers a crash before the summary row commits, so retry this same call.
    pub fn trim_scope(
        &mut self,
        adapter: &Real,
        delivery: &Delivery,
        protected: &[String],
    ) -> Result<tl3_worktrees::ScopeTrim> {
        let snapshot = self.store.load()?;
        if delivery.epoch != snapshot.epoch.get() {
            return Err(Error::Invalid(
                "scope trim epoch does not match the controller",
            ));
        }
        let trim = adapter.trim_scope(delivery, protected)?;
        let paths: Vec<_> = trim
            .removed_paths
            .iter()
            .map(|p| {
                p.as_os_str()
                    .as_encoded_bytes()
                    .iter()
                    .map(|b| {
                        if b.is_ascii_alphanumeric() || b"._-/".contains(b) {
                            char::from(*b).to_string()
                        } else {
                            format!("%{b:02X}")
                        }
                    })
                    .collect::<String>()
            })
            .collect();
        let id = delivery.node().0.to_string();
        let value = json!({"head":trim.head.as_str(), "removed_paths":paths});
        if let Some(row) = snapshot
            .rows
            .iter()
            .find(|r| r.table == "scope_trims" && r.id == id)
        {
            if row.value != value {
                return Err(Error::Invalid("scope trim receipt changed"));
            }
            return Ok(trim);
        }
        let mut commit = Commit::new(
            format!("scope-trim:{id}"),
            json!({"scope_trim":id}),
            &snapshot,
        );
        commit.rows.push(RowChange::Put {
            table: "scope_trims".into(),
            id,
            value,
        });
        self.store.commit(&commit)?;
        Ok(trim)
    }
}

pub(crate) fn summary(snapshot: &Snapshot, node: NodeId) -> Result<Option<String>> {
    let Some(row) = snapshot
        .rows
        .iter()
        .find(|r| r.table == "scope_trims" && r.id == node.0.to_string())
    else {
        return Ok(None);
    };
    let paths: Vec<String> = serde_json::from_value(row.value["removed_paths"].clone())?;
    Ok((!paths.is_empty()).then(|| {
        format!(
            "Changes not part of this request were removed: {}.",
            paths.join(", ")
        )
    }))
}
