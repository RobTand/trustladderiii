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
        let failure = match self.failure.as_ref() {
            Some(Failure::Tests {
                check,
                completed_at,
            }) => Some(route::failure::Failure::Tests((
                *check,
                completed_at.clone(),
            ))),
            Some(Failure::Infrastructure(checks)) => {
                let check = route::failure::first(checks);
                Some(route::failure::Failure::Infrastructure((
                    check.id,
                    check.completed_at.map(|time| time.to_rfc3339()),
                )))
            }
            None => None,
        };
        let route::failure::Evidence {
            value: (check, completed_at),
            infra,
        } = route::failure::evidence(failure)?;
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
    let checks = if required::needs_checks(requirements.checks.is_empty()) {
        github.check_runs(repo, &pull.head.sha).await?
    } else {
        vec![]
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
    if !required::needs_checks(required.is_empty()) {
        return Ok(Checks {
            ready: true,
            unavailable,
            missing: vec![],
            reported: vec![],
            failure: None,
        });
    }
    let latest = latest_payloads(
        checks.iter().cloned(),
        |check| (check.app.as_ref().map(|app| app.id), check.name.clone()),
        |check| required::Candidate {
            id: check.id,
            eligible: check.head_sha == pull.head.sha,
        },
    );
    let mut statuses = BTreeMap::new();
    let mut statuses_available = true;
    if required::needs_statuses(
        &required
            .iter()
            .map(|rule| rule.integration_id)
            .collect::<Vec<_>>(),
    ) {
        let result = github.commit_statuses(repo, &pull.head.sha).await;
        let read = match required::status_read(match &result {
            Ok(_) => required::ReadResult::Success,
            Err(tl3_github::Error::Http { status }) => required::ReadResult::Http(*status),
            Err(_) => required::ReadResult::Other,
        }) {
            required::StatusRead::Available => result?,
            required::StatusRead::Unavailable => {
                if let Err(tl3_github::Error::Http { status }) = result {
                    unavailable.push(format!("Commit statuses: read unavailable (HTTP {status})"));
                }
                statuses_available = false;
                vec![]
            }
            required::StatusRead::Propagate => return Err(result.unwrap_err().into()),
        };
        statuses = latest_payloads(
            read,
            |status| status.context.clone(),
            |status| required::Candidate {
                id: status.id,
                eligible: true,
            },
        );
    }
    let mut summary = required::Summary {
        ready: true,
        failure: required::Failure::None,
    };
    let mut missing = Vec::new();
    let mut reported = Vec::new();
    let mut tests = None;
    let mut infrastructure = Vec::new();
    for rule in required {
        let checks: Vec<_> = latest
            .values()
            .filter(|check| {
                required::rule_matches(
                    rule.context == check.name,
                    rule.integration_id,
                    check.app.as_ref().map(|app| app.id),
                )
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
        if facts::required_missing(reports, rule.integration_id.is_some(), statuses_available) {
            summary = required::record(summary, required::Contribution::Pending);
            missing.push(expected);
        } else if reports {
            reported.push(expected);
        }
        for check in checks {
            let view = tl3_mergify::views(std::slice::from_ref(check), &pull.head.sha, |_| true)[0];
            let runner = required::needs_annotations(
                view.status == tl3_mergify::decide::Status::Completed,
                view.app == tl3_mergify::decide::App::Actions,
                view.conclusion == Some(tl3_mergify::decide::Conclusion::Failure),
            ) && runner_failure(&github.check_annotations(repo, check.id).await?);
            let item = match lifecycle::required_check(view, runner) {
                lifecycle::RequiredState::Passed => required::Contribution::Passed,
                lifecycle::RequiredState::Pending | lifecycle::RequiredState::Unknown => {
                    required::Contribution::Pending
                }
                lifecycle::RequiredState::Infrastructure => required::Contribution::Infrastructure,
                lifecycle::RequiredState::Tests => required::Contribution::Tests,
            };
            summary = required::record(summary, item);
            match item {
                required::Contribution::Infrastructure => infrastructure.push(check.clone()),
                required::Contribution::Tests => {
                    tests = Some(Failure::Tests {
                        check: check.id,
                        completed_at: check.completed_at.map(|time| time.to_rfc3339()),
                    })
                }
                _ => {}
            }
        }
        if let Some(status) = status {
            let item = required::classic_status(match status.state.as_str() {
                "success" => required::ClassicStatus::Success,
                "failure" => required::ClassicStatus::Failure,
                "error" => required::ClassicStatus::Error,
                _ => required::ClassicStatus::Other,
            });
            summary = required::record(summary, item);
            if item == required::Contribution::Tests {
                tests = Some(Failure::Tests {
                    check: status.id,
                    completed_at: status.updated_at.map(|time| time.to_rfc3339()),
                });
            }
        }
    }
    Ok(Checks {
        ready: summary.ready,
        unavailable,
        missing,
        reported,
        failure: match summary.failure {
            required::Failure::Tests => tests,
            required::Failure::Infrastructure => Some(Failure::Infrastructure(infrastructure)),
            required::Failure::None => None,
        },
    })
}

async fn failure(github: &GitHub, repo: &str, pull: &PullRequest) -> Result<Option<Failure>> {
    Ok(read(github, repo, pull).await?.failure)
}

fn latest_payloads<T, K: Ord>(
    items: impl IntoIterator<Item = T>,
    key: impl Fn(&T) -> K,
    candidate: impl Fn(&T) -> required::Candidate,
) -> BTreeMap<K, T> {
    let mut groups: BTreeMap<K, Vec<T>> = BTreeMap::new();
    for item in items {
        groups.entry(key(&item)).or_default().push(item);
    }
    groups
        .into_iter()
        .filter_map(|(key, mut items)| {
            let candidates: Vec<_> = items.iter().map(&candidate).collect();
            required::latest(&candidates, false).map(|i| (key, items.swap_remove(i)))
        })
        .collect()
}

fn annotation_class(annotation: &CheckAnnotation) -> required::Annotation {
    if annotation.annotation_level != "failure" {
        return required::Annotation::Ignore;
    }
    if [
        "The runner has lost communication with the server",
        "The job was not acquired by Runner of type",
        "The job was abandoned as the runner did not pick it up",
        "The job was not started because recent account payments have failed",
    ]
    .iter()
    .any(|prefix| annotation.message.starts_with(prefix))
    {
        required::Annotation::RunnerFailure
    } else {
        required::Annotation::OtherFailure
    }
}

fn runner_failure(annotations: &[CheckAnnotation]) -> bool {
    required::runner_failure(&annotations.iter().map(annotation_class).collect::<Vec<_>>())
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
        let failure = self
            .runtime
            .block_on(failure(&github, &repo, &state.pull_request))?;
        let class = match &failure {
            Some(Failure::Infrastructure(_)) => required::Failure::Infrastructure,
            Some(Failure::Tests { .. }) => required::Failure::Tests,
            None => required::Failure::None,
        };
        if !required::rerun(class) {
            return Ok(());
        }
        let Some(Failure::Infrastructure(checks)) = failure else {
            unreachable!()
        };
        let (number, head) = enqueue(delivery)?;
        let runs = self
            .runtime
            .block_on(github.workflow_runs(&repo, head))
            .map_err(Error::from)?;
        let mut selected = BTreeMap::new();
        for check in checks {
            let candidates: Vec<_> = runs
                .iter()
                .map(|run| required::Candidate {
                    id: run.id,
                    eligible: required::matching_suite(
                        run.head_sha == head,
                        check.check_suite.as_ref().map(|suite| suite.id),
                        run.check_suite_id,
                    ),
                })
                .collect();
            let index = required::latest(&candidates, true).ok_or(Error::Invalid(
                "required check has no matching head workflow run",
            ))?;
            let run = &runs[index];
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
                            writable = Some(required::writable(
                                pull.head.sha == run.head_sha,
                                pull.state == "open",
                                pull.merged_at.is_some(),
                            ));
                            read_pull = true;
                        }
                        lifecycle::Effect::Error(failure) => return Err(lifecycle_error(failure)),
                        lifecycle::Effect::SaveRerun => {
                            if required::needs_fence(core.rerun.map(|rerun| rerun.waiting)) {
                                self.fence(delivery)?;
                            }
                            save_rerun_projection(self, &path, &mut saved, &run, core.rerun)?;
                        }
                        lifecycle::Effect::Write(lifecycle::Write::Rerun) => {
                            let result = self
                                .runtime
                                .block_on(github.rerun_workflow_run(&repo, run.id));
                            let dispatch = match &result {
                                Ok(()) => required::DispatchResult::Accepted,
                                Err(tl3_github::Error::ActionsForbidden(
                                    tl3_github::ActionsRefusal::RunInProgress,
                                )) => required::DispatchResult::InProgress,
                                Err(tl3_github::Error::ActionsForbidden(_)) => {
                                    required::DispatchResult::Forbidden
                                }
                                Err(tl3_github::Error::RateLimited { .. }) => {
                                    required::DispatchResult::RateLimited
                                }
                                Err(tl3_github::Error::Http { status }) => {
                                    required::DispatchResult::Http(*status)
                                }
                                Err(_) => required::DispatchResult::Other,
                            };
                            let outcome = match required::rerun_result(dispatch) {
                                required::RerunResult::Accepted => lifecycle::RerunResult::Accepted,
                                required::RerunResult::InProgress => {
                                    lifecycle::RerunResult::InProgress
                                }
                                required::RerunResult::Refused => lifecycle::RerunResult::Refused,
                                required::RerunResult::Unknown => lifecycle::RerunResult::Unknown,
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
                if required::rerun_loop(read_pull) == required::Loop::Done {
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
