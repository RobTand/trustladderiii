// #854: #447-only suite completion and TL3-owned queue-entry repair.
use super::*;
use tl3_mergify::decide::queue_entry as decision;

pub(super) async fn suites_completed(
    github: &GitHub,
    repo: &str,
    state: &tl3_mergify::Observation,
    suites: &[tl3_github::CheckSuite],
) -> Result<bool> {
    if suites.is_empty() {
        return Ok(false);
    }
    for suite in suites {
        if suite.head_sha != state.pull_request.head.sha {
            return Ok(false);
        }
        if suite.status == "completed" {
            continue;
        }
        let completed_after = state.check.as_ref().is_some_and(|check| {
            check.head_sha == suite.head_sha
                && check.status == "completed"
                && check
                    .completed_at
                    .zip(suite.updated_at)
                    .is_some_and(|(completed, updated)| completed >= updated)
        });
        if !decision::empty_suite(suite.status == "queued", completed_after, true) {
            return Ok(false);
        }
        let empty = github.check_suite_has_no_runs(repo, suite.id).await?;
        if !decision::empty_suite(true, completed_after, empty) {
            return Ok(false);
        }
    }
    Ok(true)
}

/// A stored PR binding, not its branch name or author text, establishes ownership.
/// Queue-entry attribution is separate from dequeued CI failure classification.
pub(super) async fn failure(
    github: &GitHub,
    repo: &Repository,
    state: &tl3_mergify::Observation,
    conditions: Vec<String>,
    failed: Vec<String>,
    snapshot: Option<&Snapshot>,
) -> Result<QueueFact> {
    let mut fact = QueueFact {
        repository: repo.clone(),
        number: state.pull_request.number,
        evidence: Evidence::QueueBlocked {
            head: state.pull_request.head.sha.clone(),
            conditions: conditions.clone(),
            failed: failed.clone(),
        },
    };
    let completed_queue = state.check.as_ref().is_some_and(|check| {
        check.status == "completed" && check.head_sha == state.pull_request.head.sha
    });
    // Parse the displayed queue condition at this boundary. Missing scopes
    // evidence failed detection/gates (#854); unrelated labels do not.
    let failed_conditions = !failed.is_empty()
        && conditions.iter().any(|condition| {
            let matches = |description: &str| {
                condition == description
                    || condition
                        .strip_prefix(description)
                        .is_some_and(|tail| tail.starts_with(": "))
            };
            matches("scopes-available")
                || state
                    .check_runs
                    .iter()
                    .filter(|check| failed.contains(&check.name))
                    .any(|check| {
                        ["check-success", "check-neutral", "check-skipped"]
                            .iter()
                            .any(|kind| {
                                matches(&format!("{kind} = {}", check.name))
                                    || check.app.as_ref().is_some_and(|app| {
                                        matches(&format!("{kind} = @{}/{}", app.slug, check.name))
                                    })
                            })
                    })
        });
    let ours = if completed_queue && failed_conditions {
        snapshot
            .filter(|snapshot| {
                snapshot
                    .rows
                    .iter()
                    .any(|row| row.table == "pull_requests" && row.value["number"] == fact.number)
            })
            .map(|snapshot| fact.node(snapshot))
            .transpose()?
            .flatten()
            .is_some()
    } else {
        false
    };
    if decision::repair(ours, completed_queue, failed_conditions) {
        let check = state.check.as_ref().expect("completed queue check");
        fact.evidence = Evidence::CiFailed {
            check: check.id,
            completed_at: check.completed_at.map(|time| time.to_rfc3339()),
            head: state.pull_request.head.sha.clone(),
            infra: false,
            head_check: true,
            details: Default::default(),
        };
        if !snapshot
            .map(|snapshot| fact.recorded(snapshot))
            .transpose()?
            .unwrap_or(false)
        {
            // Read only the non-required failures responsible for this queue-entry fact.
            let checks: Vec<_> = state
                .check_runs
                .iter()
                .filter(|check| {
                    check.head_sha == state.pull_request.head.sha && failed.contains(&check.name)
                })
                .cloned()
                .collect();
            let mut details = ci_evidence::read_checks(github, &full_name(repo), &checks).await;
            details.checks.insert(
                0,
                crate::findings::CiFailure {
                    name: check.name.clone(),
                    url: check.details_url.clone(),
                    summary: format!("Unmet queue-entry conditions: {}", conditions.join("; ")),
                },
            );
            if let Evidence::CiFailed {
                details: target, ..
            } = &mut fact.evidence
            {
                *target = ci_evidence::bounded(details);
            }
        }
    }
    Ok(fact)
}
