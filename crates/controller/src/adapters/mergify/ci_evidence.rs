use super::*;
use crate::findings::{
    self, CiEvidenceGap, CiFailure, CiGapReason, CiNodeStatus, CiRetrieval, FailedTest,
    QueueCiEvidence,
};
use regex::Regex;
use std::{collections::BTreeSet, sync::LazyLock};

mod remote;
#[cfg(test)]
mod remote_tests;

fn terminal_text(text: &str) -> String {
    enum State {
        Text,
        Escape,
        Csi,
        String,
        StringEscape,
    }
    let mut state = State::Text;
    let mut clean = String::with_capacity(text.len());
    for ch in text.chars() {
        state = match state {
            State::Text => match ch {
                '\x1b' => State::Escape,
                '\u{009b}' => State::Csi,
                '\u{0090}' | '\u{0098}' | '\u{009d}' | '\u{009e}' | '\u{009f}' => State::String,
                _ => {
                    if ch == '\r' {
                        clean.push('\n');
                    } else if !ch.is_control() || matches!(ch, '\n' | '\t') {
                        clean.push(ch);
                    }
                    State::Text
                }
            },
            State::Escape => match ch {
                '[' => State::Csi,
                ']' | 'P' | 'X' | '^' | '_' => State::String,
                '\x1b' | ' '..='/' => State::Escape,
                _ => State::Text,
            },
            State::Csi => {
                if ('@'..='~').contains(&ch) {
                    State::Text
                } else {
                    State::Csi
                }
            }
            State::String => match ch {
                '\x07' | '\u{009c}' => State::Text,
                '\x1b' => State::StringEscape,
                _ => State::String,
            },
            State::StringEscape => match ch {
                '\\' | '\u{009c}' => State::Text,
                '\x1b' => State::StringEscape,
                _ => State::String,
            },
        };
    }
    clean
}

fn safe(text: &str, cap: usize) -> std::result::Result<String, CiGapReason> {
    static URL: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r#"(?i)https?://[^\s<>"']+"#).unwrap());
    let text = terminal_text(text);
    let text = text
        .lines()
        .filter(|line| !line.to_ascii_lowercase().contains("x-buildbuddy-api-key"))
        .collect::<Vec<_>>()
        .join("\n");
    let text = URL.replace_all(&text, "[private URL omitted]");
    let text = crate::redaction::text(&text).map_err(|_| CiGapReason::RedactionUnavailable)?;
    Ok(findings::clipped(&text, cap))
}

fn public_url(text: &str) -> Option<String> {
    let text = terminal_text(text);
    if crate::redaction::text(&text).ok()?.as_str() != text {
        return None;
    }
    let url = reqwest::Url::parse(&text).ok()?;
    (url.scheme() == "https"
        && url.host_str() == Some("github.com")
        && url.username().is_empty()
        && url.password().is_none()
        && url.query().is_none()
        && url.fragment().is_none()
        && url.port().is_none()
        && text.len() <= 4096)
        .then_some(text)
}

