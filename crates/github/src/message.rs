use regex::Regex;
use std::sync::LazyLock;
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
    Regex::new(
    r"(?i)\b(?:tl3|trustladder(?:iii)?|lea(?:f|ves)|lineages?|re[ -]?plans?|measur(?:e|es|ement|ements|ing)|nodes?|generations?|effects?|outbox|settle|ingesting|auditing|unplanned|awaitingrob|stopping|ready|making|checking|reviewing|publishing|queued|rebasing|kernel|footprints?|epochs?)\b"
).unwrap()
});
static IDENTIFIER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
    r"(?x)(?:(?i:\b[0-9a-f]{7,}\b|\b[0-9a-f]{8}-[0-9a-f-]{27,}\b|\b(?:id|uuid|sha|request_id|operation_id|effect_key)\s*[:=]\s*\S+)|\b[a-z]+(?:_[a-z0-9]+)+\b|\b[a-z]+[A-Z][a-zA-Z0-9]*\b|\b[A-Z][a-z]+[A-Z][a-zA-Z0-9]*\b|\#[0-9]+|<!--)"
).unwrap()
});
static PATH: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
    r"(?ix)(?:[/\\][\w.-]+[/\\]|[a-z]:\\|~/|\.\./|\b[\w.-]+/[^\s]+|\b[\w.-]+\.(?:rs|py|ts|tsx|js|json|toml|yaml|yml|md|log|txt|sql|sh)\b)"
).unwrap()
});
static LOG: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
    r"(?ix)(?:```|\b(?:traceback|stack\s+trace|panic(?:ked)?|stdout|stderr)\b|\b(?:error|warn|info|debug|trace)\s*[:\[]|\b\d{4}-\d{2}-\d{2}[t\s]\d{2}:\d{2}|\bat\s+\S+:\d+)"
).unwrap()
});

/// A deterministic format/content check. It cannot judge whether prose is true.
pub fn check(draft: &Draft, policy: &Policy) -> Result<CheckedMessage, Vec<Violation>> {
    let mut errors = Vec::new();
    if draft.issue_title.trim().is_empty() || draft.request_quote.trim().is_empty() {
        errors.push(Violation::MissingRequest);
    } else if draft.request_quote != draft.issue_title {
        errors.push(Violation::RequestDoesNotQuoteTitle);
    }
    if draft.obstacle.trim().is_empty() {
        errors.push(Violation::MissingObstacle);
    }
    let sentences = draft
        .obstacle
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
        if option.action.trim().is_empty() {
            errors.push(Violation::MissingOptionAction(i));
        }
        if option.outcome.trim().is_empty() {
            errors.push(Violation::MissingOptionOutcome(i));
        }
    }
    if draft
        .recommended_option
        .is_none_or(|i| i >= draft.options.len())
    {
        errors.push(Violation::MissingRecommendation);
    }
    if draft.recommendation_reason.trim().is_empty() {
        errors.push(Violation::MissingRecommendationReason);
    }
    if draft.how_to_answer.trim().is_empty() {
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
    let normalized: String = rendered.nfkc().filter(|c| !matches!(*c, '\u{200b}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2060}'..='\u{206f}' | '\u{feff}')).collect();
    if VOCABULARY.is_match(&normalized) {
        errors.push(Violation::InternalVocabulary);
    }
    if IDENTIFIER.is_match(&normalized)
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
    if PATH.is_match(&normalized) {
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
