use super::*;
impl Controller {
    pub(crate) fn check_setup_incident(
        &self,
        snapshot: &Snapshot,
        commit: &mut Commit,
        node: NodeId,
        permissions: &[String],
    ) -> Result<()> {
        for permission in permissions
            .iter()
            .collect::<std::collections::BTreeSet<_>>()
        {
            self.check_setup_gap(snapshot, commit, node, permission)?;
        }
        Ok(())
    }
    fn check_setup_gap(
        &self,
        snapshot: &Snapshot,
        commit: &mut Commit,
        node: NodeId,
        permission: &str,
    ) -> Result<()> {
        let summary = format!(
            "Required-check setup: {}. TL3 continues with the available requirements and check runs.",
            permission
        );
        self.setup_incident(
            snapshot,
            commit,
            node,
            check_setup_incident_id(node, permission),
            summary,
        )
    }
    pub(crate) fn missing_check_incident(
        &self,
        snapshot: &Snapshot,
        commit: &mut Commit,
        node: NodeId,
        head: &str,
        required: &crate::adapters::mergify::RequiredCheck,
    ) -> Result<()> {
        let app = required
            .app
            .map_or_else(|| "any app".to_owned(), |app| format!("app {app}"));
        let summary = format!(
            "Required check `{}` from {app} has not reported on head {head}, and every check suite on that head has completed. A workflow path filter may skip it, or the requirement may name the wrong app. The enqueue waits without charging credit; this clears when the check reports.",
            required.context
        );
        self.setup_incident(
            snapshot,
            commit,
            node,
            missing_check_incident_id(node, head, required),
            summary,
        )
    }
    pub(crate) fn queue_blocked_incident(
        &self,
        snapshot: &Snapshot,
        commit: &mut Commit,
        node: NodeId,
        head: &str,
        conditions: &[String],
        failed: &[String],
    ) -> Result<()> {
        self.setup_incident(
            snapshot,
            commit,
            node,
            queue_blocked_incident_id(node, head, conditions),
            queue_blocked_summary(conditions, failed),
        )
    }
    pub(crate) fn resolve_stale_missing_checks(
        &self,
        snapshot: &Snapshot,
        world: &tl3_kernel::World,
        commit: &mut Commit,
    ) -> Result<()> {
        use crate::adapters::mergify::{Evidence, QueueFact};
        for row in snapshot
            .rows
            .iter()
            .filter(|row| row.table == "queue_facts")
        {
            let Ok(fact) = serde_json::from_value::<QueueFact>(row.value.clone()) else {
                continue;
            };
            let (head, id) = match &fact.evidence {
                Evidence::CheckMissing { head, required } => {
                    let Some(node) = fact.node(snapshot)? else {
                        continue;
                    };
                    (head, missing_check_incident_id(node, head, required))
                }
                // #447: a blocked-queue incident also ends with its head.
                Evidence::QueueBlocked {
                    head, conditions, ..
                } => {
                    let Some(node) = fact.node(snapshot)? else {
                        continue;
                    };
                    (head, queue_blocked_incident_id(node, head, conditions))
                }
                _ => continue,
            };
            let Some(node) = fact.node(snapshot)? else {
                continue;
            };
            let Some(incident) = snapshot
                .rows
                .iter()
                .find(|row| row.table == "incidents" && row.id == id && row.value["open"] == true)
            else {
                continue;
            };
            let current = &world.nodes[node
                .index()
                .ok_or(crate::Error::Invalid("node index overflow"))?];
            let interned = snapshot.interned.iter().find(|item| {
                item.kind == tl3_store::InternKind::Commit
                    && item.scope == node.0.to_string()
                    && item.value == *head
            });
            let still_head = interned.is_some_and(|item| {
                let id = Some(tl3_kernel::HeadId(item.id as u64));
                current.head == id && current.approved == id
            });
            if matches!(current.phase, tl3_kernel::Phase::Closed(_)) || !still_head {
                let mut incident: Incident = crate::decode_stored(incident.value.clone())?;
                incident.open = false;
                commit.rows.push(put("incidents", id, json!(incident)));
            }
        }
        Ok(())
    }

}
pub(crate) fn queue_blocked_incident_id(node: NodeId, head: &str, conditions: &[String]) -> String {
    use sha2::{Digest, Sha256};
    let gap = serde_json::to_string(&(head, conditions)).expect("serializable conditions");
    format!(
        "queue-blocked:{}:{:x}",
        node.0,
        Sha256::digest(gap.as_bytes())
    )
}
pub(crate) fn missing_check_incident_id(
    node: NodeId,
    head: &str,
    required: &crate::adapters::mergify::RequiredCheck,
) -> String {
    use sha2::{Digest, Sha256};
    let gap = serde_json::to_string(&(head, required)).expect("serializable check");
    format!(
        "missing-check:{}:{:x}",
        node.0,
        Sha256::digest(gap.as_bytes())
    )
}
pub(crate) fn check_setup_incident_id(node: NodeId, permission: &str) -> String {
    use sha2::{Digest, Sha256};
    let gap = format!("{:x}", Sha256::digest(permission.as_bytes()));
    format!("required-checks:{}:{gap}", node.0)
}
pub(crate) fn queue_blocked_summary(conditions: &[String], failed: &[String]) -> String {
    let quote = |text: &String| match text.split_once(": ") {
        Some((condition, note)) => format!("`{condition}` ({note})"),
        None => format!("`{text}`"),
    };
    let conditions = conditions.iter().map(quote).collect::<Vec<_>>().join(", ");
    let failed = if failed.is_empty() {
        "No check on the head failed".to_owned()
    } else {
        format!(
            "These checks on the head failed and are outside TL3's required set: {}",
            failed
                .iter()
                .map(|name| format!("`{name}`"))
                .collect::<Vec<_>>()
                .join(", ")
        )
    };
    format!(
        "Mergify is waiting for queue conditions that no running check can meet: {conditions}. Every check suite on the head has completed. {failed}. The enqueue waits; no credit is charged. This ends with the head."
    )
}