pub(crate) fn bounded(mut details: QueueCiEvidence) -> QueueCiEvidence {
    let was_complete = details.complete;
    let mut redaction_error = None;
    let mut redact = |text: &str, cap| match safe(text, cap) {
        Ok(text) => text,
        Err(reason) => {
            redaction_error = Some(reason);
            String::new()
        }
    };
    details.complete &=
        details.checks.len() <= 20 && details.failed_tests.len() <= findings::MAX_FAILED_TESTS;
    details.checks.truncate(20);
    details.failed_tests.truncate(findings::MAX_FAILED_TESTS);
    let mut remaining = findings::MAX_REPORT_CHARS;
    for check in &mut details.checks {
        check.name = redact(&check.name, 256.min(remaining));
        remaining = remaining.saturating_sub(check.name.chars().count());
        check.url = check
            .url
            .as_deref()
            .and_then(public_url)
            .filter(|url| url.chars().count() <= remaining);
        remaining =
            remaining.saturating_sub(check.url.as_deref().unwrap_or_default().chars().count());
        check.summary = redact(&check.summary, findings::MAX_MESSAGE_CHARS.min(remaining));
        remaining = remaining.saturating_sub(check.summary.chars().count());
    }
    for test in &mut details.failed_tests {
        let id = redact(&test.test, 4096.min(remaining));
        details.complete &= id == test.test;
        test.test = id;
        remaining = remaining.saturating_sub(test.test.chars().count());
        test.message = redact(&test.message, findings::MAX_MESSAGE_CHARS.min(remaining));
        remaining = remaining.saturating_sub(test.message.chars().count());
    }
    details.complete &= details.target_results.len() <= 16;
    details.target_results.truncate(16);
    let mut node_count = 0;
    for result in &mut details.target_results {
        for field in [
            &mut result.repository,
            &mut result.sha,
            &mut result.parent_invocation,
            &mut result.invocation,
            &mut result.target,
            &mut result.check_name,
        ] {
            let value = redact(field, 4096.min(remaining));
            details.complete &= value == *field;
            *field = value;
            remaining = remaining.saturating_sub(field.chars().count());
        }
        let available = 200usize.saturating_sub(node_count);
        details.complete &= result.nodes.len() <= available;
        result.nodes.truncate(available);
        node_count += result.nodes.len();
        for node in &mut result.nodes {
            let id = redact(&node.test, 4096.min(remaining));
            details.complete &= id == node.test;
            node.test = id;
            remaining = remaining.saturating_sub(node.test.chars().count());
            node.message = redact(&node.message, findings::MAX_MESSAGE_CHARS.min(remaining));
            remaining = remaining.saturating_sub(node.message.chars().count());
        }
        result.excerpt = redact(&result.excerpt, 2000.min(remaining));
        remaining = remaining.saturating_sub(result.excerpt.chars().count());
    }
    details.retrievals.truncate(72);
    details.complete &= redaction_error.is_none();
    if was_complete && !details.complete {
        details.gaps.push(CiEvidenceGap {
            check: details
                .target_results
                .first()
                .map_or(0, |result| result.check),
            reason: redaction_error.unwrap_or(CiGapReason::Oversized),
        });
    }
    details.gaps.truncate(20);
    details
}

fn failures(text: &str) -> Vec<FailedTest> {
    static FAILED: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r"(?:^|\s)FAILED(?: \((?:rust|python)\))?:?\s+(//[^\s]+|[^\s]+\.py(?:::[^\s]+)?)",
        )
        .unwrap()
    });
    let mut seen = BTreeSet::new();
    text.lines()
        .filter_map(|line| {
            let capture = FAILED.captures(line)?;
            let id = capture[1].trim_end_matches(['\r', ',']);
            seen.insert(id.to_owned()).then(|| FailedTest {
                test: id.into(),
                message: line.into(),
            })
        })
        .take(findings::MAX_FAILED_TESTS + 1)
        .collect()
}

pub(super) async fn read_checks(
    github: &GitHub,
    repo: &str,
    head: &str,
    checks: &[CheckRun],
    access: &tl3_config::BuildBuddy,
) -> QueueCiEvidence {
    let client = remote::Client::configured(access).ok();
    read_checks_inner(github, repo, head, checks, client.as_ref()).await
}

#[cfg(test)]
async fn read_checks_with(
    github: &GitHub,
    repo: &str,
    head: &str,
    checks: &[CheckRun],
    remote: &remote::Client,
) -> QueueCiEvidence {
    read_checks_inner(github, repo, head, checks, Some(remote)).await
}

async fn job_binding(
    github: &GitHub,
    repo: &str,
    head: &str,
    check: &CheckRun,
    budget: &remote::Budget,
) -> std::result::Result<tl3_github::WorkflowJob, CiGapReason> {
    let job = tokio::time::timeout(budget.remaining_time(), github.workflow_job(repo, check.id))
        .await
        .map_err(|_| CiGapReason::RetrievalLimit)?
        .map_err(|_| CiGapReason::Unavailable)?;
    let expected_url = format!(
        "https://github.com/{repo}/actions/runs/{}/job/{}",
        job.run_id, job.id
    );
    if check
        .details_url
        .as_deref()
        .is_none_or(|url| !url.eq_ignore_ascii_case(&expected_url))
        || job.id != check.id
        || job.name != check.name
        || job.head_sha != head
        || job.run_id == 0
        || job.run_attempt == 0
        || job.status != "completed"
        || job.conclusion != check.conclusion
    {
        return Err(CiGapReason::WrongCiRun);
    }
    Ok(job)
}

