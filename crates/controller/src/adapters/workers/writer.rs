use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tl3_github::message::{self, Draft, OptionDraft, Policy, Violation};

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MessageKind {
    Question,
    Clarification,
    Note,
    Summary,
}

/// Protocol v1 writer-input. The run key belongs to the supervisor, not role input.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WriteRequest {
    pub kind: MessageKind,
    pub facts: Value,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub violations: Vec<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QuestionDraft {
    pub request_quote: String,
    pub obstacle: String,
    pub options: Vec<OptionDraft>,
    pub recommended_option: usize,
    pub recommendation_reason: String,
    pub how_to_answer: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TextDraft {
    pub text: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum WriterReply {
    Question(QuestionDraft),
    Text(TextDraft),
}

pub trait Writer {
    fn draft(&mut self, key: &str, input: &WriteRequest) -> Result<WriterReply>;
}

/// Fixed, deterministic simulation of the writer role. It executes no model.
pub struct FakeWriter;
impl Writer for FakeWriter {
    fn draft(&mut self, _: &str, input: &WriteRequest) -> Result<WriterReply> {
        let title = input.facts["issue_title"]
            .as_str()
            .ok_or(Error::Invalid("writer needs the original issue title"))?;
        if input.facts["request"]
            .as_str()
            .is_none_or(|request| request.trim().is_empty())
        {
            return Err(Error::Invalid("writer needs the original request"));
        }
        match input.kind {
            MessageKind::Question => Ok(WriterReply::Question(QuestionDraft {
                request_quote: title.into(),
                obstacle: input.facts["obstacle"]
                    .as_str()
                    .ok_or(Error::Invalid("missing obstacle"))?
                    .into(),
                options: serde_json::from_value(input.facts["options"].clone())?,
                recommended_option: input.facts["recommended_option"]
                    .as_u64()
                    .ok_or(Error::Invalid("missing recommendation"))?
                    as usize,
                recommendation_reason: input.facts["recommendation_reason"]
                    .as_str()
                    .ok_or(Error::Invalid("missing recommendation reason"))?
                    .into(),
                how_to_answer: input.facts["how_to_answer"]
                    .as_str()
                    .ok_or(Error::Invalid("missing answer instructions"))?
                    .into(),
            })),
            MessageKind::Clarification => Ok(WriterReply::Text(TextDraft {
                text: format!(
                    "About \"{}\", {}",
                    input.facts["comment_excerpt"].as_str().unwrap(),
                    input.facts["question"].as_str().unwrap()
                ),
            })),
            MessageKind::Note | MessageKind::Summary => {
                let field = if input.kind == MessageKind::Note {
                    "change"
                } else {
                    "outcome"
                };
                let fact = input.facts[field]
                    .as_str()
                    .ok_or(Error::Invalid("missing message fact"))?;
                let mut text = format!("You asked for \"{title}\".\n\n{fact}");
                if let Some(goals) = input.facts["unmet_goals"].as_array() {
                    for goal in goals {
                        if let Some(goal) = goal.as_str() {
                            text.push_str(&format!("\n{goal}"));
                        }
                    }
                }
                Ok(WriterReply::Text(TextDraft { text }))
            }
        }
    }
}

pub fn checked_draft(
    writer: &mut dyn Writer,
    key: &str,
    mut input: WriteRequest,
    policy: &Policy,
) -> Result<String> {
    if input.kind == MessageKind::Question {
        let options = input.facts["options"]
            .as_array()
            .ok_or(Error::Invalid("question needs options"))?;
        let recommendation = input.facts["recommended_option"]
            .as_u64()
            .ok_or(Error::Invalid("question needs a recommendation"))?;
        if !(2..=3).contains(&options.len()) || recommendation >= options.len() as u64 {
            return Err(Error::Invalid(
                "question recommendation is outside its options",
            ));
        }
    }
    let title = input.facts["issue_title"]
        .as_str()
        .ok_or(Error::Invalid("writer needs the original issue title"))?
        .to_owned();
    let change_quote = (input.kind == MessageKind::Note)
        .then(|| input.facts["change_quote"].as_str().map(str::to_owned))
        .flatten();
    for attempt in 0..3 {
        let reply = writer.draft(&format!("{key}:writer:{attempt}"), &input)?;
        if input.kind == MessageKind::Clarification
            && !matches!(&reply, WriterReply::Text(text) if short_question(&text.text)
                && text.text.contains(&format!("\"{}\"", input.facts["comment_excerpt"].as_str().unwrap())))
        {
            input.violations = vec!["Write one question on one line, at most 300 characters, ending in a question mark. Quote the supplied comment excerpt exactly in double quotes.".into()];
            continue;
        }
        let checked = match &reply {
            WriterReply::Question(question) if input.kind == MessageKind::Question => {
                let draft = Draft {
                    issue_title: title.clone(),
                    request_quote: question.request_quote.clone(),
                    obstacle: question.obstacle.clone(),
                    options: question.options.clone(),
                    recommended_option: Some(question.recommended_option),
                    recommendation_reason: question.recommendation_reason.clone(),
                    how_to_answer: question.how_to_answer.clone(),
                };
                message::check(&draft, policy).map(|checked| checked.as_str().to_owned())
            }
            WriterReply::Text(text) if input.kind != MessageKind::Question => {
                let prefix = format!("You asked for \"{title}\".");
                let written = text.text.strip_prefix(&prefix).unwrap_or(&text.text);
                let written = if input.kind == MessageKind::Clarification {
                    written.replace(
                        &format!("\"{}\"", input.facts["comment_excerpt"].as_str().unwrap()),
                        "your comment",
                    )
                } else {
                    written.to_owned()
                };
                message::check_prose(&written, policy).map(|_| text.text.clone())
            }
            _ => Err(vec![Violation::MissingRequest]),
        };
        match checked {
            Ok(mut text) => {
                if let Some(quote) = &change_quote {
                    // Indentation keeps Markdown inert; embedded markers must also be neutralised.
                    let quote = quote
                        .replace("\r\n", "\n")
                        .replace('\r', "\n")
                        .replace("<!-- tl3:", "< !-- tl3:");
                    let mut chars = quote.chars();
                    let quote: String = chars.by_ref().take(2_000).collect();
                    let shortened = chars.next().is_some();
                    text = format!("{}\n\nYour change:\n", text.trim_end());
                    for line in quote.split('\n') {
                        text.push_str("\n    ");
                        text.push_str(line);
                    }
                    if shortened {
                        text.push_str("\n\n[Change shortened to 2,000 characters.]");
                    }
                }
                return Ok(text);
            }
            Err(violations) => input.violations = feedback(&reply, &violations, policy),
        }
    }
    Err(Error::MessageRejected)
}

fn feedback(reply: &WriterReply, violations: &[Violation], policy: &Policy) -> Vec<String> {
    let fields = match reply {
        WriterReply::Text(text) => vec![("message".to_owned(), text.text.clone())],
        WriterReply::Question(question) => {
            let mut fields = vec![("obstacle".into(), question.obstacle.clone())];
            for (index, option) in question.options.iter().enumerate() {
                fields.push((
                    format!("option {} action", index + 1),
                    option.action.clone(),
                ));
                fields.push((
                    format!("option {} outcome", index + 1),
                    option.outcome.clone(),
                ));
            }
            fields.push((
                "recommendation reason".into(),
                question.recommendation_reason.clone(),
            ));
            fields.push(("answer instructions".into(), question.how_to_answer.clone()));
            fields
        }
    };
    violations
        .iter()
        .map(|violation| {
            let (field, text) = fields
                .iter()
                .find(|(_, text)| {
                    message::check_prose(text, policy)
                        .is_err_and(|errors| errors.contains(violation))
                })
                .cloned()
                .unwrap_or_else(|| {
                    (
                        "draft".into(),
                        serde_json::to_string(reply).unwrap_or_default(),
                    )
                });
            let problem = match violation {
                Violation::InternalVocabulary => "uses an internal word",
                Violation::InternalNameOrId => "contains an internal name or identifier",
                Violation::FilePath => "contains a file path",
                Violation::Log => "contains log text",
                Violation::MissingRequest | Violation::RequestDoesNotQuoteTitle => {
                    "does not quote the original issue title exactly"
                }
                Violation::MissingObstacle => "has no obstacle text",
                Violation::ObstacleTooLong => "has more than two obstacle sentences",
                Violation::OptionCount => "does not contain two or three options",
                Violation::MissingOptionAction(_) => "has an option without an action",
                Violation::MissingOptionOutcome(_) => "has an option without an outcome",
                Violation::MissingRecommendation => "has a recommendation outside the options",
                Violation::MissingRecommendationReason => "has no recommendation reason",
                Violation::MissingAnswerInstructions => "has no answer instructions",
            };
            let offending = text
                .split_whitespace()
                .find(|token| {
                    message::check_prose(token, policy)
                        .is_err_and(|errors| errors.contains(violation))
                })
                .unwrap_or(&text)
                .trim_matches(['.', '!', '?', '"', '\'']);
            format!("The {field} {problem} '{offending}'.")
        })
        .collect()
}

fn short_question(text: &str) -> bool {
    !text.trim().is_empty()
        && text.chars().count() <= 300
        && !text.contains(['\r', '\n', '\u{0085}', '\u{2028}', '\u{2029}'])
        && text.trim_end().ends_with('?')
        && text.matches('?').count() == 1
}
