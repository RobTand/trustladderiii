use crate::{Controller, Error, Result, adapters::worktrees::Real, executor::Delivery};
use serde_json::json;
use tl3_kernel::NodeId;
use tl3_store::{Commit, RowChange, Snapshot};

impl Controller {
    /// Call once per branch before its first check. A replay returns the original trim head;
    /// re-read the current branch head before reporting it to the kernel.
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
        // Leave room for the writer's prose and scratch notice within GitHub's comment limit.
        const MAX_BYTES: usize = 8_192;
        let mut text = "Changes not part of this request were removed: ".to_owned();
        let tail_budget = format!(", and {} more paths.", paths.len()).len();
        let mut listed = 0;
        for path in &paths {
            let separator = if listed == 0 { "" } else { ", " };
            if text.len() + separator.len() + path.len() + 2 + tail_budget > MAX_BYTES {
                break;
            }
            text.push_str(separator);
            text.push('`');
            text.push_str(path);
            text.push('`');
            listed += 1;
        }
        let remaining = paths.len() - listed;
        match (listed, remaining) {
            (_, 0) => text.push('.'),
            (0, 1) => text.push_str("1 path, name omitted."),
            (0, _) => text.push_str(&format!("{remaining} paths, names omitted.")),
            (_, 1) => text.push_str(", and 1 more path."),
            _ => text.push_str(&format!(", and {remaining} more paths.")),
        }
        text
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tl3_store::{Epoch, Revision, WorldRow};

    fn scope_summary(paths: &[String]) -> String {
        let snapshot = Snapshot {
            revision: Revision::ZERO,
            epoch: Epoch::ZERO,
            rows: vec![WorldRow {
                table: "scope_trims".into(),
                id: "0".into(),
                value: json!({"removed_paths": paths}),
                version: Revision::ZERO,
                arrival: 0,
            }],
            pending_effects: vec![],
            credit: vec![],
            interned: vec![],
        };
        summary(&snapshot, NodeId(0)).unwrap().unwrap()
    }

    #[test]
    fn scope_summary_caps_long_removal_lists_and_counts_omitted_paths() {
        let paths: Vec<_> = (0..10_000)
            .map(|i| format!("removed/{i:05}/__init__.py"))
            .collect();
        let text = scope_summary(&paths);
        assert!(
            text.len() <= 8_192,
            "removal notice must leave comment room"
        );
        let listed = text.matches('`').count() / 2;
        assert!(listed > 0 && listed < paths.len());
        let prefix = paths[..listed]
            .iter()
            .map(|path| format!("`{path}`"))
            .collect::<Vec<_>>()
            .join(", ");
        assert_eq!(
            text,
            format!(
                "Changes not part of this request were removed: {prefix}, and {} more paths.",
                paths.len() - listed
            )
        );
    }

    #[test]
    fn scope_summary_caps_a_single_oversized_path_without_splitting_it() {
        let paths = vec!["%FF".repeat(30_000)];
        let text = scope_summary(&paths);
        assert!(
            text.len() <= 8_192,
            "one long path must not overflow the cap"
        );
        assert_eq!(
            text,
            "Changes not part of this request were removed: 1 path, name omitted."
        );
    }
}
