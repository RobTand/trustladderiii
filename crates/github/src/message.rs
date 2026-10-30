use pulldown_cmark::{Event, Parser, Tag, TagEnd};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, sync::LazyLock};
use unicode_normalization::UnicodeNormalization;

#[derive(Clone, Debug, Serialize, Deserialize)]
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
    /// Only these request-local issue and PR references may appear in prose.
    pub own_issue: Option<u64>,
    pub own_pull_request: Option<u64>,
    pub internal_terms: Vec<String>,
    /// Repo-relative files and directories from the request delivery.
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
    Mention(String),
    ForeignReference(String),
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
        r"(?x)(?:(?i:\b[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}\b|\b(?:id|uuid|sha|request_id|operation_id|effect_key)\s*[:=]\s*\S+)|\b[a-z]+(?:_[a-z0-9]+)+\b|<!--)",
    )
    .unwrap()
});
static LOG: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
    r"(?ix)(?:\b(?:traceback|stack\s+trace|panic(?:ked)?|stdout|stderr)\b|\b(?:error|warn|info|debug|trace)\s*[:\[]|\b\d{4}-\d{2}-\d{2}[t\s]\d{2}:\d{2}|\bat\s+\S+:\d+)"
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
    errors.extend(content_errors(&written.join("\n"), policy));
    if errors.is_empty() {
        Ok(CheckedMessage(rendered))
    } else {
        Err(errors)
    }
}

/// Notes and summaries use the same content rules without question-only fields.
pub fn check_prose(text: &str, policy: &Policy) -> Result<CheckedMessage, Vec<Violation>> {
    let mut errors = content_errors(text, policy);
    if is_blank(text) {
        errors.push(Violation::MissingObstacle);
    }
    if errors.is_empty() {
        Ok(CheckedMessage(text.into()))
    } else {
        Err(errors)
    }
}

fn content_errors(text: &str, policy: &Policy) -> Vec<Violation> {
    let normalized = normalize(text);
    let mut errors = reference_errors(text, policy);
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
                    .contains(&normalize(s).to_lowercase())
            })
    {
        errors.push(Violation::InternalNameOrId);
    }
    if normalized.split_whitespace().any(|token| {
        let punctuation = [
            '"', '\'', '`', '(', ')', '[', ']', '{', '}', ',', ';', ':', '!', '?', '*', '_', '<',
            '>', '“', '”', '‘', '’',
        ];
        let token = token
            .trim_start_matches(punctuation)
            .trim_end_matches(|c| c == '.' || punctuation.contains(&c));
        let mut token = token;
        for _ in 0..2 {
            if let Some((path, suffix)) = token.rsplit_once(':')
                && !suffix.is_empty()
                && suffix.bytes().all(|b| b.is_ascii_digit())
            {
                token = path;
            } else {
                break;
            }
        }
        is_path(token, &policy.known_paths)
    }) {
        errors.push(Violation::FilePath);
    }
    if LOG.is_match(&normalized) {
        errors.push(Violation::Log);
    }
    errors
}

static MENTION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:^|[^a-zA-Z0-9_])(?P<mention>@[a-zA-Z0-9][a-zA-Z0-9-]*(?:/[a-zA-Z0-9_][a-zA-Z0-9_-]*)?)").unwrap()
});
static REFERENCE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?P<repo>[a-zA-Z0-9_.-]+/[a-zA-Z0-9_.-]+)?#(?P<number>[0-9]+)").unwrap()
});

/// Mask code using CommonMark ranges so nested blocks and multi-backtick spans
/// have the same meaning to this rule as they do to the Markdown renderer.
fn reference_errors(text: &str, policy: &Policy) -> Vec<Violation> {
    let mut prose = text.as_bytes().to_vec();
    let mut in_code_block = false;
    for (event, range) in Parser::new(text).into_offset_iter() {
        match event {
            Event::Start(Tag::CodeBlock(_)) => {
                in_code_block = true;
                prose[range].fill(b' ');
            }
            Event::End(TagEnd::CodeBlock) => in_code_block = false,
            Event::Code(_) => prose[range].fill(b' '),
            _ if in_code_block => prose[range].fill(b' '),
            _ => {}
        }
    }
    // Complete UTF-8 ranges were replaced with ASCII spaces.
    let prose = normalize(&String::from_utf8(prose).unwrap());
    let mut errors = Vec::new();
    for capture in MENTION.captures_iter(&prose) {
        let violation = Violation::Mention(capture["mention"].to_owned());
        if !errors.contains(&violation) {
            errors.push(violation);
        }
    }
    for capture in REFERENCE.captures_iter(&prose) {
        let number = capture["number"].parse::<u64>().ok();
        if capture.name("repo").is_some()
            || number.is_none()
            || ![policy.own_issue, policy.own_pull_request].contains(&number)
        {
            let violation = Violation::ForeignReference(capture[0].to_owned());
            if !errors.contains(&violation) {
                errors.push(violation);
            }
        }
    }
    errors
}

fn is_path(token: &str, known_paths: &BTreeSet<String>) -> bool {
    let token = token.trim_end_matches('/');
    if token.contains('/')
        && known_paths.iter().any(|path| {
            let path = normalize(path);
            let path = path.trim_end_matches('/');
            path.contains('/') && path == token
        })
    {
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
