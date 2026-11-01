use crate::{
    Controller, Error, Repository, Result,
    executor::{Observation, Upstream},
    looks::{Incident, IncidentSignature},
    put,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tl3_kernel::NodeId;
use tl3_store::{Commit, Snapshot};

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

impl Controller {
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
        let needs_rob =
            crate::looks::incident_needs_rob(crate::looks::IncidentCause::ReadFailure {
                incidents: count,
            });
        let signature = IncidentSignature {
            upstream,
            class: class.into(),
        };
        let id = crate::looks::incident_id(snapshot, format!("input.{}", commit.rows.len()));
        let summary = format!(
            "{} Other requests can continue.",
            crate::looks::incident_summary(title, &signature, needs_rob, None)
        );
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
            input_failure: Some(InputFailure {
                scope: scope.into(),
                target,
                title: title.into(),
                posted: false,
                attempted_epoch: None,
                post_failure: None,
                retry_after_ms: None,
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
        | Observation::Classified { key, .. } => snapshot
            .rows
            .iter()
            .find(|row| row.table == "allowances" && row.id == *key)
            .map(|row| crate::decode_stored::<crate::looks::AllowanceState>(row.value.clone()))
            .transpose()?
            .map(|state| state.delivery.node()),
        Observation::ScratchPruned { .. }
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
