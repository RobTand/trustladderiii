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