fn aggregate_reference(check: &CheckRun, log: &str) -> bool {
    let language = match check.name.as_str() {
        "Python" => "python",
        "Rust" => "rust",
        _ => return false,
    };
    let expected = format!("The Bazel gate failed {language}: see the Bazel job");
    log.lines()
        .rev()
        .any(|line| line.trim_end().ends_with(&expected))
}

async fn read_checks_inner(
    github: &GitHub,
    repo: &str,
    head: &str,
    checks: &[CheckRun],
    remote: Option<&remote::Client>,
) -> QueueCiEvidence {
    static INVOCATION: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r"Remote Bazel run: https://app\.buildbuddy\.io/invocation/([a-zA-Z0-9-]{1,128})\b",
        )
        .unwrap()
    });
    let mut latest = BTreeMap::new();
    for check in checks.iter().filter(|check| check.head_sha == head) {
        let entry = latest
            .entry((check.app.as_ref().map(|app| app.id), &check.name))
            .or_insert(check);
        if check.id > entry.id {
            *entry = check;
        }
    }
    let failed: Vec<_> = latest
        .values()
        .copied()
        .filter(|check| {
            check.status == "completed"
                && matches!(
                    check.conclusion.as_deref(),
                    Some(
                        "failure"
                            | "timed_out"
                            | "cancelled"
                            | "startup_failure"
                            | "action_required"
                            | "stale"
                    )
                )
        })
        .collect();
    let mut details = QueueCiEvidence {
        complete: failed.len() <= 20 && !failed.is_empty(),
        ..Default::default()
    };
    let mut seen = BTreeSet::new();
    let mut budget = remote::Budget::default();
    let mut resolved_bazel = BTreeMap::new();
    let mut aggregates = Vec::new();
    for check in failed.into_iter().take(20) {
        let mut summary = vec![format!(
            "Conclusion: {}",
            check.conclusion.as_deref().unwrap_or("unknown")
        )];
        if let Some(output) = &check.output {
            summary.extend(output.title.iter().cloned());
            summary.extend(output.summary.iter().cloned());
            summary.extend(output.text.iter().cloned());
        }
        let mut text = summary.join("\n");
        let mut prefix = String::new();
        let mut truncated = false;
        let mut gap = None;
        let mut aggregate = false;
        if check
            .app
            .as_ref()
            .is_some_and(|app| app.slug == "github-actions")
        {
            let log =
                tokio::time::timeout(budget.remaining_time(), github.job_log_tail(repo, check.id))
                    .await;
            details.retrievals.push(CiRetrieval {
                check: check.id,
                method: "GitHubJobLog".into(),
                attempt: 1,
                error: match &log {
                    Ok(Ok(_)) => None,
                    Ok(Err(_)) => Some(CiGapReason::Unavailable),
                    Err(_) => Some(CiGapReason::RetrievalLimit),
                },
            });
            match log {
                Ok(Ok(log)) => {
                    aggregate =
                        !log.truncated && aggregate_reference(check, &terminal_text(&log.text));
                    prefix = log.prefix;
                    text.push('\n');
                    text.push_str(&log.text);
                    truncated = log.truncated;
                }
                Ok(Err(_)) => gap = Some(CiGapReason::Unavailable),
                Err(_) => gap = Some(CiGapReason::RetrievalLimit),
            }
        }
        text = terminal_text(&text);
        let parsed = failures(&text);
        let expected: BTreeSet<_> = parsed
            .iter()
            .filter(|test| test.test.starts_with("//"))
            .map(|test| test.test.clone())
            .collect();
        let remote_log = format!("{}\n{}", terminal_text(&prefix), text);
        let parents: BTreeSet<_> = INVOCATION
            .captures_iter(&remote_log)
            .map(|cap| cap[1].to_owned())
            .collect();
        let mut resolved = false;
        let binding = if remote.is_some() && (parents.len() == 1 || aggregate) {
            let binding = job_binding(github, repo, head, check, &budget).await;
            details.retrievals.push(CiRetrieval {
                check: check.id,
                method: "GitHubJob".into(),
                attempt: 1,
                error: binding.as_ref().err().copied(),
            });
            Some(binding)
        } else {
            None
        };
        if !expected.is_empty() || !parents.is_empty() {
            gap = match (parents.len(), remote) {
                (1, Some(client)) if binding.as_ref().is_some_and(|value| value.is_ok()) => {
                    let job = binding.as_ref().unwrap().as_ref().unwrap();
                    let resolution = client
                        .resolve(
                            repo,
                            head,
                            job,
                            parents.first().unwrap(),
                            &expected,
                            &mut budget,
                        )
                        .await;
                    details.retrievals.extend(resolution.retrievals);
                    details.target_results.extend(resolution.results);
                    resolved = resolution.gap.is_none();
                    if resolved && check.name == "Bazel" {
                        resolved_bazel.insert((job.run_id, job.run_attempt), check.id);
                    }
                    resolution.gap
                }
                (1, Some(_)) => binding
                    .as_ref()
                    .and_then(|value| value.as_ref().err())
                    .copied()
                    .or(Some(CiGapReason::WrongCiRun)),
                (1, None) => Some(CiGapReason::AccessUnavailable),
                _ => Some(CiGapReason::MissingInvocation),
            };
        } else if gap.is_none() && (truncated || parsed.is_empty()) {
            gap = Some(CiGapReason::IncompleteJobLog);
        }
        if resolved {
            for node in details
                .target_results
                .iter()
                .filter(|result| result.check == check.id)
                .flat_map(|result| &result.nodes)
            {
                if matches!(node.status, CiNodeStatus::Failed | CiNodeStatus::Errored)
                    && seen.insert(node.test.clone())
                {
                    details.failed_tests.push(FailedTest {
                        test: node.test.clone(),
                        message: node.message.clone(),
                    });
                }
            }
        } else {
            for test in parsed {
                if seen.insert(test.test.clone()) {
                    details.failed_tests.push(test);
                }
            }
        }
        if aggregate && let Some(Ok(job)) = &binding {
            aggregates.push((
                check.id,
                job.run_id,
                job.run_attempt,
                gap.unwrap_or(CiGapReason::IncompleteJobLog),
            ));
            gap = None;
        } else if aggregate && let Some(Err(reason)) = &binding {
            gap = Some(*reason);
        }
        if let Some(reason) = gap {
            details.complete = false;
            details.gaps.push(CiEvidenceGap {
                check: check.id,
                reason,
            });
        }
        let diagnostic = if resolved {
            details
                .target_results
                .iter()
                .filter(|result| result.check == check.id)
                .map(|result| {
                    format!(
                        "{} shard {} run {} attempt {}: {}",
                        result.target, result.shard, result.run, result.attempt, result.excerpt
                    )
                })
                .collect::<Vec<_>>()
                .join("\n")
        } else {
            text
        };
        let remove_key = |text: &str| match remote {
            Some(remote) => remote.remove_key(text),
            None => text.to_owned(),
        };
        details.checks.push(CiFailure {
            name: remove_key(&check.name),
            summary: remove_key(&if resolved {
                format!("Resolved remote target evidence:\n{diagnostic}")
            } else {
                format!("{}\n{diagnostic}", summary.join("\n"))
            }),
            url: check
                .details_url
                .as_deref()
                .map(remove_key)
                .as_deref()
                .and_then(public_url),
        });
    }
    for (check, run, attempt, reason) in aggregates {
        if !resolved_bazel.contains_key(&(run, attempt)) {
            details.complete = false;
            details.gaps.push(CiEvidenceGap { check, reason });
        }
    }
    if let Some(remote) = remote {
        for result in &mut details.target_results {
            for field in [
                &mut result.repository,
                &mut result.sha,
                &mut result.parent_invocation,
                &mut result.invocation,
                &mut result.target,
                &mut result.check_name,
            ] {
                let value = remote.remove_key(field);
                details.complete &= value == *field;
                *field = value;
            }
        }
        for test in &mut details.failed_tests {
            test.test = remote.remove_key(&test.test);
            test.message = remote.remove_key(&test.message);
        }
    }
    bounded(details)
}

