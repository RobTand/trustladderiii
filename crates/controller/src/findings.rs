use serde::{Deserialize, Serialize};
use serde_json::Value;

/// #838 / Q7: the store boundary guarantees an array of finding objects.
/// Keep the remaining fields as JSON: older evidence may contain nulls,
/// extensions and number representations that narrowing would rewrite.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReviewFixes {
    pub findings: Vec<serde_json::Map<String, Value>>,
    #[serde(flatten)]
    pub fields: serde_json::Map<String, Value>,
}

impl ReviewFixes {
    pub fn parse(value: Value) -> crate::Result<Self> {
        serde_json::from_value(value).map_err(|_| crate::Error::ReviewFixes)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FailedTest {
    pub test: String,
    pub message: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CiFailure {
    pub name: String,
    pub summary: String,
    pub url: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct CiEvidence {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cause: Option<Box<CauseEvidence>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fingerprint: Option<String>,
    #[serde(default)]
    pub checks: Vec<CiFailure>,
    #[serde(default)]
    pub failed_tests: Vec<FailedTest>,
    #[serde(default)]
    pub changed_paths: Option<Vec<String>>,
    #[serde(default)]
    pub complete: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Kind {
    FailedCheck,
    ReviewFinding,
}

#[derive(Serialize)]
pub(crate) struct Finding {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reviewed_sha: Option<String>,
    pub kind: Kind,
    pub report: String,
    pub goal_ids: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub failed_tests: Vec<FailedTest>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub outside_paths: Vec<String>,
    pub outside_paths_total: usize,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub ci_checks: Vec<CiFailure>,
}

pub(crate) fn ci_report(checks: &[CiFailure], tests: &[FailedTest]) -> String {
    if checks.is_empty() {
        return "CI failed for the queued change. The upstream supplied no check details.".into();
    }
    checks
        .iter()
        .map(|check| {
            let link = check
                .url
                .as_ref()
                .map(|url| format!(" ({url})"))
                .unwrap_or_default();
            format!("CI check {} failed{link}: {}", check.name, check.summary)
        })
        .chain(
            tests
                .iter()
                .map(|test| format!("{}: {}", test.test, test.message)),
        )
        .collect::<Vec<_>>()
        .join("\n")
}

// Bound feedback persisted from external reports and sent back to the maker.
pub const MAX_FAILED_TESTS: usize = 20;
pub(crate) const MAX_REVIEW_FINDINGS: usize = 20;
pub(crate) const MAX_OUTSIDE_PATHS: usize = 100;
pub(crate) const MAX_PATH_CHARS: usize = 4096;
pub const MAX_MESSAGE_CHARS: usize = crate::contracts::MESSAGE_CHARS;
pub(crate) const MAX_REPORT_CHARS: usize = 16000;

pub(crate) fn clipped(text: &str, cap: usize) -> String {
    text.chars().take(cap).collect()
}

pub(crate) fn check_report(
    pass: bool,
    in_footprint: bool,
    tests: &[FailedTest],
    paths: &[String],
    total_paths: usize,
) -> String {
    let mut lines = vec![];
    if !pass {
        if tests.is_empty() {
            lines.push("Checks failed. The upstream supplied no failing test details.".to_owned());
        } else {
            lines.extend(
                tests
                    .iter()
                    .map(|test| format!("{}: {}", test.test, test.message)),
            );
        }
    }
    if !in_footprint {
        lines.push(if paths.is_empty() {
            "The change includes files outside the plan. The upstream supplied no changed paths."
                .into()
        } else {
            format!(
                "Files outside the plan (showing {} of {total_paths}): {}",
                paths.len(),
                paths.join(", ")
            )
        });
    }
    clipped(&lines.join("\n"), MAX_REPORT_CHARS)
}

pub(crate) struct CurrentReview<'a> {
    pub head: Option<&'a str>,
    pub findings: Vec<&'a serde_json::Value>,
}

pub(crate) fn current_review<'a>(
    feedback: &'a serde_json::Value,
    round: u8,
    head: Option<&'a str>,
) -> CurrentReview<'a> {
    let head = head.filter(|_| round == 1);
    let findings = feedback
        .as_array()
        .into_iter()
        .flatten()
        .filter(|finding| {
            finding["kind"] == "review_finding"
                && head.is_some_and(|sha| finding["reviewed_sha"] == sha)
        })
        .collect();
    CurrentReview { head, findings }
}

#[cfg(test)]
mod selection_defects {
    use super::*;
    use serde_json::json;
    #[test]
    fn current_findings_require_matching_reviewed_head_and_kind() {
        let feedback = json!([
            {"kind":"review_finding", "id":"current", "reviewed_sha":"a"},
            {"kind":"review_finding", "id":"old", "reviewed_sha":"b"},
            {"kind":"failed_check", "id":"check", "reviewed_sha":"a"},
            {"kind":"review_finding", "id":"missing"}
        ]);
        for (round, head) in [(0, Some("a")), (0, None), (1, None)] {
            let current = current_review(&feedback, round, head);
            assert!(current.head.is_none());
            assert!(current.findings.is_empty());
        }
        let current = current_review(&feedback, 1, Some("a"));
        assert_eq!(current.head, Some("a"));
        assert_eq!(current.findings, vec![&feedback[0]]);
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum CauseEvidence {
    Change {
        fingerprint: String,
    },
    Environment {
        fingerprint: String,
        base: String,
        witness: String,
    },
    Unattributed,
}

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
    clipped(&crate::memory::redact_text(&terminal_text(text)), cap)
}

pub(crate) fn bounded_ci_evidence(mut details: CiEvidence) -> CiEvidence {
    details.complete &=
        details.checks.len() <= 20 && details.failed_tests.len() <= MAX_FAILED_TESTS;
    details.checks.truncate(20);
    details.failed_tests.truncate(MAX_FAILED_TESTS);
    let mut remaining = MAX_REPORT_CHARS;
    for check in &mut details.checks {
        check.name = safe(&check.name, 256.min(remaining));
        remaining = remaining.saturating_sub(check.name.chars().count());
        check.url = check.url.as_ref().map(|url| safe(url, 4096.min(remaining)));
        remaining =
            remaining.saturating_sub(check.url.as_deref().unwrap_or_default().chars().count());
        check.summary = safe(&check.summary, MAX_MESSAGE_CHARS.min(remaining));
        remaining = remaining.saturating_sub(check.summary.chars().count());
    }
    for test in &mut details.failed_tests {
        let id = safe(&test.test, 4096.min(remaining));
        details.complete &= id == test.test;
        test.test = id;
        remaining = remaining.saturating_sub(test.test.chars().count());
        test.message = safe(&test.message, MAX_MESSAGE_CHARS.min(remaining));
        remaining = remaining.saturating_sub(test.message.chars().count());
    }
    if !details.complete {
        details.fingerprint = None;
    }
    details
}
