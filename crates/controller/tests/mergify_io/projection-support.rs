// Real private projection helpers; source-pinned by recovery_projections.rs.

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Request {
    repo: Repository,
    issue: u64,
    #[serde(default)]
    title: String,
    issue_id: i64,
    #[serde(default)]
    source: Option<String>,
    #[serde(default)]
    proposed: Option<adoption::ProposedPullRequest>,
    /// P0–P3 from the issue, read once at intake (#416); `None` for unranked
    /// and older rows, and omitted then so the row keeps its older shape (#375).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    priority: Option<u8>,
}

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
            if self::request(snapshot, node)?.repo == self.repository {
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
        if !completion_core::hold_candidate(completion_hold(&self.evidence)) {
            return Ok(false);
        }
        let Some(node) = self.node(snapshot)? else {
            return Ok(completion_core::unresolved_hold(None));
        };
        let observed = snapshot
            .rows
            .iter()
            .find(|row| row.table == "queue_facts" && row.id == self.identity());
        let resumed = snapshot
            .rows
            .iter()
            .find(|row| row.table == "queue_resumes" && row.id == node.0.to_string());
        Ok(completion_core::unresolved_hold(observed.zip(resumed).map(
            |(observed, resumed)| (observed.version.get(), resumed.version.get()),
        )))
    }
}

pub(crate) fn raw_snapshot_decode(snapshot: &Snapshot) -> Result<World> {
    let meta = snapshot
        .rows
        .iter()
        .find(|r| r.table == "kernel" && r.id == "meta")
        .ok_or(Error::Invalid(
            "controller database is uninitialized; start tl3d first",
        ))?;
    if meta.value["format"] != 1 {
        return Err(Error::Invalid("unsupported snapshot format"));
    }
    let mut value = meta.value["world"].clone();
    for table in ["nodes", "gates", "file_sets"] {
        let mut entries = BTreeMap::new();
        for row in snapshot.rows.iter().filter(|r| r.table == table) {
            let index = row
                .id
                .parse::<usize>()
                .map_err(|_| Error::Invalid("invalid row index"))?;
            if entries.insert(index, row.value.clone()).is_some() {
                return Err(Error::Invalid("duplicate row index"));
            }
        }
        let mut values = Vec::new();
        for (index, item) in entries {
            if index != values.len() {
                return Err(Error::Invalid("gap in kernel rows"));
            }
            values.push(item);
        }
        value[table] = Value::Array(values);
    }
    let world: World = self::decode_stored(value)?;
    if world
        .file_sets()
        .iter()
        .any(|set| set.windows(2).any(|pair| pair[0] >= pair[1]))
    {
        return Err(Error::Invalid(
            "stored footprint must be sorted and deduplicated",
        ));
    }
    if !world.valid() || !world.is_settled() {
        return Err(Error::Invalid("invalid stored kernel world"));
    }
    Ok(world)
}

fn raw_request(snapshot: &Snapshot, node: NodeId) -> Result<Request> {
    decode_stored(
        snapshot
            .rows
            .iter()
            .find(|r| r.table == "requests" && r.id == node.0.to_string())
            .ok_or(Error::Invalid("missing request binding"))?
            .value
            .clone(),
    )
}

fn raw_decode_stored<T: serde::de::DeserializeOwned>(value: Value) -> Result<T> {
    serde_json::from_value(value).map_err(Error::StoredJson)
}

fn raw_approved_head(snapshot: &Snapshot, approved: Option<tl3_kernel::HeadId>) -> Result<String> {
    let Some(head) = approved else {
        return Ok(String::new());
    };
    snapshot
        .interned
        .iter()
        .find(|entry| entry.kind == InternKind::Commit && entry.id == head.0 as i64)
        .map(|entry| entry.value.clone())
        .ok_or(Error::Invalid("approved head is not interned"))
}

pub(crate) fn heal_phase(phase: Option<tl3_kernel::Phase>) -> bool {
    matches!(
        phase,
        Some(tl3_kernel::Phase::Publishing | tl3_kernel::Phase::Queued)
    )
}

fn enqueue(delivery: &Delivery) -> Result<(u64, &str)> {
    if !matches!(
        delivery.output,
        Out::Effect {
            kind: EffectKind::Enqueue,
            ..
        }
    ) {
        return Err(Error::Invalid("unsupported Mergify effect"));
    }
    Ok((
        delivery
            .pull_request
            .ok_or(Error::Invalid("enqueue needs a PR number"))?,
        delivery
            .head_sha
            .as_deref()
            .ok_or(Error::Invalid("enqueue needs an approved head"))?,
    ))
}

fn actor(actor: Option<&Actor>) -> facts::Actor {
    facts::Actor::parse(actor.map(|actor| actor.login.as_str()))
}

fn event_kind(event: &str, label: Option<&str>) -> facts::EventKind {
    match event {
        "unlabeled" => facts::EventKind::Unlabeled {
            queue_label: label == Some(QUEUE_LABEL),
        },
        "closed" => facts::EventKind::Closed,
        "merged" => facts::EventKind::Merged,
        _ => facts::EventKind::Other,
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

fn completion_hold(evidence: &Evidence) -> completion_core::HoldEvidence {
    match evidence {
        Evidence::DequeuedByPerson { .. } | Evidence::ClosedByPerson { .. } => {
            completion_core::HoldEvidence::Person
        }
        Evidence::DequeuedByCommand { .. } => completion_core::HoldEvidence::Command,
        _ => completion_core::HoldEvidence::Other,
    }
}