pub(super) async fn read_queue(
    github: &GitHub,
    repo: &str,
    state: &tl3_mergify::Observation,
    access: &tl3_config::BuildBuddy,
) -> QueueCiEvidence {
    static DRAFT: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"\bdraft #([1-9][0-9]*)\b").unwrap());
    let draft = state
        .check
        .as_ref()
        .and_then(|check| check.output.as_ref())
        .and_then(|output| output.summary.as_deref())
        .and_then(|summary| DRAFT.captures_iter(summary).last())
        .and_then(|capture| capture[1].parse::<u64>().ok());
    let mut details = match draft {
        Some(number) => match github.pull_request(repo, number).await {
            Ok(pull) if is_merge_queue_pr(&pull) => {
                match github.check_runs(repo, &pull.head.sha).await {
                    Ok(checks) => read_checks(github, repo, &pull.head.sha, &checks, access).await,
                    Err(_) => QueueCiEvidence::default(),
                }
            }
            _ => QueueCiEvidence::default(),
        },
        None => {
            read_checks(
                github,
                repo,
                &state.pull_request.head.sha,
                &state.check_runs,
                access,
            )
            .await
        }
    };
    if details.checks.is_empty()
        && let Some(check) = &state.check
    {
        details.checks.push(CiFailure {
            name: check.name.clone(),
            summary: check
                .output
                .as_ref()
                .map(|output| {
                    format!(
                        "{}\n{}",
                        output.title.as_deref().unwrap_or_default(),
                        output.summary.as_deref().unwrap_or_default()
                    )
                })
                .unwrap_or_else(|| "Queue check failed".into()),
            url: check.details_url.clone(),
        });
        details.complete = false;
        details.gaps.push(CiEvidenceGap {
            check: check.id,
            reason: CiGapReason::MissingInvocation,
        });
    }
    details.changed_paths = github
        .pull_request_paths(repo, state.pull_request.number)
        .await
        .ok()
        .flatten();
    bounded(details)
}

