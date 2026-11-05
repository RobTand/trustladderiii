use crate::{
    Controller, Error, Repository, Result,
    executor::{Observation, Upstream},
    looks::{Incident, IncidentSignature},
    put,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tl3_kernel::NodeId;
use tl3_store::{Commit, RowChange, Snapshot};

pub(crate) const COUNTER_HEADROOM: &str = "kernel_counter_headroom";
pub(crate) const PENDING_KERNEL: &str = "pending_kernel_events";

#[derive(Serialize, Deserialize)]
pub(crate) enum PendingKernelInput {
    Event(tl3_kernel::Event),
    Completion {
        key: String,
        completion: crate::executor::Completion,
    },
    CancelAbsent {
        key: String,
    },
    Replacement {
        original: NodeId,
    },
    PriorityReread {
        id: String,
        row: crate::Reconcile,
        now: u64,
    },
    IssueChange(crate::IssueChange),
    Intake {
        issue: crate::LabelledIssue,
        proposed: Option<crate::adoption::ProposedPullRequest>,
    },
}

/// After a halt, direct producers join the retained queue until it drains.
pub(crate) fn kernel_backlog(snapshot: &Snapshot) -> bool {
    snapshot
        .rows
        .iter()
        .any(|row| row.table == "incidents" && row.value["signature"]["class"] == COUNTER_HEADROOM)
        && snapshot
            .rows
            .iter()
            .any(|row| row.table == "inbox" || row.table == PENDING_KERNEL)
}

fn counter_incident(row: &tl3_store::WorldRow) -> bool {
    row.table == "incidents"
        && row.value["open"] == true
        && row.value["signature"]["class"] == COUNTER_HEADROOM
}

fn retain_input(commit: &mut Commit) {
    if !commit.result.is_object() {
        commit.result = json!({});
    }
    commit.result["kernel_deferred"] = json!(true);
}

pub(crate) fn deferred(commit: &Commit) -> bool {
    commit.result["kernel_deferred"] == true
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct IssueTarget {
    pub repository: Repository,
    pub issue: u64,
}

/// A reporting destination exists even when intake has not created a request.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct IncidentNotice {
    pub key: String,
    pub target: IssueTarget,
    pub title: String,
    pub summary: String,
    pub id: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct InputFailure {
    scope: String,
    pub target: Option<IssueTarget>,
    title: String,
    posted: bool,
    attempted_epoch: Option<i64>,
    #[serde(default)]
    post_failure: Option<String>,
    #[serde(default)]
    retry_after_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) memory_version: Option<i64>,
}

impl InputFailure {
    pub(crate) fn memory(title: &str, lineage: &str, version: i64) -> Self {
        Self {
            scope: lineage.into(),
            target: None,
            title: title.into(),
            posted: false,
            attempted_epoch: None,
            post_failure: None,
            retry_after_ms: None,
            memory_version: Some(version),
        }
    }

    pub(crate) fn memory_lineage(&self, lineage: &str) -> bool {
        self.scope == lineage
    }

    pub(crate) fn with_memory_lineage(&self, lineage: &str, version: i64) -> Self {
        let mut failure = self.clone();
        failure.scope = lineage.into();
        failure.memory_version = Some(version);
        failure
    }
}

pub(crate) fn recoverable(error: &Error) -> bool {
    !matches!(
        error,
        Error::Store(_) | Error::Config(_) | Error::StoredJson(_)
    )
}

fn post_backoff_ms(class: &str) -> u64 {
    match class {
        "transport" | "quota" => 30_000,
        _ => 300_000,
    }
}

#[cfg(test)]
#[derive(Default)]
pub(crate) struct RefusalInjection {
    pub at: Option<usize>,
    pub visited: usize,
}

impl Controller {
    /// State-table "Counter headroom": tests refuse immediately before every
    /// real kernel step, including fan-out, and at the final store commit.
    /// This hook never substitutes a model for translation or for the store.
    pub(crate) fn input_checkpoint(
        &self,
        snapshot: &Snapshot,
        commit: &mut Commit,
    ) -> Result<bool> {
        #[cfg(test)]
        {
            let mut injection = self.refusal_injection.borrow_mut();
            let visited = injection.visited;
            injection.visited += 1;
            if injection.at == Some(visited) {
                self.counter_exhausted(
                    snapshot,
                    commit,
                    None,
                    format!("kernel counter headroom exhausted: test checkpoint {visited}"),
                )?;
                return Ok(false);
            }
        }
        let _ = (snapshot, commit);
        Ok(true)
    }

    /// State-table "Counter headroom" / Q27: a persisted counter incident halts kernel
    /// inputs across wakes and restarts. Only its authorized clearance can pass
    /// the retained head of the queue. A repair must also restore headroom.
    pub(crate) fn pause_kernel_inputs(&mut self, snapshot: &Snapshot) -> Result<Option<bool>> {
        let incident = snapshot.rows.iter().find(|row| counter_incident(row));
        let pending = snapshot
            .rows
            .iter()
            .any(|row| row.table == "inbox" || row.table == PENDING_KERNEL);
        if incident.is_none() && !pending {
            return Ok(None);
        }
        if let Some(incident) = incident {
            for row in snapshot.rows.iter().filter(|row| row.table == "inbox") {
                let observation: Observation = crate::decode_stored(row.value.clone())?;
                let Observation::GitHubComment {
                    repository,
                    issue,
                    comment,
                } = observation
                else {
                    continue;
                };
                if comment.user.id != self.bootstrap.rob
                    || comment.body.trim() != format!("clear incident {}", incident.id)
                {
                    continue;
                }
                let mut commit =
                    Commit::new(format!("consume:{}", row.id), row.value.clone(), snapshot);
                if self.operations_comment(snapshot, &repository, issue, &comment, &mut commit)? {
                    commit.rows.push(RowChange::Delete {
                        table: "inbox".into(),
                        id: row.id.clone(),
                    });
                    self.store.commit(&commit)?;
                    return Ok(Some(true));
                }
            }
            return Ok(Some(false));
        }
        let world = crate::snapshot::decode(snapshot)?;
        let mut commit = Commit::new(
            format!("kernel-headroom:{}", snapshot.revision.get()),
            json!({"kernel_headroom": false}),
            snapshot,
        );
        if self.kernel_headroom(snapshot, &world, &mut commit)? {
            return Ok(None);
        }
        self.store.commit(&commit)?;
        Ok(Some(true))
    }

    /// State-table "Counter headroom": translation, interning, fan-out and
    /// consuming the input have one durability boundary. Refusal rolls back
    /// before the independent incident transaction. No staged business row is
    /// allowed into that transaction.
    pub(crate) fn consume_kernel_input(
        &mut self,
        snapshot: &Snapshot,
        row: &tl3_store::WorldRow,
    ) -> Result<bool> {
        self.store.begin_input()?;
        let holds = self.held_effects.clone();
        let postmortem_since = self.postmortem_since;
        let attempted = self.prepare_kernel_input(snapshot, row);
        let result = match attempted {
            Ok(mut commit) if !deferred(&commit) => {
                commit.rows.push(RowChange::Delete {
                    table: row.table.clone(),
                    id: row.id.clone(),
                });
                self.store
                    .commit(&commit)
                    .map_err(Error::from)
                    .and_then(|_| {
                        // Exercise rollback after SQL writes as well as before steps.
                        self.input_checkpoint(snapshot, &mut commit)?;
                        Ok((!deferred(&commit), commit))
                    })
            }
            Ok(commit) => Ok((false, commit)),
            Err(error) => Err(error),
        };
        let accepted = result.as_ref().is_ok_and(|(accepted, _)| *accepted);
        self.store.finish_input(accepted)?;
        if !accepted {
            self.held_effects = holds;
            self.postmortem_since = postmortem_since;
        }
        let (accepted, attempted) = result?;
        if !accepted {
            let current = self.store.load()?;
            let mut incident = Commit::new(
                format!("kernel-refusal:{}:{}", row.id, current.revision.get()),
                json!({"kernel_refusal": row.id}),
                &current,
            );
            incident.result = json!({"kernel_deferred":true});
            // This projection is the only persistence path after rollback.
            incident
                .rows
                .extend(attempted.rows.into_iter().filter(|change| {
                    matches!(change, RowChange::Put {table, value, ..}
                    if table == "incidents" && value["signature"]["class"] == COUNTER_HEADROOM)
                }));
            if !incident.rows.is_empty() {
                self.store.commit(&incident)?;
            }
        }
        if accepted && row.table == PENDING_KERNEL {
            let input: PendingKernelInput = crate::decode_stored(row.value.clone())?;
            if let PendingKernelInput::Completion { key, .. }
            | PendingKernelInput::CancelAbsent { key } = input
                && let Some(effect) = snapshot.pending_effects.iter().find(|e| e.key == key)
            {
                self.remove_hold(&crate::decode_stored(effect.payload.clone())?);
            }
        }
        Ok(accepted)
    }

    fn prepare_kernel_input(
        &mut self,
        snapshot: &Snapshot,
        row: &tl3_store::WorldRow,
    ) -> Result<Commit> {
        let mut world = crate::snapshot::decode(snapshot)?;
        let mut commit = Commit::new(format!("consume:{}", row.id), row.value.clone(), snapshot);
        if row.table == "inbox" {
            let observation: Observation = crate::decode_stored(row.value.clone())?;
            match self.translate(snapshot, &mut world, observation.clone(), &mut commit) {
                Ok(Some(event)) => self.apply(snapshot, world, event, &mut commit)?,
                Ok(None) => {}
                Err(error) => {
                    // This entire transaction is discarded on an error too;
                    // input diagnostics must not publish translation writes.
                    return Err(error);
                }
            }
        } else {
            let input: PendingKernelInput = crate::decode_stored(row.value.clone())?;
            let event = match input {
                PendingKernelInput::Event(event) => Some(event),
                PendingKernelInput::Completion { key, completion } => {
                    return self.prepare_completion(snapshot, &key, completion);
                }
                PendingKernelInput::CancelAbsent { key } => {
                    let effect = snapshot
                        .pending_effects
                        .iter()
                        .find(|e| e.key == key)
                        .ok_or(Error::Invalid("cancellation has no pending effect"))?;
                    let delivery = crate::decode_stored(effect.payload.clone())?;
                    return self.prepare_cancel_absent(snapshot, effect, &delivery);
                }
                PendingKernelInput::Replacement { original } => {
                    let old = crate::request(snapshot, original)?;
                    let Some(pending) = snapshot.rows.iter().find(|r| {
                        r.table == "replacement_pending" && r.id == original.0.to_string()
                    }) else {
                        return Ok(commit);
                    };
                    let issue =
                        crate::steering::replacement_issue(snapshot, original, &pending.value)?;
                    self.linked_request(snapshot, old.repo, issue, original, &mut commit)?
                }
                PendingKernelInput::PriorityReread { id, row, now } => {
                    return self.prepare_reread(&id, row, now);
                }

                PendingKernelInput::IssueChange(change) => {
                    self.prepare_issue_change(snapshot, &change, &mut commit)?;
                    None
                }
                PendingKernelInput::Intake { issue, proposed } => {
                    let repository = self
                        .intake_repository(&issue)
                        .ok_or(Error::Invalid("retained intake is no longer authorized"))?;
                    commit.event_key = format!("github:{}", issue.delivery);
                    commit.event = json!({"labelled_issue": issue, "proposed": proposed});
                    if !self.kernel_headroom(snapshot, &world, &mut commit)? {
                        return Ok(commit);
                    }
                    self.prepare_intake(snapshot, &issue, repository, proposed, &mut commit)?
                }
            };
            if let Some(event) = event {
                self.apply(snapshot, world, event, &mut commit)?;
            }
        }
        Ok(commit)
    }

    pub(crate) fn replay_kernel_input(
        &mut self,
        snapshot: &Snapshot,
        row: &tl3_store::WorldRow,
    ) -> Result<()> {
        match self.consume_kernel_input(snapshot, row) {
            Ok(_) => Ok(()),
            Err(error) => self.reject_kernel_input(snapshot, row, error),
        }
    }

    fn reject_kernel_input(
        &mut self,
        snapshot: &Snapshot,
        row: &tl3_store::WorldRow,
        error: Error,
    ) -> Result<()> {
        let input: PendingKernelInput = crate::decode_stored(row.value.clone())?;
        let (target, title, node) = match input {
            PendingKernelInput::Intake { issue, .. } => (
                Some(IssueTarget {
                    repository: Repository {
                        owner: issue.owner,
                        name: issue.repo,
                        installation: issue.installation,
                    },
                    issue: issue.number,
                }),
                issue.title,
                None,
            ),
            PendingKernelInput::IssueChange(change) => (
                Some(IssueTarget {
                    repository: Repository {
                        owner: change.owner,
                        name: change.repo,
                        installation: change.installation,
                    },
                    issue: change.number,
                }),
                change.title,
                None,
            ),
            _ => (None, "Kernel input".into(), None),
        };
        let mut commit = Commit::new(format!("reject:{}", row.id), row.value.clone(), snapshot);
        self.input_failed(
            snapshot,
            &row.id,
            target,
            &title,
            node,
            Upstream::GitHub,
            error,
            &mut commit,
        )?;
        commit.rows.push(RowChange::Delete {
            table: row.table.clone(),
            id: row.id.clone(),
        });
        self.store.commit(&commit)?;
        Ok(())
    }

    /// Accept complete direct input durably before attempting any translation.
    /// It joins older retained input instead of looking at a frozen world.
    pub(crate) fn submit_kernel_input(
        &mut self,
        id: String,
        input: PendingKernelInput,
    ) -> Result<bool> {
        let snapshot = self.store.load()?;
        let mut queued = Commit::new(format!("queue:{id}"), json!(&input), &snapshot);
        queued
            .rows
            .push(put(PENDING_KERNEL, id.clone(), json!(input)));
        self.store.commit(&queued)?;
        let current = self.store.load()?;
        if current.rows.iter().any(counter_incident) {
            return Ok(true);
        }
        if !kernel_backlog(&snapshot)
            && let Some(row) = current
                .rows
                .iter()
                .find(|r| r.table == PENDING_KERNEL && r.id == id)
            && let Err(error) = self.consume_kernel_input(&current, row)
        {
            self.reject_kernel_input(&current, row, error)?;
            return Ok(false);
        }
        Ok(true)
    }

    pub(crate) fn kernel_headroom(
        &self,
        snapshot: &Snapshot,
        world: &tl3_kernel::World,
        commit: &mut Commit,
    ) -> Result<bool> {
        if snapshot.rows.iter().any(counter_incident)
            || commit.rows.iter().any(|row| {
                matches!(row,
                RowChange::Put { table, value, .. }
                    if table == "incidents" && value["open"] == true
                        && value["signature"]["class"] == COUNTER_HEADROOM)
            })
        {
            retain_input(commit);
            return Ok(false);
        }
        let Some(exhausted) = world.counter_exhaustion() else {
            return Ok(true);
        };
        self.counter_exhausted(
            snapshot,
            commit,
            exhausted.node,
            format!("kernel counter headroom exhausted: {}", exhausted.counter),
        )?;
        Ok(false)
    }

    pub(crate) fn counter_exhausted(
        &self,
        snapshot: &Snapshot,
        commit: &mut Commit,
        node: Option<NodeId>,
        message: String,
    ) -> Result<()> {
        retain_input(commit);
        let target = node
            .map(|node| {
                crate::request(snapshot, node).map(|request| IssueTarget {
                    repository: request.repo,
                    issue: request.issue,
                })
            })
            .transpose()?;
        self.input_failed(
            snapshot,
            &format!("counter:{message}"),
            target,
            "Kernel counter exhaustion",
            node,
            Upstream::Workers,
            Error::CounterExhausted(message),
            commit,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn input_failed(
        &self,
        snapshot: &Snapshot,
        scope: &str,
        target: Option<IssueTarget>,
        title: &str,
        node: Option<NodeId>,
        upstream: Upstream,
        error: Error,
        commit: &mut Commit,
    ) -> Result<()> {
        if !recoverable(&error) {
            return Err(error);
        }
        eprintln!("input failed for {scope}: {error}");
        let target = target.filter(|target| target.issue > 0);
        let class = crate::looks::failure_class(&error);
        let previous = snapshot
            .rows
            .iter()
            .filter(|row| row.table == "incidents")
            .map(|row| crate::decode_stored::<Incident>(row.value.clone()))
            .collect::<std::result::Result<Vec<_>, _>>()?;
        if previous.iter().any(|incident| {
            incident.open
                && incident.signature.upstream == upstream
                && incident.signature.class == class
                && incident
                    .input_failure
                    .as_ref()
                    .is_some_and(|failure| failure.scope == scope)
        }) {
            return Ok(());
        }
        let count = previous
            .iter()
            .filter(|incident| {
                incident
                    .input_failure
                    .as_ref()
                    .is_some_and(|failure| failure.scope == scope)
            })
            .count() as u64
            + 1;
        let needs_rob = matches!(error, Error::CounterExhausted(_))
            || crate::looks::incident_needs_rob(crate::looks::IncidentCause::ReadFailure {
                incidents: count,
            });
        let signature = IncidentSignature {
            upstream,
            class: class.into(),
        };
        let id = crate::looks::incident_id(snapshot, format!("input.{}", commit.rows.len()));
        let mut summary = format!(
            "{} Other requests can continue.",
            crate::looks::incident_summary(title, &signature, needs_rob, None)
        );
        if let Error::CounterExhausted(message) = &error {
            summary.clone_from(message);
        }
        crate::worker_inputs::append(&mut summary, crate::worker_inputs::of(&error));
        let incident = Incident {
            id: id.clone(),
            node,
            open: true,
            needs_rob,
            blocking: false,
            summary,
            signature,
            effect: None,
            post_key: Some(format!("incident:{id}")),
            // Reading or translating upstream input failed.
            cause: Some(
                if matches!(
                    class,
                    "redaction_infra" | "worker_input_schema" | "kernel_counter_headroom"
                ) || class == crate::postmortems::PAUSED
                {
                    crate::looks::Cause::Tl3
                } else {
                    crate::looks::Cause::Upstream
                },
            ),
            input_failure: Some(InputFailure {
                scope: scope.into(),
                target,
                title: title.into(),
                posted: false,
                attempted_epoch: None,
                post_failure: None,
                retry_after_ms: None,
                memory_version: None,
            }),
        };
        commit.rows.push(put("incidents", id, json!(incident)));
        Ok(())
    }

    pub(crate) fn post_input_incident(&mut self, snapshot: &Snapshot) -> Result<bool> {
        for row in snapshot.rows.iter().filter(|row| row.table == "incidents") {
            let mut incident: Incident = crate::decode_stored(row.value.clone())?;
            let Some(failure) = incident.input_failure.as_mut() else {
                continue;
            };
            if !incident.open
                || failure.posted
                || failure.attempted_epoch == Some(snapshot.epoch.get())
                || (failure.post_failure.is_some()
                    && failure
                        .retry_after_ms
                        .is_some_and(|retry| retry > self.now_ms()))
            {
                continue;
            }
            let operations = self.bootstrap.operations.as_ref().map(|o| IssueTarget {
                repository: Repository {
                    owner: o.owner.clone(),
                    name: o.name.clone(),
                    installation: o.installation_id,
                },
                issue: o.number,
            });
            let result = if let Some(target) = failure.target.clone().or_else(|| operations.clone())
            {
                let mut notice = IncidentNotice {
                    key: incident.post_key.clone().expect("input incident post key"),
                    target,
                    title: failure.title.clone(),
                    summary: incident.summary.clone(),
                    id: incident.id.clone(),
                };
                match self
                    .adapters
                    .get(Upstream::GitHub)?
                    .input_incident(&notice, snapshot.epoch.get())
                {
                    Err(error)
                        if error.is_github_not_found()
                            && failure.target.is_some()
                            && operations.is_some() =>
                    {
                        notice.target = operations.expect("operations destination");
                        self.adapters
                            .get(Upstream::GitHub)?
                            .input_incident(&notice, snapshot.epoch.get())
                    }
                    result => result,
                }
            } else {
                Err(Error::Invalid("no issue or operations destination"))
            };
            failure.attempted_epoch = Some(snapshot.epoch.get());
            match result {
                Ok(()) => {
                    failure.posted = true;
                    failure.post_failure = None;
                    failure.retry_after_ms = None;
                }
                Err(error) => {
                    if !recoverable(&error) {
                        return Err(error);
                    }
                    eprintln!(
                        "input incident {} could not be posted: {error}",
                        incident.id
                    );
                    let class = crate::looks::failure_class(&error);
                    let delay = match &error {
                        Error::Quota { retry_after } => {
                            retry_after.as_millis().min(u64::MAX as u128) as u64
                        }
                        _ => post_backoff_ms(class),
                    };
                    failure.post_failure = Some(class.into());
                    failure.retry_after_ms = Some(self.now_ms().saturating_add(delay));
                }
            }
            let mut commit = Commit::new(
                format!(
                    "input-incident-post:{}:{}",
                    incident.id,
                    snapshot.revision.get()
                ),
                json!({"input_incident_post":incident.id}),
                snapshot,
            );
            commit
                .rows
                .push(put("incidents", row.id.clone(), json!(incident)));
            self.store.commit(&commit)?;
            return Ok(true);
        }
        Ok(false)
    }

    pub(crate) fn retry_input_incident_posts(&mut self) -> Result<()> {
        let snapshot = self.store.load()?;
        let mut commit = Commit::new(
            format!("input-incident-retry:{}", snapshot.revision.get()),
            json!({"retry_input_incident_posts":true}),
            &snapshot,
        );
        for row in snapshot.rows.iter().filter(|row| row.table == "incidents") {
            let mut incident: Incident = crate::decode_stored(row.value.clone())?;
            if incident.open
                && let Some(failure) = incident.input_failure.as_mut()
                && !failure.posted
                && failure.attempted_epoch.is_some()
            {
                // Old rows have a failure class but no persisted deadline.
                if let Some(class) = failure.post_failure.as_deref() {
                    if failure.retry_after_ms.is_none() {
                        failure.retry_after_ms =
                            Some(self.now_ms().saturating_add(post_backoff_ms(class)));
                    } else if failure
                        .retry_after_ms
                        .is_some_and(|retry| retry <= self.now_ms())
                    {
                        failure.attempted_epoch = None;
                    } else {
                        continue;
                    }
                } else {
                    failure.attempted_epoch = None;
                }
                commit
                    .rows
                    .push(put("incidents", row.id.clone(), json!(incident)));
            }
        }
        if !commit.rows.is_empty() {
            self.store.commit(&commit)?;
        }
        Ok(())
    }
}

pub(crate) fn observation_context(
    snapshot: &Snapshot,
    observation: &Observation,
) -> Result<(Option<IssueTarget>, String, Option<NodeId>)> {
    use tl3_kernel::Event;
    let node = match observation {
        Observation::ScratchSaved { node, .. }
        | Observation::Steering { node, .. }
        | Observation::WorkerReply { node, .. }
        | Observation::WorkerFailed { node, .. }
        | Observation::CiFailed { node, .. }
        | Observation::WorktreePrepared { node, .. }
        | Observation::CheckPlan { node, .. }
        | Observation::Checks { node, .. }
        | Observation::Plan { node, .. }
        | Observation::Made { node, .. }
        | Observation::Reviewed { node, .. }
        | Observation::RebaseConflict { node, .. }
        | Observation::Rebased { node, .. }
        | Observation::Merged { node, .. }
        | Observation::PullRequest { node, .. }
        | Observation::Kernel(
            Event::Reply { node, .. }
            | Event::Ack { node, .. }
            | Event::Fact { node, .. }
            | Event::Command { node, .. },
        ) => Some(*node),
        Observation::LinkedRequest { original, .. } => Some(*original),
        Observation::Queue(fact) => fact.node(snapshot)?,
        Observation::Progress { key, .. }
        | Observation::Quota { key, .. }
        | Observation::Classified { key, .. }
        | Observation::Postmortem { key, .. } => snapshot
            .rows
            .iter()
            .find(|row| row.table == "allowances" && row.id == *key)
            .map(|row| crate::decode_stored::<crate::looks::AllowanceState>(row.value.clone()))
            .transpose()?
            .map(|state| state.delivery.node()),
        Observation::Prerequisite(_)
        | Observation::ScratchPruned { .. }
        | Observation::TraceLogsCapped { .. }
        | Observation::GitHubComment { .. }
        | Observation::Kernel(Event::NewRequest { .. } | Event::NewAdoption { .. }) => None,
    };
    if let Some(node) = node
        && snapshot
            .rows
            .iter()
            .any(|row| row.table == "requests" && row.id == node.0.to_string())
    {
        let request = crate::request(snapshot, node)?;
        return Ok((
            Some(IssueTarget {
                repository: request.repo,
                issue: request.issue,
            }),
            request.title,
            Some(node),
        ));
    }
    if let Observation::GitHubComment {
        repository, issue, ..
    } = observation
    {
        return Ok((
            Some(IssueTarget {
                repository: repository.clone(),
                issue: *issue,
            }),
            "An issue update".into(),
            None,
        ));
    }
    Ok((None, "An incoming update".into(), None))
}
