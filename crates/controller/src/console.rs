//! Console commands (PRD #327, C3 and C8). The console files a typed command
//! into ingress as the `console` source. tl3d applies it as Rob: pause,
//! resume, stop, restart and answers take the same steering path as Rob's
//! classified comment, and clear incident the same path as his
//! `clear incident <id>` comment. Rob's own words go to the classifier like a
//! comment. Each command settles in one commit keyed `console:<id>`, which
//! records whether it applied or why not, and queues an audit note on the
//! issue. Replaying the command after a crash finds that commit and changes
//! nothing.

use crate::sync::mpsc::Sender;
use crate::{
    Controller, Error, Result,
    executor::{CommentKind, Delivery, Observation, Output},
    looks::Incident,
    snapshot,
    steering::{Origin, Task},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};
use tl3_kernel::{NodeId, Phase, Question};
use tl3_store::{Commit, Snapshot, Transaction};
use ts_rs::TS;

/// The ingress source for console commands.
pub const SOURCE: &str = "console";

/// What the console files, and tl3d reads back from ingress.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct Command {
    /// A lowercase UUID the browser chooses, so a retried request files once.
    pub id: String,
    /// The request's node.
    pub node: u64,
    pub action: Action,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Action {
    // Empty structs, not unit variants: serde ignores deny_unknown_fields on
    // an internally tagged unit variant.
    Pause {},
    Resume {},
    Stop {},
    Restart {},
    ClearIncident {
        incident: String,
    },
    Answer {
        question: u64,
        choice: Choice,
    },
    /// Rob deferred to TL3: apply the question's recommended option.
    Defer {
        question: u64,
    },
    /// Rob's own words, read by the comment classifier like an issue comment.
    Comment {
        text: String,
    },
    /// Rob asked for a postmortem of a closed request or one waiting for him (#403).
    Postmortem {},
}

/// One of a question's options, with the details it needs from Rob.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "answer", rename_all = "snake_case", deny_unknown_fields)]
pub enum Choice {
    Continue {},
    Stop {},
    Retry {},
    MarkHandled {},
    TryAgain { guidance: String },
    AddCredit { amount: u8 },
    ChangeRequest { text: String },
}

/// A question option's answer, without its details.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum AnswerKind {
    Continue,
    Stop,
    Retry,
    MarkHandled,
    TryAgain,
    AddCredit,
    ChangeRequest,
}

/// What an option needs from Rob before it can apply.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum Detail {
    /// The new request's text.
    Text,
    Guidance,
    /// Credit from 1 to 100.
    Amount,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, TS)]
pub struct QuestionOption {
    pub answer: AnswerKind,
    pub label: String,
    pub outcome: String,
    pub detail: Option<Detail>,
}

/// An open question as Rob sees it on the issue, with its options.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, TS)]
pub struct CardQuestion {
    /// The number Rob names when he answers on GitHub.
    pub id: u64,
    pub prompt: String,
    pub options: Vec<QuestionOption>,
    /// The index of TL3's recommended option.
    pub recommended: u32,
    pub reason: String,
    /// Why "Let TL3 decide" can't apply the recommendation; None when it can.
    pub defer_blocked: Option<String>,
    /// The one decision the question asks, from the deciding worker's findings (#322).
    pub decision: Option<String>,
    /// The deciding worker's findings and evidence, as the writer received them (#322).
    #[ts(type = "Record<string, unknown> | null")]
    pub findings: Option<Value>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum CommandStatus {
    Pending,
    Applied,
    Refused,
}

/// A filed command and how it settled.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, TS)]
pub struct CommandRecord {
    pub id: String,
    pub node: u64,
    /// None when the stored action isn't one this console knows.
    pub action: Option<Action>,
    pub status: CommandStatus,
    pub reason: Option<String>,
    pub received_ms: i64,
    pub settled_ms: Option<i64>,
}

/// The newest filed commands, newest first, inside the caller's snapshot.
pub fn recent(tx: &mut Transaction<'_>, limit: i64) -> Result<Vec<CommandRecord>> {
    Ok(tx
        .query(
            "SELECT id, node_id, action, status, reason,
                    (extract(epoch FROM received_at) * 1000)::bigint,
                    (extract(epoch FROM settled_at) * 1000)::bigint
             FROM tl3_console_commands ORDER BY received_at DESC, id LIMIT $1",
            &[&limit],
        )
        .map_err(tl3_store::Error::from)?
        .into_iter()
        .map(|row| CommandRecord {
            id: row.get(0),
            node: row.get::<_, i64>(1) as u64,
            action: serde_json::from_value(row.get(2)).ok(),
            status: match row.get::<_, &str>(3) {
                "applied" => CommandStatus::Applied,
                "refused" => CommandStatus::Refused,
                _ => CommandStatus::Pending,
            },
            reason: row.get(4),
            received_ms: row.get(5),
            settled_ms: row.get(6),
        })
        .collect())
}

/// What an answer needs from Rob before it can apply.
fn detail_of(answer: AnswerKind) -> Option<Detail> {
    match answer {
        AnswerKind::ChangeRequest => Some(Detail::Text),
        AnswerKind::TryAgain => Some(Detail::Guidance),
        AnswerKind::AddCredit => Some(Detail::Amount),
        AnswerKind::Continue | AnswerKind::Stop | AnswerKind::Retry | AnswerKind::MarkHandled => {
            None
        }
    }
}

fn detail_words(detail: Detail) -> &'static str {
    match detail {
        Detail::Text => "your new request",
        Detail::Guidance => "your guidance",
        Detail::Amount => "a credit amount",
    }
}

