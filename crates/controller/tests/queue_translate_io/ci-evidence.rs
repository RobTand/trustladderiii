use super::*;
use tl3_controller::findings::{CiFailure, FailedTest, QueueCiEvidence};
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
fn bounded(mut details: QueueCiEvidence) -> QueueCiEvidence {
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
fn outside_change(
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
