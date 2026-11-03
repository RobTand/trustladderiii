use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::BTreeSet, sync::LazyLock};
use tl3_github::message::{self, Draft, OptionDraft, Policy, Violation};

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MessageKind {
    Question,
    Clarification,
    Note,
    Summary,
}

/// The request a draft is written for. It names the writer run's operation and
/// trace tags like any other worker run's (#427); it isn't writer-input.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WriteFor {
    pub node: u64,
    pub generation: u64,
    /// `owner/name`, as the other roles' traces tag it.
    pub repo: String,
    pub issue: u64,
    pub attempt: u8,
    pub round: u8,
}

/// Protocol v1 writer-input. The run key belongs to the supervisor, not role input.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WriteRequest {
    pub kind: MessageKind,
    pub facts: Value,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub violations: Vec<String>,
    /// `None` for a write that belongs to no request.
    #[serde(skip)]
    pub request: Option<WriteFor>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision: Option<String>,
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

pub struct ProcessWriter {
    config: tl3_config::Config,
    root: std::path::PathBuf,
    prompts: bool,
    /// One store session for every draft's prompt, replaced after a failure.
    store: Option<tl3_store::Store>,
}
impl ProcessWriter {
    pub fn new(config: &tl3_config::Config, root: &std::path::Path) -> Result<Self> {
        std::fs::create_dir_all(root)?;
        Ok(Self {
            config: config.clone(),
            root: root.to_owned(),
            prompts: false,
            store: None,
        })
    }

