//! #449: the ingest input of a request that Rob's answer narrowed or changed.
//!
//! The new request is built from the answer, so the answer is the scope. It often refers to
//! sections of the original issue instead of restating them, so the issue's current title and
//! body ride along as context, with the question the answer replied to.

use serde_json::Value;

/// Labels the ingest and audit prompts name; change them together.
pub(crate) const ANSWER: &str =
    "Rob's answer (the new scope; it wins where it conflicts with the original issue)";
pub(crate) const REFERRED: &str = "Sections of the original issue that the answer refers to";
pub(crate) const QUESTION: &str = "Question Rob answered";
pub(crate) const ORIGINAL: &str = "Original issue, narrowed by Rob's answer above";

/// GitHub's own limit for an issue body or a comment.
const BODY_CHARS: usize = 65_536;

/// Headings of `body` that `answer` refers to, by full heading text or by the heading's first word.
pub(crate) fn referenced_sections(answer: &str, body: &str) -> Vec<String> {
    let _ = (answer, body);
    todo!("#449")
}

/// The issue body the ingest and audit agents read for a narrowed request.
pub(crate) fn issue_body(
    answer: &str,
    title: &str,
    original: &str,
    question: Option<&Value>,
) -> String {
    let _ = (answer, title, original, question, BODY_CHARS);
    todo!("#449")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const ISSUE: &str = include_str!("../tests/fixtures/narrowing/issue-2626.md");
    const ANSWER_Q20: &str = include_str!("../tests/fixtures/narrowing/answer-q20.md");

    #[test]
    fn answer_refers_to_the_sections_it_names_by_heading_or_phrase() {
        let sections = referenced_sections(ANSWER_Q20, ISSUE);
        let has = |prefix: &str| sections.iter().any(|s| s.starts_with(prefix));
        assert!(has("Behaviour wanted"), "named in quotes: {sections:?}");
        assert!(has("Protocol `prismabuild.gpu_lease.v1`"), "'the protocol section': {sections:?}");
        assert!(has("Acceptance tests"), "'their acceptance tests': {sections:?}");
        assert!(!has("Problem") && !has("Directory"), "unreferenced: {sections:?}");
    }

    #[test]
    fn the_brief_input_carries_scope_context_and_question_in_order() {
        let question = json!({"id": 20, "kind": "needs_splitting",
            "pieces": ["Fence and offer", "Publisher and docs"]});
        let body = issue_body(ANSWER_Q20, "Defer new GPU claims", ISSUE, Some(&question));
        let order = [ANSWER, REFERRED, QUESTION, ORIGINAL].map(|label| {
            body.find(&format!("## {label}\n")).unwrap_or_else(|| panic!("{label}: {body}"))
        });
        assert!(order.windows(2).all(|pair| pair[0] < pair[1]), "{order:?}");
        assert!(body.starts_with(&format!("## {ANSWER}\n{}", ANSWER_Q20.trim())));
        assert!(body.contains("Question 20") && body.contains("narrow"));
        assert!(body.contains("- Fence and offer"));
        assert!(body.contains("Title: Defer new GPU claims\n\n## Problem"));
        assert!(body.contains("`TTL_S = 30`"), "the protocol's liveness rules survive");
    }

    #[test]
    fn a_comment_without_a_question_omits_that_part_and_the_original_is_bounded() {
        let long = format!("## Problem\n{}", "x".repeat(70_000));
        let body = issue_body("Do less.", "Title", &long, None);
        assert!(!body.contains(QUESTION));
        assert!(body.chars().count() < 66_500, "{}", body.chars().count());
        assert!(body.ends_with("[truncated]"));
    }
}