fn test_path(repo: &Repository, id: &str) -> Option<String> {
    let path = if let Some(label) = id.strip_prefix("//") {
        let (package, target) = label.split_once(':')?;
        if !target
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        {
            return None;
        }
        if repo.owner.eq_ignore_ascii_case("RobTand")
            && repo.name.eq_ignore_ascii_case("prismabuild")
            && package == "tests"
            && target.starts_with("test_")
        {
            format!("tests/{target}.py")
        } else if repo.owner.eq_ignore_ascii_case("RobTand")
            && repo.name.eq_ignore_ascii_case("trustladderiii")
            && package.starts_with("crates/")
            && package.split('/').count() == 2
        {
            format!("{package}/tests/{}.rs", target.strip_prefix("test_test_")?)
        } else {
            return None;
        }
    } else {
        let path = id.split("::").next()?;
        if !path.ends_with(".py") {
            return None;
        }
        path.into()
    };
    (!path.starts_with('/')
        && path.split('/').all(|part| !matches!(part, "" | "." | ".."))
        && !path.contains('\\'))
    .then_some(path)
}

/// Classification is controller input translation. The kernel still owns
/// the enqueue, failure-round and credit transitions.
pub(crate) fn outside_change(
    repo: &Repository,
    details: &QueueCiEvidence,
    plan: Option<&crate::adapters::prismabuild::CheckRun>,
) -> bool {
    let (Some(paths), Some(run)) = (&details.changed_paths, plan) else {
        return false;
    };
    details.complete
        && !details.failed_tests.is_empty()
        && details.failed_tests.iter().all(|test| {
            let Some(path) = test_path(repo, &test.test) else {
                return false;
            };
            !paths.iter().any(|changed| {
                changed == &path
                    || path
                        .strip_suffix(".rs")
                        .is_some_and(|path| changed == &format!("{path}/main.rs"))
            }) && run.plan.iter().all(|check| {
                !details
                    .checks
                    .iter()
                    .any(|failed| failed.name == check.name)
                    && check.tests.iter().all(|named| {
                        named != &test.test
                            && named.split("::").next() != Some(path.as_str())
                            && test_path(repo, named).as_ref() != Some(&path)
                    })
            })
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_codes_are_removed_before_redaction_and_clipping() {
        for text in [
            "pass\x1b[31mword=planted-secret\x1b[0m",
            "\x1b]window title\x07password=planted-secret",
            "\x1b]window title\x1b\\password=planted-secret",
            "\x1bPdevice data\x1b\\password=planted-secret",
            "pass\0\x08\x7fword=planted-secret",
            "\u{009b}31mpassword=planted-secret\u{009b}0m",
        ] {
            assert_eq!(
                safe(text, 1000).unwrap(),
                crate::redaction::text("password=planted-secret").unwrap()
            );
        }
        assert_eq!(safe("a\t雪\nb", 1000).unwrap(), "a\t雪\nb");
    }
}