    /// Draft with the writer's active prompt version from the store.
    pub fn with_prompts(mut self) -> Self {
        self.prompts = true;
        self
    }
}
impl Writer for ProcessWriter {
    fn draft(&mut self, key: &str, input: &WriteRequest) -> Result<WriterReply> {
        use serde_json::json;
        use std::{
            fs::{self, File},
            io::Write,
            process::{Command, Stdio},
        };
        let root = super::run_root(&self.root, key);
        fs::create_dir_all(&root)?;
        let reply = root.join("reply.json");
        if let Some(value) = super::process::read(&reply)? {
            return Ok(serde_json::from_value(value)?);
        }
        let model = self
            .config
            .models
            .writer
            .first()
            .ok_or(Error::Invalid("writer ladder is empty"))?;
        // #427: the run carries its request's operation and trace tags, like
        // every other worker run; a write for no request says so explicitly.
        let (operation, trace) = match &input.request {
            Some(at) => (
                json!({"node": at.node, "generation": at.generation, "kind": "writer"}),
                json!({"repo": at.repo, "issue": at.issue, "request": at.node,
                    "attempt": at.attempt, "round": at.round}),
            ),
            None => (
                json!({"node": 0, "generation": 0, "kind": "writer"}),
                json!({"repo": "none", "issue": "none", "request": "none",
                    "attempt": "none", "round": "none"}),
            ),
        };
        if let Err(error) =
            super::protocol::validate_inputs("writer", &serde_json::to_value(input)?)
        {
            if let Error::WorkerInputs(failure) = &error {
                super::trace::input_failure(&self.config.workers, key, operation, trace, failure);
            }
            return Err(error);
        }
        let mut request = json!({"protocol":1,"operation":operation,"trace":trace,
            "role":"writer","model":self.config.models.worker_model(model), "key_file":self.config.models.key_file,
            "inputs":input,"worktree":root});
        if self.prompts {
            let store = match &mut self.store {
                Some(store) => store,
                None => self
                    .store
                    .insert(tl3_store::Store::open(&self.config.store)?),
            };
            match crate::prompts::for_run(store, key, "writer") {
                Ok(prompt) => request["prompt"] = prompt,
                Err(error) => {
                    self.store = None;
                    return Err(error);
                }
            }
        }
        let credentials = root.join("credentials");
        if credentials.exists() {
            fs::remove_dir_all(&credentials)?;
        }
        let mut command = Command::new(&self.config.workers.python);
        super::isolate_credentials(&root, &mut request, &mut command)?;
        let mut child = command
            .args(&self.config.workers.command)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;
        let written = child
            .stdin
            .take()
            .ok_or(Error::Invalid("writer stdin missing"))?
            .write_all(&serde_json::to_vec(&request)?);
        let output = child.wait_with_output()?;
        fs::remove_dir_all(credentials)?;
        written?;
        if !output.status.success() {
            return Err(Error::Invalid("writer process failed"));
        }
        let mut result = None;
        for line in output
            .stdout
            .split(|b| *b == b'\n')
            .filter(|line| !line.is_empty())
        {
            let event = super::json::parse(line)?;
            if event["event"] == "reply" {
                if event["role"] != "writer" || result.is_some() {
                    return Err(Error::Invalid("invalid writer reply stream"));
                }
                result = Some(serde_json::from_value::<WriterReply>(
                    event["reply"].clone(),
                )?);
            }
        }
        let result = result.ok_or(Error::Invalid("writer returned no reply"))?;
        super::process::write(&reply, &result)?;
        File::open(&root)?.sync_all()?;
        Ok(result)
    }
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
                decision: input.facts["findings"]["decision"]
                    .as_str()
                    .map(str::to_owned),
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
    if input.kind == MessageKind::Summary {
        let mut text = format!(
            "Request {}.\n\n{}\n\n{}",
            crate::evidence::label(&title)?,
            input.facts["outcome"]
                .as_str()
                .unwrap_or("The request ended."),
            input.facts["recorded_summary"]
                .as_str()
                .unwrap_or("Details of checks and review were not captured in this summary.")
        );
        if let Some(removed) = input.facts["removed_changes"].as_str() {
            let removed = crate::redaction::text(removed)?;
            text.push_str("\n\n");
            text.push_str(&removed);
        }
        return Ok(text);
    }
    let blocker = blocker_anchors(&input);
    let actions = proposed_actions(&input);
    let policy = Policy {
        request_title: Some(title.clone()),
        blocker_paths: blocker
            .iter()
            .filter(|anchor| anchor.contains('/'))
            .cloned()
            .collect(),
        ..policy.clone()
    };
    let change_quote = (input.kind == MessageKind::Note)
        .then(|| input.facts["change_quote"].as_str().map(str::to_owned))
        .flatten();
    // #322: a question built on a worker's findings must name the decision it needs.
    let needs_decision = input.kind == MessageKind::Question
        && input
            .facts
            .get("findings")
            .is_some_and(serde_json::Value::is_object);
    for attempt in 0..3 {
        let reply = writer.draft(&format!("{key}:writer:{attempt}"), &input)?;
        if let WriterReply::Question(question) = &reply
            && !names_blocker(&question.obstacle, &blocker)
        {
            input.violations = vec![
                "The obstacle must name the reported blocker: include at least one full file path or named rule from facts.findings.report in one or two plain sentences. Preserve the worker's proposed decisions in the options' outcomes.".into(),
            ];
            continue;
        }
        if let WriterReply::Question(question) = &reply
            && !actions.is_empty()
            && !question.options.iter().any(|option| {
                let text = format!("{} {}", option.action, option.outcome);
                (!blocker.is_empty() && names_blocker(&text, &blocker))
                    || names_blocker(&text, &actions)
            })
        {
            input.violations = vec!["At least one option must name a reported blocker or a proposed action from facts.findings.report, so Rob can choose what to do about it.".into()];
            continue;
        }
        if input.kind == MessageKind::Clarification
            && !matches!(&reply, WriterReply::Text(text) if short_question(&text.text)
                && text.text.contains(&format!("\"{}\"", input.facts["comment_excerpt"].as_str().unwrap())))
        {
            input.violations = vec!["Write one question on one line, at most 300 characters, ending in a question mark. Quote the supplied comment excerpt exactly in double quotes.".into()];
            continue;
        }
        let checked = match &reply {
            WriterReply::Question(question) if needs_decision && question.decision.is_none() => {
                Err(vec![Violation::MissingDecision])
            }
            WriterReply::Question(question) if input.kind == MessageKind::Question => {
                let draft = Draft {
                    issue_title: title.clone(),
                    request_quote: question.request_quote.clone(),
                    obstacle: question.obstacle.clone(),
                    options: question.options.clone(),
                    recommended_option: Some(question.recommended_option),
                    recommendation_reason: question.recommendation_reason.clone(),
                    how_to_answer: question.how_to_answer.clone(),
                    decision: question.decision.clone(),
                };
                message::check(&draft, &policy).map(|checked| checked.as_str().to_owned())
            }
            WriterReply::Text(text) if input.kind != MessageKind::Question => {
                let written = &text.text;
                let written = if input.kind == MessageKind::Clarification {
                    written.replace(
                        &format!("\"{}\"", input.facts["comment_excerpt"].as_str().unwrap()),
                        "your comment",
                    )
                } else {
                    written.to_owned()
                };
                message::check_prose(&written, &policy).map(|_| text.text.clone())
            }
            _ => Err(vec![Violation::WrongReplyShape]),
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
            Err(violations) => input.violations = feedback(&reply, &violations, &policy),
        }
    }
    Err(Error::MessageRejected)
}

