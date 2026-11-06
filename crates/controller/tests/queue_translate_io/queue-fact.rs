use super::*;
use tl3_mergify::decide::translate::Kind;
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Evidence {
    ChecksUnavailable {
        permissions: Vec<String>,
    },
    /// Every check suite on the head completed without this required context (Q52).
    CheckMissing {
        head: String,
        required: RequiredCheck,
    },
    /// #447: Mergify waits on queue conditions that nothing still running can meet.
    QueueBlocked {
        head: String,
        conditions: Vec<String>,
        failed: Vec<String>,
    },
    /// The context of an open missing-check incident has reported on that head (Q52).
    CheckReported {
        head: String,
        required: RequiredCheck,
    },
    Merged {
        head: String,
        checks_passed: bool,
    },
    CiFailed {
        check: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        completed_at: Option<String>,
        head: String,
        infra: bool,
        #[serde(default)]
        head_check: bool,
        #[serde(default)]
        details: crate::findings::QueueCiEvidence,
    },
    Conflict {
        check: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        completed_at: Option<String>,
        head: String,
    },
    DequeuedByCommand {
        check: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        completed_at: Option<String>,
        head: String,
    },
    DequeuedByPerson {
        event: u64,
    },
    ClosedByPerson {
        event: u64,
    },
    PushByOther {
        head: String,
    },
}
impl Evidence {
    /// The evidence without its payload, as the core decides on it.
    pub(crate) fn kind(&self) -> Kind {
        match self {
            Self::ChecksUnavailable { .. } => Kind::ChecksUnavailable,
            Self::CheckMissing { .. } => Kind::CheckMissing,
            Self::QueueBlocked { .. } => Kind::QueueBlocked,
            Self::CheckReported { .. } => Kind::CheckReported,
            Self::Merged { checks_passed, .. } => Kind::Merged {
                checks_passed: *checks_passed,
            },
            Self::CiFailed {
                head_check, infra, ..
            } => Kind::CiFailed {
                head_check: *head_check,
                infra: *infra,
            },
            Self::Conflict { .. } => Kind::Conflict,
            Self::DequeuedByCommand { .. } => Kind::DequeuedByCommand,
            Self::DequeuedByPerson { .. } => Kind::DequeuedByPerson,
            Self::ClosedByPerson { .. } => Kind::ClosedByPerson,
            Self::PushByOther { .. } => Kind::PushByOther,
        }
    }
}

impl QueueFact {
    pub(crate) fn node(&self, snapshot: &Snapshot) -> Result<Option<NodeId>> {
        for row in snapshot.rows.iter().filter(|row| {
            row.table == "pull_requests" && row.value["number"].as_u64() == Some(self.number)
        }) {
            let node = NodeId(
                row.id
                    .parse()
                    .map_err(|_| Error::Invalid("invalid PR binding"))?,
            );
            if crate::request(snapshot, node)?.repo == self.repository {
                return Ok(Some(node));
            }
        }
        Ok(None)
    }
    pub(crate) fn recorded(&self, snapshot: &Snapshot) -> Result<bool> {
        if let Evidence::CheckMissing { head, required }
        | Evidence::CheckReported { head, required } = &self.evidence
        {
            let Some(node) = self.node(snapshot)? else {
                return Ok(false);
            };
            let id = crate::looks::missing_check_incident_id(node, head, required);
            let incident = snapshot
                .rows
                .iter()
                .find(|row| row.table == "incidents" && row.id == id);
            // A raised incident stays deduplicated after a clear; a report matters only while open.
            return Ok(match self.evidence {
                Evidence::CheckMissing { .. } => incident.is_some(),
                _ => !incident.is_some_and(|row| row.value["open"] == true),
            });
        }
        if let Evidence::QueueBlocked {
            head, conditions, ..
        } = &self.evidence
        {
            let Some(node) = self.node(snapshot)? else {
                return Ok(false);
            };
            let id = crate::looks::queue_blocked_incident_id(node, head, conditions);
            return Ok(snapshot
                .rows
                .iter()
                .any(|row| row.table == "incidents" && row.id == id));
        }
        if let Evidence::ChecksUnavailable { permissions } = &self.evidence {
            let Some(node) = self.node(snapshot)? else {
                return Ok(false);
            };
            return Ok(permissions.iter().all(|permission| {
                let id = crate::looks::check_setup_incident_id(node, permission);
                snapshot.rows.iter().any(|row| {
                    row.table == "incidents" && row.id == id && row.value["open"] == true
                })
            }));
        }
        let identity = self.identity();
        Ok(snapshot
            .rows
            .iter()
            .any(|row| row.table == "queue_facts" && row.id == identity))
    }
    pub(crate) fn identity(&self) -> String {
        let mut evidence = match &self.evidence {
            Evidence::ChecksUnavailable { permissions } => {
                let mut gaps = permissions.clone();
                gaps.sort();
                gaps.dedup();
                format!(
                    "required-checks-setup:{}",
                    serde_json::to_string(&gaps).unwrap()
                )
            }
            Evidence::CheckMissing { head, required } => {
                format!(
                    "missing-check:{head}:{}",
                    serde_json::to_string(required).unwrap()
                )
            }
            Evidence::CheckReported { head, required } => {
                format!(
                    "check-reported:{head}:{}",
                    serde_json::to_string(required).unwrap()
                )
            }
            Evidence::QueueBlocked {
                head, conditions, ..
            } => format!(
                "queue-blocked:{head}:{}",
                serde_json::to_string(conditions).unwrap()
            ),
            Evidence::Merged { head, .. } => format!("merged:{head}"),
            Evidence::CiFailed {
                check, head_check, ..
            } => {
                format!("{}:{check}", if *head_check { "head-ci" } else { "ci" })
            }
            Evidence::Conflict { check, .. } => format!("conflict:{check}"),
            Evidence::DequeuedByCommand { check, .. } => format!("command-dequeued:{check}"),
            Evidence::DequeuedByPerson { event } => format!("dequeued:{event}"),
            Evidence::ClosedByPerson { event } => format!("closed:{event}"),
            Evidence::PushByOther { head } => format!("push:{head}"),
        };
        // Mergify can reuse one check ID across successive queue attempts.
        if let Evidence::CiFailed {
            completed_at: Some(time),
            ..
        }
        | Evidence::Conflict {
            completed_at: Some(time),
            ..
        }
        | Evidence::DequeuedByCommand {
            completed_at: Some(time),
            ..
        } = &self.evidence
        {
            evidence.push(':');
            evidence.push_str(time);
        }
        format!(
            "{}/{}/{}/{}/{evidence}",
            self.repository.owner.to_ascii_lowercase(),
            self.repository.name.to_ascii_lowercase(),
            self.repository.installation,
            self.number
        )
    }
}
