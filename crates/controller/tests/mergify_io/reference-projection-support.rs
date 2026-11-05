// Frozen e3292d91 QueueFact behavior for the reference projection shell.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct QueueFact {
    pub repository: Repository,
    pub number: u64,
    pub evidence: Evidence,
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
            if super::request(snapshot, node)?.repo == self.repository {
                return Ok(Some(node));
            }
        }
        Ok(None)
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
    fn unresolved_hold(&self, snapshot: &Snapshot) -> Result<bool> {
        if !matches!(
            self.evidence,
            Evidence::DequeuedByPerson { .. }
                | Evidence::ClosedByPerson { .. }
                | Evidence::DequeuedByCommand { .. }
        ) {
            return Ok(false);
        }
        let Some(node) = self.node(snapshot)? else {
            return Ok(true);
        };
        let observed = snapshot
            .rows
            .iter()
            .find(|row| row.table == "queue_facts" && row.id == self.identity());
        let resumed = snapshot
            .rows
            .iter()
            .find(|row| row.table == "queue_resumes" && row.id == node.0.to_string());
        Ok(!observed
            .zip(resumed)
            .is_some_and(|(observed, resumed)| resumed.version > observed.version))
    }
}

fn person_facts(
    repo: &Repository,
    number: u64,
    events: &[tl3_github::IssueActivity],
    merged: bool,
) -> Vec<QueueFact> {
    let events: Vec<_> = events
        .iter()
        .map(|event| facts::IssueEvent {
            id: event.id,
            actor: actor(event.actor.as_ref()),
            kind: event_kind(
                &event.event,
                event.label.as_ref().map(|label| label.name.as_str()),
            ),
        })
        .collect();
    facts::person_facts(&events, merged)
        .map(|fact| QueueFact {
            repository: repo.clone(),
            number,
            evidence: match fact {
                facts::PersonFact::Dequeued { event } => Evidence::DequeuedByPerson { event },
                facts::PersonFact::Closed { event } => Evidence::ClosedByPerson { event },
            },
        })
        .collect()
}
