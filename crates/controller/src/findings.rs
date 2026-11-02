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

const TITLE_CHARS: usize = 120;

/// What a maker fix round is sent of the accumulated feedback (#316). Findings that a
/// later review or check superseded shrink to a one-line record: kind, id, title and the
/// round that resolved them. The latest review, the reviewed head's findings and the
/// check failure no review has superseded keep their full text. The store keeps it all.
///
/// A round is one feedback event: a review (all its findings) or a check failure.
pub(crate) fn bounded_feedback(feedback: Vec<Value>, head: Option<&str>) -> Vec<Value> {
    let reviewed = |item: &Value| {
        (item["kind"] == "review_finding")
            .then(|| item["reviewed_sha"].as_str())
            .flatten()
    };
    let last_review = feedback.iter().rposition(|item| reviewed(item).is_some());
    let last_sha = last_review.and_then(|index| reviewed(&feedback[index]));
    let last_check = feedback
        .iter()
        .rposition(|item| item["kind"] == "failed_check");
    let mut review_rounds: Vec<(&str, u64)> = Vec::new();
    let mut round = 0u64;
    let rounds: Vec<u64> = feedback
        .iter()
        .map(|item| match reviewed(item) {
            Some(sha) => match review_rounds.iter().find(|(seen, _)| *seen == sha) {
                Some((_, round)) => *round,
                None => {
                    round += 1;
                    review_rounds.push((sha, round));
                    round
                }
            },
            None => {
                round += u64::from(item["kind"] == "failed_check");
                round
            }
        })
        .collect();
    feedback
        .iter()
        .enumerate()
        .map(|(index, item)| {
            let open = match reviewed(item) {
                Some(sha) => Some(sha) == last_sha || Some(sha) == head,
                None => {
                    item["kind"] != "failed_check"
                        || last_check == Some(index) && last_check > last_review
                }
            };
            if open {
                return item.clone();
            }
            let title = item["report"]
                .as_str()
                .unwrap_or_default()
                .lines()
                .map(str::trim)
                .find(|line| !line.is_empty());
            let mut record = serde_json::json!({
                "kind": item["kind"], "goal_ids": item["goal_ids"],
                "report": clipped(title.unwrap_or("(no text)"), TITLE_CHARS),
                "resolved_round": rounds[index],
            });
            for field in ["id", "reviewed_sha"] {
                if !item[field].is_null() {
                    record[field] = item[field].clone();
                }
            }
            record
        })
        .collect()
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

#[cfg(test)]
mod bounded_feedback_defects {
    use super::*;
    use serde_json::{Value, json};

    fn review(id: &str, sha: &str, report: &str) -> Value {
        json!({"kind":"review_finding", "id":id, "reviewed_sha":sha, "report":report, "goal_ids":["g"]})
    }
    fn failed(report: &str) -> Value {
        json!({"kind":"failed_check", "report":report, "goal_ids":[]})
    }
    fn long(tag: &str) -> String {
        format!("{tag} problem: {}", "evidence ".repeat(200))
    }
    fn one_line(item: &Value, kind: &str, round: u64) {
        assert_eq!(item["kind"], kind, "{item}");
        assert_eq!(item["resolved_round"], round, "{item}");
        assert!(
            item["report"].as_str().unwrap().chars().count() <= 120,
            "{item}"
        );
        assert!(item.get("failed_tests").is_none() && item.get("outside_paths").is_none());
    }

    #[test]
    fn earlier_reviews_shrink_to_one_line_records_and_the_latest_stays_whole() {
        let feedback = vec![
            review("review-1-0", "a", &long("first")),
            review("review-1-1", "a", &long("second")),
            review("review-3-0", "b", &long("third")),
            review("review-5-0", "c", &long("fourth")),
        ];
        let sent = bounded_feedback(feedback.clone(), Some("c"));
        assert_eq!(sent.len(), 4, "no finding is dropped");
        assert_eq!(sent[3], feedback[3], "the open finding keeps its full text");
        for (item, id, sha, round, title) in [
            (&sent[0], "review-1-0", "a", 1, "first problem"),
            (&sent[1], "review-1-1", "a", 1, "second problem"),
            (&sent[2], "review-3-0", "b", 2, "third problem"),
        ] {
            one_line(item, "review_finding", round);
            assert_eq!(item["id"], id);
            assert_eq!(item["reviewed_sha"], sha, "the id keeps its commit");
            assert_eq!(item["goal_ids"], json!(["g"]));
            assert!(
                item["report"].as_str().unwrap().starts_with(title),
                "{item}"
            );
        }
    }

    #[test]
    fn a_check_failure_before_a_later_review_is_resolved() {
        let feedback = vec![
            failed(&long("old check")),
            review("review-2-0", "a", &long("review")),
            failed(&long("new check")),
        ];
        let sent = bounded_feedback(feedback.clone(), None);
        one_line(&sent[0], "failed_check", 1);
        assert!(sent[0].get("id").is_none());
        assert_eq!(sent[1], feedback[1], "the latest review is still open");
        assert_eq!(sent[2], feedback[2], "a check failure after it is open");
    }

    #[test]
    fn only_the_latest_check_failure_keeps_full_text() {
        let feedback = vec![
            failed(&long("same")),
            failed(&long("same")),
            failed(&long("last")),
        ];
        let sent = bounded_feedback(feedback.clone(), None);
        one_line(&sent[0], "failed_check", 1);
        one_line(&sent[1], "failed_check", 2);
        assert_eq!(sent[2], feedback[2]);
    }

    #[test]
    fn the_head_under_review_is_open_whatever_the_order() {
        let feedback = vec![
            review("review-1-0", "a", &long("head")),
            review("review-3-0", "b", &long("later")),
        ];
        let sent = bounded_feedback(feedback.clone(), Some("a"));
        assert_eq!(
            sent, feedback,
            "both are open: the reviewed head and the latest review"
        );
    }

    #[test]
    fn short_feedback_with_nothing_to_resolve_is_unchanged() {
        let feedback = vec![
            review("review-1-0", "a", "short"),
            review("review-1-1", "a", "also short"),
        ];
        assert_eq!(bounded_feedback(feedback.clone(), Some("a")), feedback);
        assert!(bounded_feedback(vec![], None).is_empty());
    }

    #[test]
    fn a_title_is_the_first_line_clipped() {
        let feedback = vec![
            failed(&format!("{}\nsecond line", "x".repeat(300))),
            failed("Checks failed.\nmore detail"),
            failed("last"),
        ];
        let sent = bounded_feedback(feedback, None);
        assert_eq!(sent[0]["report"].as_str().unwrap().chars().count(), 120);
        assert_eq!(sent[1]["report"], "Checks failed.");
    }
}