/// The open question with its options, for the board. The words, the
/// recommendation and the deciding worker's findings come from the facts the
/// GitHub question is written from (`adapters::github::writer_input`), so the
/// card and the issue can't disagree.
pub fn card_question(question: Question, context: Option<&Value>) -> CardQuestion {
    let ask = tl3_kernel::Notice::Ask {
        question: question.id,
    };
    // Only the question, its context and the request text shape a question's facts.
    let delivery = Delivery {
        key: String::new(),
        epoch: 0,
        output: Output::Notice {
            node: NodeId(0),
            notice: ask,
        },
        generation: 0,
        head_sha: None,
        repository: crate::Repository {
            owner: String::new(),
            name: String::new(),
            installation: 0,
        },
        issue: 0,
        pull_request: None,
        proposed: None,
        paths: vec![],
        attempt: 0,
        round: 0,
        replanned: false,
        checks: None,
        job_generation: None,
        question: Some(question),
        note: None,
        scope_removals: None,
        marker_key: None,
        question_context: context.cloned(),
        steering: None,
        request_text: None,
        evidence: None,
    };
    let facts = crate::adapters::github::writer_input(&delivery, Some(ask), "", "")
        .expect("a question's own notice always has facts")
        .facts;
    let (_, answers) = crate::steering::question(question.kind);
    let options: Vec<_> = facts["options"]
        .as_array()
        .into_iter()
        .flatten()
        .zip(answers)
        .map(|(option, answer)| {
            let answer: AnswerKind =
                serde_json::from_value(json!(answer)).expect("classifier answers are answer kinds");
            QuestionOption {
                answer,
                label: option["action"].as_str().unwrap_or_default().into(),
                outcome: option["outcome"].as_str().unwrap_or_default().into(),
                detail: detail_of(answer),
            }
        })
        .collect();
    let recommended = facts["recommended_option"].as_u64().unwrap_or(0) as usize;
    let findings = facts.get("findings").cloned();
    CardQuestion {
        id: question.id,
        prompt: facts["obstacle"].as_str().unwrap_or_default().into(),
        defer_blocked: options
            .get(recommended)
            .and_then(|option| option.detail.map(|detail| (option, detail)))
            .map(|(option, detail)| {
                format!(
                    "TL3's recommended option, \"{}\", needs {} from you.",
                    option.label,
                    detail_words(detail)
                )
            }),
        recommended: recommended as u32,
        reason: facts["recommendation_reason"]
            .as_str()
            .unwrap_or_default()
            .into(),
        decision: findings
            .as_ref()
            .and_then(|findings| findings["decision"].as_str())
            .map(str::to_owned),
        findings,
        options,
    }
}

