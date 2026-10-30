use serde::{Deserialize, Serialize};

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

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Kind {
    FailedCheck,
    ReviewFinding,
}

#[derive(Serialize)]
pub(crate) struct Finding {
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

pub(crate) fn ci_report(checks: &[CiFailure]) -> String {
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
        .collect::<Vec<_>>()
        .join("\n")
}

// Bound feedback persisted from external reports and sent back to the maker.
pub const MAX_FAILED_TESTS: usize = 20;
pub(crate) const MAX_REVIEW_FINDINGS: usize = 20;
pub(crate) const MAX_OUTSIDE_PATHS: usize = 100;
pub(crate) const MAX_PATH_CHARS: usize = 4096;
pub const MAX_MESSAGE_CHARS: usize = 1000;
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
