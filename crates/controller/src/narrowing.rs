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
/// The list is a hint for the audit; it names where to look, not what the brief must say.
pub(crate) fn referenced_sections(answer: &str, body: &str) -> Vec<String> {
    let words = |text: &str| {
        let plain: String = text
            .chars()
            .map(|c| {
                if c.is_alphanumeric() {
                    c.to_ascii_lowercase()
                } else {
                    ' '
                }
            })
            .collect();
        format!(
            " {} ",
            plain.split_whitespace().collect::<Vec<_>>().join(" ")
        )
    };
    let answer = words(answer);
    let mut sections: Vec<String> = Vec::new();
    let mut fenced = false;
    for line in body.lines() {
        if line.trim_start().starts_with("```") {
            fenced = !fenced;
        }
        let hashes = line.chars().take_while(|c| *c == '#').count();
        let text = line[hashes..].trim().trim_end_matches('#').trim();
        if fenced
            || !(1..=6).contains(&hashes)
            || !line[hashes..].starts_with(' ')
            || text.is_empty()
        {
            continue;
        }
        let heading = words(text);
        let first = heading.split_whitespace().next().unwrap_or("");
        let referred = answer.contains(&heading)
            || (first.len() >= 4 && answer.contains(&format!(" {first} ")));
        if referred && !sections.iter().any(|seen| seen == text) {
            sections.push(text.to_string());
        }
    }
    sections
}

fn clipped(text: &str) -> String {
    let text = text.trim();
    if text.chars().count() <= BODY_CHARS {
        return text.to_string();
    }
    format!(
        "{}\n[truncated]",
        text.chars().take(BODY_CHARS).collect::<String>()
    )
}

fn headline(kind: &str) -> &'static str {
    match kind {
        "audit_disagreement" => "Two readings of the request still disagreed.",
        "needs_splitting" => {
            "The request needed more than one change, so TL3 asked Rob to narrow it."
        }
        "cant_meet" => "TL3 could not finish the request with the approach it tried.",
        "out_of_credit" => "The request had used its allowance for further attempts.",
        "effect_failed" => "A required action needed Rob's help.",
        "cleanup_failed" | "cleanup_cant_finish" => "TL3 could not finish cleaning up the work.",
        _ => "TL3 asked Rob a question about the request.",
    }
}

fn question_text(question: &Value) -> String {
    let id = question["id"]
        .as_u64()
        .map_or(String::new(), |id| format!(" {id}"));
    let mut text = format!(
        "Question{id}: {}",
        headline(question["kind"].as_str().unwrap_or(""))
    );
    let pieces: Vec<String> = question["pieces"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|piece| {
            piece
                .as_str()
                .map_or_else(|| piece.to_string(), str::to_string)
        })
        .map(|piece| format!("- {}", piece.chars().take(500).collect::<String>()))
        .collect();
    if !pieces.is_empty() {
        text += &format!("\nPieces TL3 proposed:\n{}", pieces.join("\n"));
    }
    let report = &question["findings"];
    if let Some(report) = report["report"]
        .as_str()
        .or_else(|| report["reading"].as_str())
    {
        text += &format!(
            "\nWhat TL3 found:\n{}",
            report.chars().take(2000).collect::<String>()
        );
    }
    text
}

/// The issue body the ingest and audit agents read for a narrowed request.
pub(crate) fn issue_body(
    answer: &str,
    title: &str,
    original: &str,
    question: Option<&Value>,
) -> String {
    let mut body = format!("## {ANSWER}\n{}\n\n", clipped(answer));
    let sections = referenced_sections(answer, original);
    if !sections.is_empty() {
        let list: Vec<_> = sections
            .iter()
            .map(|section| format!("- {section}"))
            .collect();
        body += &format!("## {REFERRED}\n{}\n\n", list.join("\n"));
    }
    if let Some(question) = question {
        body += &format!("## {QUESTION}\n{}\n\n", question_text(question));
    }
    body + &format!("## {ORIGINAL}\nTitle: {title}\n\n{}", clipped(original))
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
        assert!(
            has("Protocol `prismabuild.gpu_lease.v1`"),
            "'the protocol section': {sections:?}"
        );
        assert!(
            has("Acceptance tests"),
            "'their acceptance tests': {sections:?}"
        );
        assert!(
            !has("Problem") && !has("Names"),
            "unreferenced: {sections:?}"
        );
    }

    #[test]
    fn the_brief_input_carries_scope_context_and_question_in_order() {
        let question = json!({"id": 20, "kind": "needs_splitting",
            "pieces": ["Fence and offer", "Publisher and docs"]});
        let body = issue_body(ANSWER_Q20, "Defer new GPU claims", ISSUE, Some(&question));
        let order = [ANSWER, REFERRED, QUESTION, ORIGINAL].map(|label| {
            body.find(&format!("## {label}\n"))
                .unwrap_or_else(|| panic!("{label}: {body}"))
        });
        assert!(order.windows(2).all(|pair| pair[0] < pair[1]), "{order:?}");
        assert!(body.starts_with(&format!("## {ANSWER}\n{}", ANSWER_Q20.trim())));
        assert!(body.contains("Question 20") && body.contains("narrow"));
        assert!(body.contains("- Fence and offer"));
        assert!(body.contains("Title: Defer new GPU claims\n\n## Problem"));
        assert!(
            body.contains("`TTL_S = 30`"),
            "the protocol's liveness rules survive"
        );
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