impl Choice {
    fn kind(&self) -> AnswerKind {
        match self {
            Choice::Continue {} => AnswerKind::Continue,
            Choice::Stop {} => AnswerKind::Stop,
            Choice::Retry {} => AnswerKind::Retry,
            Choice::MarkHandled {} => AnswerKind::MarkHandled,
            Choice::TryAgain { .. } => AnswerKind::TryAgain,
            Choice::AddCredit { .. } => AnswerKind::AddCredit,
            Choice::ChangeRequest { .. } => AnswerKind::ChangeRequest,
        }
    }

    /// The classifier reply Rob's comment choosing this option would get.
    fn reply(&self, question: u64) -> Value {
        let mut reply = json!({"kind": "answer", "question_id": question.to_string()});
        let fields = serde_json::to_value(self).expect("choices serialize");
        for (key, value) in fields.as_object().expect("choices are objects") {
            let key = match key.as_str() {
                "text" => "new_text",
                key => key,
            };
            reply[key] = value.clone();
        }
        reply
    }

    fn default_for(kind: AnswerKind) -> Option<Choice> {
        Some(match kind {
            AnswerKind::Continue => Choice::Continue {},
            AnswerKind::Stop => Choice::Stop {},
            AnswerKind::Retry => Choice::Retry {},
            AnswerKind::MarkHandled => Choice::MarkHandled {},
            AnswerKind::TryAgain | AnswerKind::AddCredit | AnswerKind::ChangeRequest => {
                return None;
            }
        })
    }

    /// The option as the audit note names it.
    fn described(&self, label: &str) -> String {
        match self {
            Choice::AddCredit { amount } => format!("{label} ({amount})"),
            Choice::TryAgain { guidance } => format!("{label}{}", quoted(guidance)),
            Choice::ChangeRequest { text } => format!("{label}{}", quoted(text)),
            _ => label.into(),
        }
    }
}

/// Rob's words, indented so Markdown and mentions stay inert, as the writer
/// quotes a changed request.
fn quoted(text: &str) -> String {
    let text = text
        .replace("\r\n", "\n")
        .replace('\r', "\n")
        .replace("<!-- tl3:", "< !-- tl3:");
    let mut quoted = String::from(":\n");
    for line in text.split('\n') {
        quoted.push_str("\n    ");
        quoted.push_str(line);
    }
    quoted
}

const MAX_TEXT_CHARS: usize = 20_000;

/// A command that changes nothing, and why, in Rob's words.
struct Refused(String);

fn refuse<T>(reason: impl Into<String>) -> std::result::Result<T, Refused> {
    Err(Refused(reason.into()))
}

/// Apply one console command from ingress. Called on the controller thread.
pub(crate) fn apply(controller: &mut Controller, stored: &tl3_store::Ingress) -> Result<()> {
    let snapshot = controller.store.load()?;
    let key = format!("console:{}", stored.delivery_key);
    let event: Value = serde_json::from_slice(&stored.payload).unwrap_or(Value::Null);
    let mut commit = Commit::new(key.clone(), event.clone(), &snapshot);
    let verdict = match serde_json::from_value::<Command>(event.clone()) {
        Ok(command) if command.id == stored.delivery_key => {
            match translate(
                controller,
                &snapshot,
                &command,
                stored.sequence,
                &mut commit,
            ) {
                Ok(verdict) => verdict,
                Err(Error::Store(error)) => return Err(Error::Store(error)),
                Err(error) => refuse(format!("TL3 couldn't apply this command: {error}")),
            }
        }
        _ => refuse("TL3 couldn't read this command."),
    };
    let outcome = match verdict {
        Ok(()) => json!({"status": "applied"}),
        Err(Refused(reason)) => {
            commit = Commit::new(key, event, &snapshot);
            json!({"status": "refused", "reason": reason})
        }
    };
    let mut result = std::mem::take(&mut commit.result);
    if !result.is_object() {
        result = json!({});
    }
    result["console"] = outcome;
    commit.result = result;
    controller.store.commit(&commit)?;
    Ok(())
}

