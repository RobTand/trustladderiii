use super::*;
use tl3_github::{CheckAnnotation, PullRequest, WorkflowRun};

enum Failure {
    Tests {
        check: u64,
        completed_at: Option<String>,
    },
    Infrastructure(Vec<CheckRun>),
}

pub(super) struct Checks {
    pub ready: bool,
    pub unavailable: Vec<String>,
    /// Required contexts with no check run from the expected app and no status.
    pub missing: Vec<RequiredCheck>,
    /// Required contexts that have reported on the head, in any state.
    pub reported: Vec<RequiredCheck>,
    failure: Option<Failure>,
}

impl Checks {
    pub(super) fn infrastructure(&self) -> Option<bool> {
        self.failure
            .as_ref()
            .map(|failure| matches!(failure, Failure::Infrastructure(_)))
    }

    pub(super) fn evidence(&self, pull: &PullRequest) -> Option<Evidence> {
        let (check, completed_at, infra) = match self.failure.as_ref()? {
            Failure::Tests {
                check,
                completed_at,
            } => (*check, completed_at.clone(), false),
            Failure::Infrastructure(checks) => {
                let check = &checks[0];
                (
                    check.id,
                    check.completed_at.map(|time| time.to_rfc3339()),
                    true,
                )
            }
        };
        Some(Evidence::CiFailed {
            check,
            completed_at,
            head: pull.head.sha.clone(),
            infra,
            head_check: true,
        })
    }
}

pub(super) async fn read(github: &GitHub, repo: &str, pull: &PullRequest) -> Result<Checks> {
    let requirements = github
        .required_status_checks(repo, &pull.base.branch)
        .await?;
    let checks = if requirements.checks.is_empty() {
        vec![]
    } else {
        github.check_runs(repo, &pull.head.sha).await?
    };
    evaluate(github, repo, pull, requirements, &checks).await
}

pub(super) async fn evaluate(
    github: &GitHub,
    repo: &str,
    pull: &PullRequest,
    requirements: tl3_github::RequiredChecks,
    checks: &[CheckRun],
) -> Result<Checks> {
    let required = requirements.checks;
    let mut unavailable = requirements.unavailable;
    if required.is_empty() {
        return Ok(Checks {
            ready: true,
            unavailable,
            missing: vec![],
            reported: vec![],
            failure: None,
        });
    }
    let mut latest = BTreeMap::new();
    for check in checks.iter().cloned() {
        if check.head_sha == pull.head.sha {
            let key = (check.app.as_ref().map(|app| app.id), check.name.clone());
            let entry = latest.entry(key).or_insert_with(|| check.clone());
            if check.id > entry.id {
                *entry = check;
            }
        }
    }
    let mut statuses = BTreeMap::new();
    let mut statuses_available = true;
    if required.iter().any(|rule| rule.integration_id.is_none()) {
        let read = match github.commit_statuses(repo, &pull.head.sha).await {
            Ok(statuses) => statuses,
            Err(tl3_github::Error::Http {
                status: status @ (403 | 404),
            }) => {
                unavailable.push(format!("Commit statuses: read unavailable (HTTP {status})"));
                statuses_available = false;
                vec![]
            }
            Err(error) => return Err(error.into()),
        };
        for status in read {
            let entry = statuses
                .entry(status.context.clone())
                .or_insert_with(|| status.clone());
            if status.id > entry.id {
                *entry = status;
            }
        }
    }
    let mut ready = true;
    let mut missing = Vec::new();
    let mut reported = Vec::new();
    let mut tests = None;
    let mut infrastructure = Vec::new();
    for rule in required {
        let checks: Vec<_> = latest
            .values()
            .filter(|check| {
                rule.context == check.name
                    && rule
                        .integration_id
                        .is_none_or(|id| check.app.as_ref().is_some_and(|app| app.id == id))
            })
            .collect();
        let status = rule
            .integration_id
            .is_none()
            .then(|| statuses.get(&rule.context))
            .flatten();
        let expected = RequiredCheck {
            context: rule.context.clone(),
            app: rule.integration_id,
        };
        if checks.is_empty() && status.is_none() {
            if rule.integration_id.is_some() || statuses_available {
                ready = false;
                missing.push(expected);
            }
        } else {
            reported.push(expected);
        }
        for check in checks {
            if check.status != "completed"
                || matches!(
                    check.conclusion.as_deref(),
                    Some("action_required" | "stale")
                )
            {
                ready = false;
                continue;
            }
            if matches!(
                check.conclusion.as_deref(),
                Some("success" | "neutral" | "skipped")
            ) {
                continue;
            }
            ready = false;
            let actions = check
                .app
                .as_ref()
                .is_some_and(|app| app.slug == "github-actions");
            let infra = actions
                && (matches!(
                    check.conclusion.as_deref(),
                    Some("cancelled" | "timed_out" | "startup_failure")
                ) || check.conclusion.as_deref() == Some("failure")
                    && runner_failure(&github.check_annotations(repo, check.id).await?));
            if infra {
                infrastructure.push(check.clone());
            } else if check.conclusion.is_some() {
                tests = Some(Failure::Tests {
                    check: check.id,
                    completed_at: check.completed_at.map(|time| time.to_rfc3339()),
                });
            }
        }
        if let Some(status) = status {
            if status.state != "success" {
                ready = false;
            }
            if matches!(status.state.as_str(), "failure" | "error") {
                tests = Some(Failure::Tests {
                    check: status.id,
                    completed_at: status.updated_at.map(|time| time.to_rfc3339()),
                });
            }
        }
    }
    Ok(Checks {
        ready,
        unavailable,
        missing,
        reported,
        failure: tests.or_else(|| {
            (!infrastructure.is_empty()).then_some(Failure::Infrastructure(infrastructure))
        }),
    })
}

async fn failure(github: &GitHub, repo: &str, pull: &PullRequest) -> Result<Option<Failure>> {
    Ok(read(github, repo, pull).await?.failure)
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
