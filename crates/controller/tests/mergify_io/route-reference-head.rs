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
        if facts::required_missing(reports, rule.integration_id.is_some(), statuses_available) {
            ready = false;
            missing.push(expected);
        } else if reports {
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

