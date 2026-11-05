use super::*;
use tl3_github::{CheckAnnotation, PullRequest, WorkflowRun};
use tl3_mergify::decide::required;

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
            details: Default::default(),
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
        let reports = !checks.is_empty() || status.is_some();
        match required::presence(reports, rule.integration_id.is_some(), statuses_available) {
            required::Presence::Missing => {
                ready = false;
                missing.push(expected);
            }
            required::Presence::Reported => reported.push(expected),
            required::Presence::Unavailable => {}
        }
        for check in checks {
            let view = tl3_mergify::views(std::slice::from_ref(check), &pull.head.sha, |_| true)[0];
            // Only Actions failures need annotations to distinguish runner failures.
            let runner = view.status == tl3_mergify::decide::Status::Completed
                && view.app == tl3_mergify::decide::App::Actions
                && view.conclusion == Some(tl3_mergify::decide::Conclusion::Failure)
                && runner_failure(&github.check_annotations(repo, check.id).await?);
            match lifecycle::required_check(view, runner) {
                lifecycle::RequiredState::Passed => {}
                lifecycle::RequiredState::Pending | lifecycle::RequiredState::Unknown => {
                    ready = false
                }
                lifecycle::RequiredState::Infrastructure => {
                    ready = false;
                    infrastructure.push(check.clone());
                }
                lifecycle::RequiredState::Tests => {
                    ready = false;
                    tests = Some(Failure::Tests {
                        check: check.id,
                        completed_at: check.completed_at.map(|time| time.to_rfc3339()),
                    });
                }
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
            let (_, effects) = lifecycle::decide(
                lifecycle::EnqueueState {
                    rerun: Some(lifecycle::Rerun {
                        attempt: saved.run.run_attempt,
                        accepted: saved.accepted,
                        waiting: saved.waiting_for_completion,
                    }),
                    ..Default::default()
                },
                lifecycle::QueueEvent::RerunPending {
                    head: lifecycle_head(current.head_sha == saved.run.head_sha),
                    attempt: current.run_attempt,
                    status: tl3_mergify::decide::Status::parse(&current.status),
                },
            );
            if effects == [lifecycle::Effect::RerunPending(true)] {
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
            let prior = saved.iter().find(|saved| saved.run.id == run.id);
            let mut core = lifecycle::EnqueueState {
                rerun: prior.map(|saved| lifecycle::Rerun {
                    attempt: saved.run.run_attempt,
                    accepted: saved.accepted,
                    waiting: saved.waiting_for_completion,
                }),
                ..Default::default()
            };
            let mut writable = None;
            loop {
                let (next, effects) = lifecycle::decide(
                    core,
                    lifecycle::QueueEvent::RerunRead {
                        head: lifecycle_head(run.head_sha == head),
                        attempt: run.run_attempt,
                        status: tl3_mergify::decide::Status::parse(&run.status),
                        writable,
                    },
                );
                core = next;
                let mut read_pull = false;
                for effect in effects {
                    match effect {
                        lifecycle::Effect::Continue => {}
                        lifecycle::Effect::Read(lifecycle::Need::RerunPull) => {
                            let pull = self.runtime.block_on(github.pull_request(&repo, number))?;
                            writable = Some(
                                pull.head.sha == run.head_sha
                                    && pull.state == "open"
                                    && pull.merged_at.is_none(),
                            );
                            read_pull = true;
                        }
                        lifecycle::Effect::Error(failure) => return Err(lifecycle_error(failure)),
                        lifecycle::Effect::SaveRerun => {
                            if core.rerun.is_some_and(|rerun| !rerun.waiting) {
                                self.fence(delivery)?;
                            }
                            save_rerun_projection(self, &path, &mut saved, &run, core.rerun)?;
                        }
                        lifecycle::Effect::Write(lifecycle::Write::Rerun) => {
                            let result = self
                                .runtime
                                .block_on(github.rerun_workflow_run(&repo, run.id));
                            let outcome = match &result {
                                Ok(()) => lifecycle::RerunResult::Accepted,
                                Err(tl3_github::Error::ActionsForbidden(
                                    tl3_github::ActionsRefusal::RunInProgress,
                                )) => lifecycle::RerunResult::InProgress,
                                Err(
                                    tl3_github::Error::ActionsForbidden(_)
                                    | tl3_github::Error::RateLimited { .. }
                                    | tl3_github::Error::Http {
                                        status: 400..=407 | 409..=499,
                                    },
                                ) => lifecycle::RerunResult::Refused,
                                Err(_) => lifecycle::RerunResult::Unknown,
                            };
                            let (next, effects) =
                                lifecycle::decide(core, lifecycle::QueueEvent::RerunDone(outcome));
                            for effect in effects {
                                if effect == lifecycle::Effect::SaveRerun {
                                    save_rerun_projection(
                                        self, &path, &mut saved, &run, next.rerun,
                                    )?;
                                }
                            }
                            result?;
                        }
                        _ => return Err(Error::Invalid("unexpected workflow rerun effect")),
                    }
                }
                if !read_pull {
                    break;
                }
            }
        }

        Ok(())
    }
}

/// Update the existing JSON list without changing its schema or workflow payload.
fn save_rerun_projection(
    adapter: &Adapter,
    path: &Path,
    saved: &mut Vec<Rerun>,
    run: &WorkflowRun,
    state: Option<lifecycle::Rerun>,
) -> Result<()> {
    saved.retain(|saved| saved.run.id != run.id);
    if let Some(state) = state {
        saved.push(Rerun {
            run: run.clone(),
            accepted: state.accepted,
            waiting_for_completion: state.waiting,
        });
    }
    adapter.records.write(path, &serde_json::to_vec(saved)?)
}
