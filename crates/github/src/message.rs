use regex::Regex;
use std::{collections::BTreeSet, sync::LazyLock};
use unicode_normalization::UnicodeNormalization;

#[derive(Clone, Debug)]
pub struct OptionDraft {
    pub action: String,
    pub outcome: String,
}

/// The issue title comes from the original issue, not the message writer.
#[derive(Clone, Debug)]
pub struct Draft {
    pub issue_title: String,
    pub request_quote: String,
    pub obstacle: String,
    pub options: Vec<OptionDraft>,
    pub recommended_option: Option<usize>,
    pub recommendation_reason: String,
    pub how_to_answer: String,
}

/// Add current internal identifiers and names, including request and operation ids.
#[derive(Clone, Debug, Default)]
pub struct Policy {
    pub internal_terms: Vec<String>,
    /// Repo-relative files and directories in the served repo and TL3's repo.
    pub known_paths: BTreeSet<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Violation {
    MissingRequest,
    RequestDoesNotQuoteTitle,
    MissingObstacle,
    ObstacleTooLong,
    OptionCount,
    MissingOptionAction(usize),
    MissingOptionOutcome(usize),
    MissingRecommendation,
    MissingRecommendationReason,
    MissingAnswerInstructions,
    InternalVocabulary,
    InternalNameOrId,
    FilePath,
    Log,
}

#[derive(Clone, Debug)]
pub struct CheckedMessage(String);

impl CheckedMessage {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

static VOCABULARY: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(?:leaf|lineages?|re-?plan(?:s|ned|ning)?|measures?)\b").unwrap()
});
static HEX_ID: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\b[0-9a-f]{7,}\b").unwrap());
static IDENTIFIER: LazyLock<Regex> = LazyLock::new(|| {
    // CamelCase alone also describes product names. Actual internal names belong in Policy.
    Regex::new(
        r"(?x)(?:(?i:\b[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}\b|\b(?:id|uuid|sha|request_id|operation_id|effect_key)\s*[:=]\s*\S+)|\b[a-z]+(?:_[a-z0-9]+)+\b|\#[0-9]+|<!--)",
    )
    .unwrap()
});
static LOG: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
    r"(?ix)(?:```|\b(?:traceback|stack\s+trace|panic(?:ked)?|stdout|stderr)\b|\b(?:error|warn|info|debug|trace)\s*[:\[]|\b\d{4}-\d{2}-\d{2}[t\s]\d{2}:\d{2}|\bat\s+\S+:\d+)"
).unwrap()
});

/// A deterministic format/content check. It cannot judge whether prose is true.
pub fn check(draft: &Draft, policy: &Policy) -> Result<CheckedMessage, Vec<Violation>> {
    let mut errors = Vec::new();
    if is_blank(&draft.issue_title) || is_blank(&draft.request_quote) {
        errors.push(Violation::MissingRequest);
    } else if draft.request_quote != draft.issue_title {
        errors.push(Violation::RequestDoesNotQuoteTitle);
    }
    if is_blank(&draft.obstacle) {
        errors.push(Violation::MissingObstacle);
    }
    let obstacle = normalize(&draft.obstacle);
    let sentences = obstacle
        .split_terminator(['.', '!', '?'])
        .filter(|s| !s.trim().is_empty())
        .count();
    if sentences > 2 {
        errors.push(Violation::ObstacleTooLong);
    }
    if !(2..=3).contains(&draft.options.len()) {
        errors.push(Violation::OptionCount);
    }
    for (i, option) in draft.options.iter().enumerate() {
        if is_blank(&option.action) {
            errors.push(Violation::MissingOptionAction(i));
        }
        if is_blank(&option.outcome) {
            errors.push(Violation::MissingOptionOutcome(i));
        }
    }
    if draft
        .recommended_option
        .is_none_or(|i| i >= draft.options.len())
    {
        errors.push(Violation::MissingRecommendation);
    }
    if is_blank(&draft.recommendation_reason) {
        errors.push(Violation::MissingRecommendationReason);
    }
    if is_blank(&draft.how_to_answer) {
        errors.push(Violation::MissingAnswerInstructions);
    }
    let mut rendered = format!(
        "You asked for \"{}\".\n\n{}\n",
        draft.request_quote, draft.obstacle
    );
    for (i, option) in draft.options.iter().enumerate() {
        rendered.push_str(&format!(
            "\n{}. {}: {}\n",
            i + 1,
            option.action,
            option.outcome
        ));
    }
    if let Some(i) = draft.recommended_option {
        rendered.push_str(&format!(
            "\nI recommend option {}. {}\n",
            i.saturating_add(1),
            draft.recommendation_reason
        ));
    }
    rendered.push_str(&format!("\n{}", draft.how_to_answer));
    let mut written = vec![draft.obstacle.as_str()];
    for option in &draft.options {
        written.extend([option.action.as_str(), option.outcome.as_str()]);
    }
    written.extend([
        draft.recommendation_reason.as_str(),
        draft.how_to_answer.as_str(),
    ]);
    let normalized = normalize(&written.join("\n"));
    if VOCABULARY.is_match(&normalized) {
        errors.push(Violation::InternalVocabulary);
    }
    if IDENTIFIER.is_match(&normalized)
        || HEX_ID.find_iter(&normalized).any(|token| {
            let bytes = token.as_str().as_bytes();
            bytes.iter().any(u8::is_ascii_digit) && bytes.iter().any(u8::is_ascii_alphabetic)
        })
        || policy
            .internal_terms
            .iter()
            .filter(|s| !s.is_empty())
            .any(|s| {
                normalized
                    .to_lowercase()
                    .contains(&s.nfkc().collect::<String>().to_lowercase())
            })
    {
        errors.push(Violation::InternalNameOrId);
    }
    if normalized.split_whitespace().any(|token| {
        let punctuation = [
            '"', '\'', '`', '(', ')', '[', ']', '{', '}', ',', ';', ':', '!', '?',
        ];
        let token = token
            .trim_start_matches(punctuation)
            .trim_end_matches(|c| c == '.' || punctuation.contains(&c));
        is_path(token, &policy.known_paths)
    }) {
        errors.push(Violation::FilePath);
    }
    if LOG.is_match(&normalized) {
        errors.push(Violation::Log);
    }
    if errors.is_empty() {
        Ok(CheckedMessage(rendered))
    } else {
        Err(errors)
    }
}

fn is_path(token: &str, known_paths: &BTreeSet<String>) -> bool {
    if known_paths.contains(token) {
        return true;
    }
    if ["/", "~/", "./", "../"].iter().any(|prefix| {
        token
            .strip_prefix(prefix)
            .is_some_and(|rest| rest.split('/').any(|segment| !segment.is_empty()))
    }) {
        return true;
    }
    token.rsplit_once('/').is_some_and(|(_, last)| {
        last.rsplit_once('.').is_some_and(|(_, extension)| {
            (1..=5).contains(&extension.len())
                && extension
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
        })
    })
}

fn normalize(text: &str) -> String {
    text.nfkc()
        .filter(|c| {
            !matches!(*c,
                '\u{200b}'..='\u{200f}' | '\u{202a}'..='\u{202e}' |
                '\u{2060}'..='\u{206f}' | '\u{feff}'
            )
        })
        .collect()
}

fn is_blank(text: &str) -> bool {
    normalize(text).trim().is_empty()
}
