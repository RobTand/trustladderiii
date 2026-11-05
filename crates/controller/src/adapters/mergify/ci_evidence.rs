use super::*;
use crate::findings::{self, CiFailure, FailedTest, QueueCiEvidence};
use regex::Regex;
use std::{collections::BTreeSet, sync::LazyLock};
use tl3_mergify::decide::{Conclusion, Status, required};

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
            required::evidence_failure(
                Status::parse(&check.status),
                check.conclusion.as_deref().map(Conclusion::parse),
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

    /// #503/H8: Unicode and terminal input remain total, bounded and redacted.
    /// This parser edge is intentionally tested rather than modeled by Verus.
    #[test]
    fn generated_logs_are_total_bounded_and_redacted() {
        use proptest::{
            prelude::*,
            test_runner::{Config, FileFailurePersistence, RngSeed, TestRunner},
        };
        let directory = tempfile::tempdir().unwrap();
        let regressions = directory.path().join("parser-regressions.txt");
        std::fs::write(
            &regressions,
            include_bytes!("../../../tests/head_checks_schedule/parser-regressions.txt"),
        )
        .unwrap();
        let path = Box::leak(regressions.to_string_lossy().into_owned().into_boxed_str());
        let mut runner = TestRunner::new(Config {
            cases: 64,
            rng_seed: RngSeed::Fixed(730_803),
            failure_persistence: Some(Box::new(FileFailurePersistence::Direct(path))),
            ..Config::default()
        });
        runner.run(&(prop::collection::vec(any::<char>(), 0..2048), 0usize..1000), |(chars, cap)| {
            let text: String = chars.into_iter().collect();
            let hostile = format!("{text}\npass\x1b[31mword=planted-secret\x1b[0m\nFAILED: //tests:test_a\nFAILED tests/a.py::test_b");
            let clean = terminal_text(&hostile);
            let parsed = failures(&clean);
            prop_assert!(parsed.len() <= findings::MAX_FAILED_TESTS + 1);
            let safe = safe(&hostile, cap);
            prop_assert!(safe.chars().count() <= cap);
            prop_assert!(!safe.contains("planted-secret"));
            prop_assert!(safe.chars().all(|ch| !ch.is_control() || matches!(ch, '\n' | '\t')));
            let bounded = bounded(QueueCiEvidence {
                complete: true,
                checks: (0..24).map(|_| CiFailure { name: hostile.clone(), summary: hostile.clone(), url: Some(hostile.clone()) }).collect(),
                failed_tests: parsed,
                ..Default::default()
            });
            prop_assert!(!bounded.complete);
            prop_assert!(bounded.checks.len() <= 20);
            prop_assert!(bounded.failed_tests.len() <= findings::MAX_FAILED_TESTS);
            let total: usize = bounded.checks.iter().map(|check| check.name.chars().count() + check.summary.chars().count() + check.url.as_deref().unwrap_or_default().chars().count()).sum::<usize>()
                + bounded.failed_tests.iter().map(|test| test.test.chars().count() + test.message.chars().count()).sum::<usize>();
            prop_assert!(total <= findings::MAX_REPORT_CHARS);
            Ok(())
        }).unwrap();
    }

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
            assert_eq!(safe(text, 1000), "[redacted]");
        }
        assert_eq!(safe("a\t雪\nb", 1000), "a\t雪\nb");
    }
}