fn translate(
    controller: &mut Controller,
    snapshot: &Snapshot,
    command: &Command,
    sequence: i64,
    commit: &mut Commit,
) -> Result<std::result::Result<(), Refused>> {
    let world = snapshot::decode(snapshot)?;
    let node = NodeId(command.node);
    let Some(n) = usize::try_from(command.node)
        .ok()
        .and_then(|index| world.nodes.get(index))
        .copied()
    else {
        return Ok(refuse(format!("There is no request {}.", command.node)));
    };
    let binding = crate::request(snapshot, node)?;
    if crate::latest_request(snapshot, &binding.repo, binding.issue)
        .is_none_or(|row| row.id != node.0.to_string())
    {
        return Ok(refuse("A newer request replaced this one."));
    }
    if command.action == (Action::Postmortem {}) {
        // An internal record: nothing is posted on the issue.
        if !(n.phase.is_closed() || n.phase == tl3_kernel::Phase::AwaitingRob) {
            return Ok(refuse(
                "A postmortem can be asked for once the request has closed or is waiting for Rob.",
            ));
        }
        if controller
            .store
            .postmortems(&node.0.to_string())?
            .iter()
            .any(|record| matches!(record.status.as_str(), "owed" | "running"))
        {
            return Ok(refuse(
                "A postmortem for this request is already owed or running.",
            ));
        }
        commit.postmortems.push(crate::postmortems::owed(
            snapshot,
            node,
            format!("rob:{}", command.id),
            crate::postmortems::Reason::RobAsked,
            None,
        )?);
        return Ok(Ok(()));
    }
    let note = match &command.action {
        Action::ClearIncident { incident } => {
            match clear_incident(controller, snapshot, command, &binding, incident, commit)? {
                Ok(()) => format!("Rob cleared incident {incident} from the TL3 console."),
                Err(refused) => return Ok(Err(refused)),
            }
        }
        Action::Comment { text } => {
            if text.trim().is_empty() {
                return Ok(refuse("Write an answer first."));
            }
            if text.chars().count() > MAX_TEXT_CHARS {
                return Ok(refuse("Answers can be at most 20,000 characters."));
            }
            // GitHub's comment ids fit in 53 bits, so these never collide with one.
            let id = (1u64 << 62) | sequence as u64;
            let now = chrono::DateTime::<chrono::Utc>::from(controller.clock.now());
            let observation = Observation::GitHubComment {
                repository: binding.repo.clone(),
                issue: binding.issue,
                comment: tl3_github::Comment {
                    id,
                    body: text.clone(),
                    user: tl3_github::Actor {
                        id: controller.bootstrap.rob,
                        login: "console".into(),
                    },
                    created_at: now,
                    updated_at: now,
                    issue_url: format!(
                        "https://api.github.com/repos/{}/{}/issues/{}",
                        binding.repo.owner, binding.repo.name, binding.issue
                    ),
                },
            };
            commit.rows.push(crate::put(
                "inbox",
                format!("external:console-comment:{}", command.id),
                json!(observation),
            ));
            format!(
                "Rob answered from the TL3 console. TL3 reads it as a comment{}",
                quoted(text)
            )
        }
        action => {
            let (reply, note) = match steering_reply(&n, action) {
                Ok(steering) => steering,
                Err(refused) => return Ok(Err(refused)),
            };
            let inputs = json!({"comment": {"author": "console", "body": ""}, "state": crate::steering::state(&n)});
            if crate::adapters::workers::protocol::validate_classification(&inputs, &reply).is_err()
            {
                return Ok(refuse("TL3's rules don't allow that right now."));
            }
            let since = chrono::DateTime::<chrono::Utc>::from(controller.clock.now()).to_rfc3339();
            let key = format!("console-{}", command.id);
            let origin = Origin {
                key: &key,
                text: &note,
                comment: 0,
                since,
                issue: binding.issue,
            };
            let before = commit.rows.len();
            match controller.steer(snapshot, &world, node, &reply, &origin, commit)? {
                Some(event) => {
                    controller.apply(snapshot, world.clone(), event, commit)?;
                    let id = node.0.to_string();
                    let changed = commit.rows.iter().any(|row| {
                        matches!(row, tl3_store::RowChange::Put { table, id: row_id, .. }
                            if table == "nodes" && *row_id == id)
                    });
                    if !changed {
                        return Ok(refuse("TL3's rules don't allow that right now."));
                    }
                }
                None if commit.rows.len() == before => {
                    return Ok(refuse("TL3's rules don't allow that right now."));
                }
                None => {}
            }
            note
        }
    };
    let delivery = Delivery {
        key: format!("console-note:{}", command.id),
        epoch: snapshot.epoch.get(),
        output: Output::Comment {
            node,
            kind: CommentKind::ConsoleNote,
        },
        generation: n.generation,
        head_sha: None,
        repository: binding.repo,
        issue: binding.issue,
        pull_request: None,
        paths: vec![],
        attempt: 1,
        round: 0,
        replanned: n.replanned,
        checks: None,
        job_generation: None,
        question: None,
        marker_key: None,
        note: Some(note.clone()),
        scope_removals: None,
        question_context: None,
        request_text: binding.source,
        evidence: None,
        proposed: None,
        steering: Some(Task::ConsoleNote { text: note }),
    };
    commit.rows.push(controller.allowance_row(delivery.clone()));
    commit.effects.push(delivery.into_effect());
    Ok(Ok(()))
}

