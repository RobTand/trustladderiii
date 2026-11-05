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