/// The controller's own question for when every draft fails.
pub fn fixed_question(
    input: &WriteRequest,
    decision: Option<&str>,
    policy: &Policy,
) -> Result<String> {
    let text = |field: &str| -> Result<String> {
        Ok(input.facts[field]
            .as_str()
            .ok_or(Error::Invalid("fixed question needs its facts"))?
            .to_owned())
    };
    let title = text("issue_title")?;
    let anchors = blocker_anchors(input);
    let mut draft = Draft {
        issue_title: title.clone(),
        request_quote: title.clone(),
        obstacle: text("obstacle")?,
        options: serde_json::from_value(input.facts["options"].clone())?,
        recommended_option: input.facts["recommended_option"]
            .as_u64()
            .map(|index| index as usize),
        recommendation_reason: text("recommendation_reason")?,
        how_to_answer: text("how_to_answer")?,
        decision: decision.map(str::to_owned),
    };
    let policy = Policy {
        request_title: Some(title),
        blocker_paths: anchors
            .iter()
            .filter(|anchor| anchor.contains('/'))
            .cloned()
            .collect(),
        ..policy.clone()
    };
    if let Some(report) = cant_meet_report(input) {
        // The worker normally redacts findings. This fallback never enters it.
        let sentence = sentences(report)
            .find(|sentence| names_blocker(sentence, &anchors))
            .unwrap_or_else(|| sentences(report).next().unwrap_or_default());
        let obstacle: String = crate::memory::redact_text(sentence)
            .trim()
            .chars()
            .take(message::MAX_OBSTACLE_CHARS)
            .collect();
        let safe_anchor = anchors
            .iter()
            .filter(|anchor| names_blocker(sentence, &BTreeSet::from([(*anchor).clone()])))
            .chain(anchors.iter())
            .find(|anchor| {
                anchor.chars().count() <= 200
                    && message::check_prose(
                        &format!("The reported restriction concerns {anchor}."),
                        &policy,
                    )
                    .is_ok()
            });
        if !proposed_actions(input).is_empty()
            && let (Some(anchor), Some(option)) = (safe_anchor, draft.options.first_mut())
        {
            option
                .outcome
                .push_str(&format!(" Your guidance must address {anchor}."));
        }
        draft.obstacle = obstacle;
        if names_blocker(&draft.obstacle, &anchors)
            && let Ok(checked) = message::check(&draft, &policy)
        {
            return Ok(checked.as_str().to_owned());
        }
        // Unsafe prose must not suppress the question or bypass the message checks.
        draft.obstacle = safe_anchor.map_or_else(
            || "The worker reported a restriction that needs your decision.".into(),
            |anchor| format!("I cannot finish because the reported restriction concerns {anchor}."),
        );
    }
    message::check(&draft, &policy)
        .map(|checked| checked.as_str().to_owned())
        .map_err(|_| Error::MessageRejected)
}

fn cant_meet_report(input: &WriteRequest) -> Option<&str> {
    let findings = &input.facts["findings"];
    if input.kind != MessageKind::Question
        || findings["outcome"] != "cant_meet"
        || !matches!(findings["source"].as_str(), Some("planner" | "maker"))
    {
        return None;
    }
    findings["report"].as_str()
}

