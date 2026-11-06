pub use tl3_controller::findings::ReviewFixes;
use serde::{Deserialize, Serialize};
use serde_json::Value;

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
pub struct QueueCiEvidence {
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

pub const MAX_FAILED_TESTS: usize = 20;
pub(crate) const MAX_REVIEW_FINDINGS: usize = 20;
pub const MAX_MESSAGE_CHARS: usize = crate::contracts::MESSAGE_CHARS;
pub(crate) const MAX_REPORT_CHARS: usize = 16000;
pub(crate) fn clipped(text: &str, cap: usize) -> String {
    text.chars().take(cap).collect()
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
