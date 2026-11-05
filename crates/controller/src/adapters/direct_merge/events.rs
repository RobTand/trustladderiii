use super::*;
use crate::{
    cli::Input,
    ingress::{GitHubDelivery, GitHubTranslator},
};
use tl3_github::webhook::Event;

/// Queue repositories keep the existing translator, including its read order and fact keys.
pub struct Translator {
    queue: super::super::mergify::Translator,
    clients: BTreeMap<u64, GitHub>,
}
impl Translator {
    pub fn new(clients: BTreeMap<u64, GitHub>) -> Result<Self> {
        Ok(Self {
            queue: super::super::mergify::Translator::new(clients.clone())?,
            clients,
        })
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
        for row in snapshot.rows.iter().filter(|row| {
            row.table == "pull_requests" && row.value["number"].as_u64() == Some(pull.number)
        }) {
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
                Some(Evidence::Merged {
                    head: pull.head.sha.clone(),
                    checks_passed: crate::commit_sha(snapshot, n.approved)
                        == Some(pull.head.sha.as_str())
                        && n.checked == n.approved
                        && n.approved.is_some(),
                })
            } else if synchronized
                && crate::commit_sha(snapshot, n.head) != Some(pull.head.sha.as_str())
            {
                Some(Evidence::PushByOther {
                    head: pull.head.sha.clone(),
                })
            } else {
                None
            };
            if let Some(evidence) = evidence {
                let fact = QueueFact {
                    repository: repo.clone(),
                    number: pull.number,
                    evidence,
                };
                if !fact.recorded(snapshot)? {
                    inputs.push(Input::Observation {
                        key: format!("direct:{}", fact.identity()),
                        observation: Observation::Queue(fact),
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
    ) -> Result<Vec<Input>> {
        let mut inputs = Vec::new();
        for effect in &snapshot.pending_effects {
            let delivery = crate::hydrate(effect, snapshot)?;
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
                inputs.push(Input::Look {
                    key: effect.key.clone(),
                });
            }
        }
        Ok(inputs)
    }
}
impl GitHubTranslator for Translator {
    fn translate(&mut self, delivery: &GitHubDelivery, snapshot: &Snapshot) -> Result<Vec<Input>> {
        let repo: Option<Repository> = match delivery {
            GitHubDelivery::Webhook(delivery) => delivery.event.as_ref().and_then(|event| {
                let context = match event {
                    Event::PullRequestMerged { context, .. }
                    | Event::PullRequestClosed { context, .. }
                    | Event::PullRequestSynchronized { context, .. }
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
            return self.queue.translate(delivery, snapshot);
        };
        if !repository_config::direct(snapshot, &repo)? {
            return self.queue.translate(delivery, snapshot);
        }
        match delivery {
            GitHubDelivery::Webhook(delivery) => match delivery.event.as_ref() {
                Some(Event::PullRequestMerged { pull_request, .. }) => {
                    self.input(snapshot, &repo, pull_request, false)
                }
                Some(Event::PullRequestSynchronized { pull_request, .. }) => {
                    let mut inputs = self.input(snapshot, &repo, pull_request, true)?;
                    inputs.extend(self.wakes(snapshot, &repo, Some(pull_request.number), None)?);
                    Ok(inputs)
                }
                Some(
                    Event::CheckRunCompleted { check_run, .. }
                    | Event::CheckRunUpdated { check_run, .. },
                ) => self.wakes(snapshot, &repo, None, Some(&check_run.head_sha)),
                Some(Event::CheckSuiteCompleted { check_suite, .. }) => {
                    self.wakes(snapshot, &repo, None, Some(&check_suite.head_sha))
                }
                Some(Event::PullRequestClosed { pull_request, .. }) => {
                    self.wakes(snapshot, &repo, Some(pull_request.number), None)
                }
                _ => Ok(vec![]), // No Mergify dequeue, missing-check or merge-group rules in direct mode.
            },
            GitHubDelivery::CatchUp { .. } => {
                let mut inputs = self.wakes(snapshot, &repo, None, None)?;
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
                    let number = row.value["number"]
                        .as_u64()
                        .ok_or(Error::Invalid("invalid direct PR number"))?;
                    let pull = crate::executor::runtime()?
                        .block_on(client.pull_request(&full_name(&repo), number))?;
                    inputs.extend(self.input(snapshot, &repo, &pull, true)?);
                }
                Ok(inputs)
            }
        }
    }
}
