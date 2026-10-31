use super::*;
use tl3_github::{CheckAnnotation, PullRequest, WorkflowRun};

enum Failure {
    Tests,
    Infrastructure(Vec<CheckRun>),
}

async fn failure(github: &GitHub, repo: &str, pull: &PullRequest) -> Result<Option<Failure>> {
    let mut latest = BTreeMap::new();
    for check in github.check_runs(repo, &pull.head.sha).await? {
        if check.head_sha == pull.head.sha
            && check
                .app
                .as_ref()
                .is_some_and(|app| app.slug == "github-actions")
        {
            let key = (check.app.as_ref().unwrap().id, check.name.clone());
            let entry = latest.entry(key).or_insert_with(|| check.clone());
            if check.id > entry.id {
                *entry = check;
            }
        }
    }
    let failed: Vec<_> = latest
        .into_values()
        .filter(|check| {
            check.status == "completed"
                && matches!(
                    check.conclusion.as_deref(),
                    Some("cancelled" | "timed_out" | "startup_failure" | "failure")
                )
        })
        .collect();
    if failed.is_empty() {
        return Ok(None);
    }
    let required = github
        .required_status_checks(repo, &pull.base.branch)
        .await?;
    let mut infrastructure = Vec::new();
    for check in failed.into_iter().filter(|check| {
        required.iter().any(|rule| {
            rule.context == check.name
                && rule
                    .integration_id
                    .is_none_or(|id| check.app.as_ref().is_some_and(|app| app.id == id))
        })
    }) {
        if check.conclusion.as_deref() == Some("failure")
            && !runner_failure(&github.check_annotations(repo, check.id).await?)
        {
            return Ok(Some(Failure::Tests));
        }
        infrastructure.push(check);
    }
    Ok((!infrastructure.is_empty()).then_some(Failure::Infrastructure(infrastructure)))
}

fn runner_failure(annotations: &[CheckAnnotation]) -> bool {
    let failures: Vec<_> = annotations
        .iter()
        .filter(|annotation| annotation.annotation_level == "failure")
        .collect();
    !failures.is_empty()
        && failures.iter().all(|annotation| {
            [
                "The runner has lost communication with the server",
                "The job was not acquired by Runner of type",
                "The job was abandoned as the runner did not pick it up",
                "The job was not started because recent account payments have failed",
            ]
            .iter()
            .any(|prefix| annotation.message.starts_with(prefix))
        })
}

pub(super) async fn infrastructure(
    github: &GitHub,
    repo: &str,
    pull: &PullRequest,
) -> Result<Option<bool>> {
    Ok(failure(github, repo, pull)
        .await?
        .map(|failure| matches!(failure, Failure::Infrastructure(_))))
}

#[derive(Serialize, Deserialize)]
struct Rerun {
    run: WorkflowRun,
    accepted: bool,
    #[serde(default)]
    waiting_for_completion: bool,
}

impl Adapter {
    fn saved_reruns(&self, delivery: &Delivery) -> Result<Vec<Rerun>> {
        match std::fs::read(self.records.path(&delivery.key).with_extension("reruns")) {
            Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(vec![]),
            Err(error) => Err(error.into()),
        }
    }

    pub(super) fn reruns_pending(&self, delivery: &Delivery) -> Result<bool> {
        let github = &self.clients[&delivery.repository.installation];
        let repo = full_name(&delivery.repository);
        for saved in self
            .saved_reruns(delivery)?
            .iter()
            .filter(|saved| !saved.accepted)
        {
            let current = self
                .runtime
                .block_on(github.workflow_run(&repo, saved.run.id))
                .map_err(Error::from)?;
            if current.head_sha != saved.run.head_sha {
                return Ok(true);
            }
            if current.run_attempt > saved.run.run_attempt {
                continue;
            }
            if !saved.waiting_for_completion || current.status != "completed" {
                return Ok(true);
            }
        }
        Ok(false)
    }

    pub(super) fn rerun_head_checks(
        &mut self,
        delivery: &Delivery,
        state: &tl3_mergify::Observation,
    ) -> Result<()> {
        let github = self.clients[&delivery.repository.installation].clone();
        let repo = full_name(&delivery.repository);
        let Some(Failure::Infrastructure(checks)) =
            self.runtime
                .block_on(failure(&github, &repo, &state.pull_request))?
        else {
            return Ok(());
        };
        let (number, head) = enqueue(delivery)?;
        let runs = self
            .runtime
            .block_on(github.workflow_runs(&repo, head))
            .map_err(Error::from)?;
        let mut selected = BTreeMap::new();
        for check in checks {
            let run = runs
                .iter()
                .filter(|run| {
                    run.head_sha == head
                        && check
                            .check_suite
                            .as_ref()
                            .is_some_and(|suite| suite.id == run.check_suite_id)
                })
                .max_by_key(|run| run.id)
                .ok_or(Error::Invalid(
                    "required check has no matching head workflow run",
                ))?;
            selected.insert(run.id, run.clone());
        }
        let mut saved = self.saved_reruns(delivery)?;
        let path = self.records.path(&delivery.key).with_extension("reruns");
        for run in selected.into_values() {
            if let Some(index) = saved.iter().position(|saved| saved.run.id == run.id) {
                let previous = &saved[index];
                if previous.accepted || run.run_attempt > previous.run.run_attempt {
                    continue;
                }
                if !previous.waiting_for_completion {
                    return Err(Error::Invalid("workflow rerun acceptance is still unknown"));
                }
                saved.remove(index);
            }
            if run.status != "completed" {
                saved.push(Rerun {
                    run,
                    accepted: false,
                    waiting_for_completion: true,
                });
                self.records.write(&path, &serde_json::to_vec(&saved)?)?;
                return Err(Error::Invalid(
                    "workflow run is still finishing before rerun",
                ));
            }
            let pull = self.runtime.block_on(github.pull_request(&repo, number))?;
            if pull.head.sha != run.head_sha || pull.state != "open" || pull.merged_at.is_some() {
                return Err(Error::Invalid("PR changed before workflow rerun"));
            }
            self.fence(delivery)?;
            saved.push(Rerun {
                run: run.clone(),
                accepted: false,
                waiting_for_completion: false,
            });
            // GitHub has no idempotency key for reruns. Persist before POST and recover
            // a lost response from run_attempt; silence never permits another POST.
            self.records.write(&path, &serde_json::to_vec(&saved)?)?;
            match self
                .runtime
                .block_on(github.rerun_workflow_run(&repo, run.id))
            {
                Ok(()) => saved.last_mut().unwrap().accepted = true,
                Err(error) => {
                    match &error {
                        tl3_github::Error::ActionsForbidden(
                            tl3_github::ActionsRefusal::RunInProgress,
                        ) => {
                            saved.last_mut().unwrap().waiting_for_completion = true;
                        }
                        tl3_github::Error::ActionsForbidden(_)
                        | tl3_github::Error::RateLimited { .. }
                        | tl3_github::Error::Http {
                            status: 400..=407 | 409..=499,
                        } => {
                            // A refused request did not start a rerun. Retrying this key is safe.
                            saved.pop();
                        }
                        _ => {}
                    }
                    self.records.write(&path, &serde_json::to_vec(&saved)?)?;
                    return Err(error.into());
                }
            }
            self.records.write(&path, &serde_json::to_vec(&saved)?)?;
        }
        Ok(())
    }
}
