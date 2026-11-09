use super::*;
use crate::{
    cli::Input,
    ingress::{GitHubDelivery, GitHubTranslator},
};
use tl3_github::webhook::Event;

/// Translate direct delivery evidence without queue mechanics.
pub struct Translator {
    wakes: Wakes,
    clients: BTreeMap<u64, GitHub>,
}
impl Translator {
    pub fn new(clients: BTreeMap<u64, GitHub>) -> Result<Self> {
        Ok(Self {
            wakes: Wakes::default(),
            clients,
        })
    }
    pub fn with_wakes(mut self, wakes: Wakes) -> Self {
        self.wakes = wakes;
        self
    }
    fn input(
        &self,
        snapshot: &Snapshot,
        repo: &Repository,
        pull: &PullRequest,
        synchronized: bool,
    ) -> Result<Vec<Input>> {
        let world = crate::snapshot::decode(snapshot)?;
        let mut inputs = Vec::new();
        for row in snapshot
            .rows
            .iter()
            .filter(|row| row.table == "pull_requests")
        {
            // A stored PR row whose number does not decode is corruption: fatal.
            if crate::decode_stored::<u64>(row.value["number"].clone())? != pull.number {
                continue;
            }
            let node = tl3_kernel::NodeId(
                row.id
                    .parse()
                    .map_err(|_| Error::Invalid("invalid direct PR binding"))?,
            );
            if crate::request(snapshot, node)?.repo != *repo {
                continue;
            }
            let n = world
                .nodes
                .get(node.0 as usize)
                .ok_or(Error::Invalid("missing direct PR node"))?;
            let evidence = if pull.merged_at.is_some() {
                Some(DeliveryOutcome::Merged {
                    head: pull.head.sha.clone(),
                    checks_passed: crate::commit_sha(snapshot, n.approved)
                        == Some(pull.head.sha.as_str())
                        && n.checked == n.approved
                        && n.approved.is_some(),
                })
            } else if synchronized
                && crate::commit_sha(snapshot, n.head) != Some(pull.head.sha.as_str())
            {
                Some(DeliveryOutcome::PushByOther {
                    head: pull.head.sha.clone(),
                })
            } else {
                None
            };
            if let Some(evidence) = evidence {
                if let DeliveryOutcome::Merged {
                    head,
                    checks_passed,
                } = &evidence
                    && let Some(journal) = self.wakes.journal()
                {
                    let mut deliveries: Vec<Delivery> = snapshot
                        .pending_effects
                        .iter()
                        .map(|effect| crate::hydrate(effect, snapshot))
                        .collect::<Result<_>>()?;
                    for row in snapshot
                        .rows
                        .iter()
                        .filter(|r| matches!(r.table.as_str(), "watches" | "retiring_effects"))
                    {
                        deliveries.push(crate::decode_stored(row.value.clone())?);
                    }
                    for mut delivery in deliveries {
                        if delivery.node() == node
                            && matches!(
                                delivery.output,
                                Output::Effect {
                                    kind: EffectKind::Enqueue,
                                    ..
                                }
                            )
                        {
                            delivery.pull_request = Some(pull.number);
                            journal.remember_merged(
                                &delivery,
                                &Merged {
                                    head: head.clone(),
                                    checks_passed: *checks_passed,
                                },
                                |_, _| {},
                            )?;
                        }
                    }
                }
                let suffix = match &evidence {
                    DeliveryOutcome::Merged { head, .. } => format!("merged:{head}"),
                    DeliveryOutcome::PushByOther { head } => format!("push:{head}"),
                    _ => unreachable!("PR event outcome"),
                };
                let identity = crate::outcome::identity(repo, pull.number, &suffix);
                if !crate::outcome::recorded(snapshot, &identity) {
                    inputs.push(Input::Observation {
                        key: format!("direct:{identity}"),
                        observation: Observation::Outcome {
                            node,
                            outcome: evidence,
                            receipt: Some(identity),
                        },
                    });
                }
            }
        }
        Ok(inputs)
    }
    fn wakes(
        &self,
        snapshot: &Snapshot,
        repo: &Repository,
        number: Option<u64>,
        sha: Option<&str>,
        event: &GitHubDelivery,
    ) -> Result<Vec<Input>> {
        let Some(sequence) = self
            .wakes
            .journal()
            .map(|j| j.event_sequence(event))
            .transpose()?
            .flatten()
        else {
            return Ok(vec![]);
        };
        let mut inputs = Vec::new();
        let mut deliveries = BTreeMap::new();
        for effect in &snapshot.pending_effects {
            deliveries.insert(effect.key.clone(), crate::hydrate(effect, snapshot)?);
        }
        for row in snapshot.rows.iter().filter(|row| row.table == "watches") {
            let delivery: Delivery = crate::decode_stored(row.value.clone())?;
            deliveries.entry(delivery.key.clone()).or_insert(delivery);
        }
        let world = crate::snapshot::decode(snapshot)?;
        for (key, delivery) in deliveries {
            let n = world
                .nodes
                .get(delivery.node().0 as usize)
                .ok_or(Error::Invalid("missing watched direct node"))?;
            if !delivery_authority(n, &delivery) {
                continue;
            }
            if delivery.repository == *repo
                && number.is_none_or(|number| delivery.pull_request == Some(number))
                && sha.is_none_or(|sha| delivery.head_sha.as_deref() == Some(sha))
                && matches!(
                    delivery.output,
                    Output::Effect {
                        kind: EffectKind::Enqueue,
                        ..
                    }
                )
            {
                self.wakes.wake(&key, sequence);
                inputs.push(Input::Look { key });
            }
        }
        Ok(inputs)
    }
}
impl GitHubTranslator for Translator {
    fn translate(&mut self, delivery: &GitHubDelivery, snapshot: &Snapshot) -> Result<Vec<Input>> {
        // Corrupt served-repository rows are controller corruption: fail with
        // the fatal stored-JSON error before any delivery is read.
        let repos = snapshot
            .rows
            .iter()
            .filter(|row| row.table == "repos")
            .map(|row| crate::decode_stored(row.value.clone()))
            .collect::<Result<Vec<Repository>>>()?;
        if let GitHubDelivery::CatchUp { repository, .. } = delivery
            && !repos.contains(repository)
        {
            return Ok(vec![]);
        }
        // Legacy protection statuses have no typed Event variant. The authenticated
        // payload is still durably replayed by ingress, so decode just its wake key.
        if let GitHubDelivery::Webhook(webhook) = delivery
            && webhook.event_name == "status"
        {
            #[derive(Deserialize)]
            struct Installation {
                id: u64,
            }
            #[derive(Deserialize)]
            struct Status {
                repository: tl3_github::Repository,
                installation: Installation,
                sha: String,
            }
            if let Ok(status) = serde_json::from_value::<Status>(webhook.payload.clone()) {
                let repo = Repository {
                    owner: status.repository.owner.login,
                    name: status.repository.name,
                    installation: status.installation.id,
                };
                return self.wakes(snapshot, &repo, None, Some(&status.sha), delivery);
            }
        }
        let repo: Option<Repository> = match delivery {
            GitHubDelivery::Webhook(delivery) => delivery.event.as_ref().and_then(|event| {
                let context = match event {
                    Event::PullRequestMerged { context, .. }
                    | Event::PullRequestClosed { context, .. }
                    | Event::PullRequestSynchronized { context, .. }
                    | Event::PullRequestUpdated { context, .. }
                    | Event::PullRequestUnlabeled { context, .. }
                    | Event::CheckRunCompleted { context, .. }
                    | Event::CheckRunUpdated { context, .. }
                    | Event::CheckSuiteCompleted { context, .. }
                    | Event::MergeGroup { context, .. } => context,
                    _ => return None,
                };
                Some(Repository {
                    owner: context.repository.owner.login.clone(),
                    name: context.repository.name.clone(),
                    installation: context.installation_id,
                })
            }),
            GitHubDelivery::CatchUp { repository, .. } => Some(repository.clone()),
        };
        let Some(repo) = repo else {
            return Ok(vec![]);
        };
        match delivery {
            GitHubDelivery::Webhook(webhook) => match webhook.event.as_ref() {
                Some(Event::PullRequestMerged { pull_request, .. }) => {
                    self.input(snapshot, &repo, pull_request, false)
                }
                Some(Event::PullRequestSynchronized { pull_request, .. }) => {
                    let mut inputs = self.input(snapshot, &repo, pull_request, true)?;
                    inputs.extend(self.wakes(
                        snapshot,
                        &repo,
                        Some(pull_request.number),
                        None,
                        delivery,
                    )?);
                    Ok(inputs)
                }
                Some(
                    Event::CheckRunCompleted { check_run, .. }
                    | Event::CheckRunUpdated { check_run, .. },
                ) => self.wakes(snapshot, &repo, None, Some(&check_run.head_sha), delivery),
                Some(Event::CheckSuiteCompleted { check_suite, .. }) => {
                    self.wakes(snapshot, &repo, None, Some(&check_suite.head_sha), delivery)
                }
                Some(
                    Event::PullRequestClosed { pull_request, .. }
                    | Event::PullRequestUpdated { pull_request, .. },
                ) => self.wakes(snapshot, &repo, Some(pull_request.number), None, delivery),
                _ => Ok(vec![]), // No Mergify dequeue, missing-check or merge-group rules in direct mode.
            },
            GitHubDelivery::CatchUp { changes, .. } => {
                let world = crate::snapshot::decode(snapshot)?;
                let mut inputs = self.wakes(snapshot, &repo, None, None, delivery)?;
                let client = self
                    .clients
                    .get(&repo.installation)
                    .ok_or(Error::Invalid("missing direct catch-up client"))?;
                for row in snapshot
                    .rows
                    .iter()
                    .filter(|row| row.table == "pull_requests")
                {
                    let node = tl3_kernel::NodeId(
                        row.id
                            .parse()
                            .map_err(|_| Error::Invalid("invalid direct PR binding"))?,
                    );
                    if crate::request(snapshot, node)?.repo != repo {
                        continue;
                    }
                    let number: u64 = crate::decode_stored(row.value["number"].clone())?;
                    let n = world
                        .nodes
                        .get(node.0 as usize)
                        .ok_or(Error::Invalid("missing direct catch-up node"))?;
                    let changed = changes
                        .pull_requests
                        .iter()
                        .any(|pull| pull.number == number)
                        || changes.closed.iter().any(|(closed, _)| *closed == number)
                        || changes.check_runs.iter().any(|check| {
                            Some(check.head_sha.as_str()) == crate::commit_sha(snapshot, n.head)
                        })
                        || changes.check_suites.iter().any(|check| {
                            Some(check.head_sha.as_str()) == crate::commit_sha(snapshot, n.head)
                        });
                    if matches!(n.phase, Phase::Closed(_)) && !changed {
                        continue;
                    }
                    let pull = crate::executor::runtime()?
                        .block_on(client.pull_request(&full_name(&repo), number))?;
                    inputs.extend(self.input(snapshot, &repo, &pull, true)?);
                }
                Ok(inputs)
            }
        }
    }
}