/// The classifier reply Rob's equivalent comment would get, and the audit note.
fn steering_reply(
    n: &tl3_kernel::Node,
    action: &Action,
) -> std::result::Result<(Value, String), Refused> {
    let closed = || {
        if n.phase.is_closed() {
            refuse("The request is closed.")
        } else {
            Ok(())
        }
    };
    Ok(match action {
        Action::Pause {} => {
            closed()?;
            if n.paused {
                return refuse("The request is already paused.");
            }
            (
                json!({"kind": "pause"}),
                "Rob paused this request from the TL3 console.".into(),
            )
        }
        Action::Resume {} => {
            closed()?;
            if !n.paused {
                return refuse("The request isn't paused.");
            }
            (
                json!({"kind": "resume"}),
                "Rob resumed this request from the TL3 console.".into(),
            )
        }
        Action::Stop {} => {
            closed()?;
            if matches!(n.phase, Phase::Stopping(_)) {
                return refuse("The request is already stopping.");
            }
            (
                json!({"kind": "stop"}),
                "Rob stopped this request from the TL3 console.".into(),
            )
        }
        Action::Restart {} => {
            if crate::steering::state(n)["restartable"] != true {
                return refuse(
                    "The request merged or is already being replaced, so it can't restart.",
                );
            }
            (
                json!({"kind": "restart"}),
                "Rob restarted this request from the TL3 console. Work starts again from the issue as it is now.".into(),
            )
        }
        Action::Answer { question, choice } => {
            let (label, _) = open_option(n, *question, choice.kind())?;
            match choice {
                Choice::AddCredit { amount } if !(1..=100).contains(amount) => {
                    return refuse("Credit must be from 1 to 100.");
                }
                Choice::TryAgain { guidance: text } | Choice::ChangeRequest { text }
                    if text.trim().is_empty() =>
                {
                    return refuse("This option needs your words.");
                }
                Choice::TryAgain { guidance: text } | Choice::ChangeRequest { text }
                    if text.chars().count() > MAX_TEXT_CHARS =>
                {
                    return refuse("Answers can be at most 20,000 characters.");
                }
                _ => {}
            }
            (
                choice.reply(*question),
                format!(
                    "Rob answered question {question} from the TL3 console: {}.",
                    choice.described(&label)
                ),
            )
        }
        Action::Defer { question } => {
            let current = open_question(n, *question)?;
            let card = card_question(current, None);
            if let Some(reason) = card.defer_blocked {
                return refuse(reason);
            }
            let recommended = &card.options[card.recommended as usize];
            let choice = Choice::default_for(recommended.answer)
                .expect("a recommendation without details has a default choice");
            (
                choice.reply(*question),
                format!(
                    "Rob deferred to TL3 on question {question}. TL3 applied its recommended option: {}.",
                    recommended.label
                ),
            )
        }
        Action::ClearIncident { .. } | Action::Comment { .. } | Action::Postmortem {} => {
            unreachable!("handled before steering")
        }
    })
}

fn open_question(n: &tl3_kernel::Node, question: u64) -> std::result::Result<Question, Refused> {
    if n.phase.is_closed() {
        return refuse("The request is closed.");
    }
    match n.question {
        Some(current) if current.id == question => Ok(current),
        Some(current) => refuse(format!(
            "Question {question} was answered meanwhile; question {} is open now.",
            current.id
        )),
        None => refuse(format!("Question {question} was answered meanwhile.")),
    }
}

