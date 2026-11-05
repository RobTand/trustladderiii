use super::*;
use crate::findings::{self, CiFailure, FailedTest, QueueCiEvidence};
use regex::Regex;
use std::{collections::BTreeSet, sync::LazyLock};

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

fn safe(text: &str, cap: usize) -> String {
    findings::clipped(&crate::memory::redact_text(&terminal_text(text)), cap)
}

pub(crate) fn bounded(mut details: QueueCiEvidence) -> QueueCiEvidence {
    details.complete &=
        details.checks.len() <= 20 && details.failed_tests.len() <= findings::MAX_FAILED_TESTS;
    details.checks.truncate(20);
    details.failed_tests.truncate(findings::MAX_FAILED_TESTS);
    let mut remaining = findings::MAX_REPORT_CHARS;
    for check in &mut details.checks {
        check.name = safe(&check.name, 256.min(remaining));
        remaining = remaining.saturating_sub(check.name.chars().count());
        check.url = check.url.as_ref().map(|url| safe(url, 4096.min(remaining)));
        remaining =
            remaining.saturating_sub(check.url.as_deref().unwrap_or_default().chars().count());
        check.summary = safe(&check.summary, findings::MAX_MESSAGE_CHARS.min(remaining));
        remaining = remaining.saturating_sub(check.summary.chars().count());
    }
    for test in &mut details.failed_tests {
        let id = safe(&test.test, 4096.min(remaining));
        details.complete &= id == test.test;
        test.test = id;
        remaining = remaining.saturating_sub(test.test.chars().count());
        test.message = safe(&test.message, findings::MAX_MESSAGE_CHARS.min(remaining));
        remaining = remaining.saturating_sub(test.message.chars().count());
    }
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
    checks: &[CheckRun],
) -> QueueCiEvidence {
    let mut latest = BTreeMap::new();
    for check in checks {
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
        if check
            .app
            .as_ref()
            .is_some_and(|app| app.slug == "github-actions")
        {
            // A GitHub Actions check ID is its job ID. Only the numeric ID is
            // used to build a fixed API endpoint; external URLs are evidence.
            match github.job_log_tail(repo, check.id).await {
                Ok(log) => {
                    details.complete &= !log.truncated;
                    text.push('\n');
                    text.push_str(&log.text);
                    if log.truncated {
                        text.push_str("\nJob log truncated; earlier failures may be missing.");
                    }
                }
                Err(error) => {
                    details.complete = false;
                    text.push_str(&format!("\nJob log unavailable: {error}"));
                }
            }
        }
        let text = terminal_text(&text);
        let parsed = failures(&text);
        details.complete &= !parsed.is_empty() && parsed.len() <= findings::MAX_FAILED_TESTS;
        let log_lines = parsed
            .iter()
            .map(|test| test.message.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        let tail = text
            .chars()
            .rev()
            .take(1000)
            .collect::<String>()
            .chars()
            .rev()
            .collect::<String>();
        for test in parsed {
            if seen.insert(test.test.clone()) {
                details.failed_tests.push(test);
            }
        }
        details.checks.push(CiFailure {
            name: check.name.clone(),
            summary: format!("{}\n{log_lines}\n{tail}", summary.join("\n")),
            url: check.details_url.clone(),
        });
    }
    bounded(details)
}

pub(super) async fn read_queue(
    github: &GitHub,
    repo: &str,
    state: &tl3_mergify::Observation,
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
                    Ok(checks) => read_checks(github, repo, &checks).await,
                    Err(_) => QueueCiEvidence::default(),
                }
            }
            _ => QueueCiEvidence::default(),
        },
        None => read_checks(github, repo, &state.check_runs).await,
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
    }
    details.changed_paths = github
        .pull_request_paths(repo, state.pull_request.number)
        .await
        .ok()
        .flatten();
    bounded(details)
}