fn blocker_anchors(input: &WriteRequest) -> BTreeSet<String> {
    let Some(report) = cant_meet_report(input) else {
        return BTreeSet::new();
    };
    let report = crate::memory::redact_text(report);
    let mut anchors = message::paths_in(&report);
    static RULE: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(r#"\b[Rr]ule\s+(?:[`'"]([A-Za-z0-9_-]+)[`'"]|([A-Z][A-Z0-9_]+)\b)"#)
            .unwrap()
    });
    for capture in RULE.captures_iter(&report) {
        anchors.insert(
            capture
                .get(1)
                .or_else(|| capture.get(2))
                .unwrap()
                .as_str()
                .to_owned(),
        );
    }
    if report
        .to_lowercase()
        .replace('-', " ")
        .contains("protected path")
    {
        anchors.insert("protected-path".into());
    }
    if anchors.is_empty() {
        anchors = sentences(&report)
            .next()
            .unwrap_or_default()
            .split(|c: char| !c.is_alphabetic())
            .map(str::to_lowercase)
            .filter(|word| {
                const STOP_WORDS: &[&str] = &[
                    "about",
                    "above",
                    "after",
                    "again",
                    "against",
                    "approach",
                    "asked",
                    "because",
                    "before",
                    "being",
                    "below",
                    "between",
                    "cannot",
                    "could",
                    "doing",
                    "during",
                    "every",
                    "finish",
                    "first",
                    "further",
                    "having",
                    "itself",
                    "might",
                    "other",
                    "ought",
                    "please",
                    "report",
                    "request",
                    "should",
                    "since",
                    "something",
                    "still",
                    "their",
                    "theirs",
                    "there",
                    "therefore",
                    "these",
                    "thing",
                    "those",
                    "through",
                    "tried",
                    "under",
                    "until",
                    "using",
                    "where",
                    "whether",
                    "which",
                    "while",
                    "without",
                    "would",
                    "yourself",
                ];
                word.chars().count() >= 5 && !STOP_WORDS.contains(&word.as_str())
            })
            .collect();
    }
    anchors
}

fn proposed_actions(input: &WriteRequest) -> BTreeSet<String> {
    static ACTION: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(r"(?i)\b(?:grant|narrow|accept|approve|authorize|permit|waive|exclude)\b")
            .unwrap()
    });
    cant_meet_report(input)
        .into_iter()
        .flat_map(|report| {
            ACTION
                .find_iter(report)
                .map(|action| action.as_str().to_lowercase())
        })
        .collect()
}

fn sentences(mut report: &str) -> impl Iterator<Item = &str> {
    std::iter::from_fn(move || {
        report = report.trim_start();
        if report.is_empty() {
            return None;
        }
        let end = report
            .char_indices()
            .find(|&(index, c)| {
                matches!(c, '.' | '!' | '?')
                    && report[index + c.len_utf8()..]
                        .chars()
                        .next()
                        .is_none_or(char::is_whitespace)
            })
            .map_or(report.len(), |(index, c)| index + c.len_utf8());
        let (sentence, rest) = report.split_at(end);
        report = rest;
        Some(sentence)
    })
}

fn names_blocker(obstacle: &str, anchors: &BTreeSet<String>) -> bool {
    if anchors.is_empty() {
        return true;
    }
    let paths = message::paths_in(obstacle);
    let words = obstacle.to_lowercase().replace('-', " ");
    anchors.iter().any(|anchor| {
        if anchor.contains('/') {
            paths.contains(anchor)
        } else {
            let anchor = anchor.to_lowercase().replace('-', " ");
            words.match_indices(&anchor).any(|(start, _)| {
                !words[..start]
                    .chars()
                    .next_back()
                    .is_some_and(char::is_alphanumeric)
                    && !words[start + anchor.len()..]
                        .chars()
                        .next()
                        .is_some_and(char::is_alphanumeric)
            })
        }
    })
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
            if let Some(decision) = &question.decision {
                fields.push(("decision".into(), decision.clone()));
            }
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
                Violation::WrongReplyShape => {
                    "has the wrong reply shape for the requested message kind"
                }
                Violation::Mention(token) => {
                    return format!("Remove the user or team mention '{token}'.");
                }
                Violation::ForeignReference(token) => {
                    return format!("Remove the foreign reference '{token}'; refer only to this request's issue or pull request.");
                }
                Violation::MissingDecision => {
                    return "Add the decision: the one decision Rob must make, as a single question ending in a question mark.".into();
                }
                Violation::DecisionNotOneQuestion => {
                    return "The decision must be one plain question ending in a question mark, with no other sentence.".into();
                }
                Violation::HtmlEntity(entity) => {
                    return format!("Replace the HTML entity '{entity}' in '{text}'; write the character itself, not an HTML entity.");
                }
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