fn open_option(
    n: &tl3_kernel::Node,
    question: u64,
    answer: AnswerKind,
) -> std::result::Result<(String, Question), Refused> {
    let current = open_question(n, question)?;
    let offered = card_question(current, None).options;
    match offered.into_iter().find(|option| option.answer == answer) {
        Some(option) => Ok((option.label, current)),
        None => refuse(format!("Question {question} doesn't offer that option.")),
    }
}

/// Rob's `clear incident <id>` on the request's issue.
fn clear_incident(
    controller: &mut Controller,
    snapshot: &Snapshot,
    command: &Command,
    binding: &crate::Request,
    id: &str,
    commit: &mut Commit,
) -> Result<std::result::Result<(), Refused>> {
    let Some(row) = snapshot
        .rows
        .iter()
        .find(|row| row.table == "incidents" && row.id == id)
    else {
        return Ok(refuse(format!("There is no incident {id}.")));
    };
    let incident: Incident = crate::decode_stored(row.value.clone())?;
    let on_issue = match incident.node {
        Some(node) => {
            let request = crate::request(snapshot, node)?;
            request.issue == binding.issue && request.repo == binding.repo
        }
        None => incident
            .input_failure
            .as_ref()
            .and_then(|failure| failure.target.as_ref())
            .is_some_and(|target| {
                target.issue == binding.issue && target.repository == binding.repo
            }),
    };
    if !on_issue {
        return Ok(refuse(format!("Incident {id} isn't on this request.")));
    }
    if !incident.open {
        return Ok(refuse(format!("Incident {id} is already cleared.")));
    }
    controller.clear_incident(
        snapshot,
        incident,
        format!("incident-clear:console-{}:{id}", command.id),
        commit,
    )?;
    Ok(Ok(()))
}

/// Coalesces wakes: rung when a wake goes out, answered when the controller
/// drains, so a burst of commands sends one wake.
#[derive(Default)]
pub(crate) struct Doorbell(AtomicBool);

impl Doorbell {
    /// Whether to send a wake now.
    pub(crate) fn ring(&self) -> bool {
        !self.0.swap(true, Ordering::SeqCst)
    }

    pub(crate) fn answer(&self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

/// tl3d's doorbell; it runs one controller per process.
static DOORBELL: Doorbell = Doorbell(AtomicBool::new(false));

/// Wakes tl3d when the console files a command. Commands filed while no
/// listener was open are drained on the wake each new session sends first.
pub struct Wakes {
    settings: tl3_config::Store,
}

impl Wakes {
    const FIRST: Duration = Duration::from_millis(100);
    const CAP: Duration = Duration::from_secs(5);
    /// A session open this long was healthy, so the next drop retries fast.
    const HEALTHY: Duration = Duration::from_secs(30);

    pub fn new(settings: tl3_config::Store) -> Self {
        Self { settings }
    }

    /// The controller is draining ingress; the next filed command wakes it again.
    pub fn answered() {
        DOORBELL.answer();
    }
}

impl crate::ingress::Source for Wakes {
    fn run(self, sender: Sender<crate::cli::Input>) -> Result<()> {
        let mut delay = Self::FIRST;
        loop {
            let opened = Instant::now();
            match tl3_store::Listener::ingress(&self.settings) {
                Ok(mut listener) => {
                    // Commands filed while no session listened drain on this wake.
                    DOORBELL.ring();
                    if sender.send(crate::cli::Input::Wake).is_err() {
                        return Ok(());
                    }
                    let error = loop {
                        match listener.wait() {
                            // A wake already waiting for the drain covers this one.
                            Ok(_) if !DOORBELL.ring() => {}
                            Ok(_) => {
                                if sender.send(crate::cli::Input::Wake).is_err() {
                                    return Ok(());
                                }
                            }
                            Err(error) => break error,
                        }
                    };
                    eprintln!("console command wake lost: {error}; reconnecting");
                    if opened.elapsed() >= Self::HEALTHY {
                        delay = Self::FIRST;
                    }
                }
                Err(error) => eprintln!("console command wake unavailable: {error}; retrying"),
            }
            std::thread::sleep(delay);
            delay = (delay * 2).min(Self::CAP);
        }
    }
}

#[cfg(test)]
mod tests;
