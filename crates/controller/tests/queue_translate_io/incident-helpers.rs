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
}
fn queue_blocked_incident_id(node: NodeId, head: &str, conditions: &[String]) -> String {
    use sha2::{Digest, Sha256};
    let gap = serde_json::to_string(&(head, conditions)).expect("serializable conditions");
    format!(
        "queue-blocked:{}:{:x}",
        node.0,
        Sha256::digest(gap.as_bytes())
    )
}
fn missing_check_incident_id(
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
fn check_setup_incident_id(node: NodeId, permission: &str) -> String {
    use sha2::{Digest, Sha256};
    let gap = format!("{:x}", Sha256::digest(permission.as_bytes()));
    format!("required-checks:{}:{gap}", node.0)
}
fn queue_blocked_summary(conditions: &[String], failed: &[String]) -> String {
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
