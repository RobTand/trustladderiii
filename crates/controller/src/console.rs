//! Console commands (PRD #327, C3 and C8). The console files a typed command
//! into ingress as the `console` source. tl3d applies it as Rob: pause,
//! resume, stop, restart and answers take the same steering path as Rob's
//! classified comment, and clear incident the same path as his
//! `clear incident <id>` comment. Rob's own words go to the classifier like a
//! comment. Each command settles in one commit keyed `console:<id>`, which
//! records whether it applied or why not, and queues an audit note on the
//! issue. Replaying the command after a crash finds that commit and changes
//! nothing.

use crate::{Controller, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::mpsc::Sender;
use tl3_kernel::Question;
use tl3_store::Transaction;
use ts_rs::TS;
#[cfg(test)]
use {serde_json::json, tl3_kernel::QKind};

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
    Pause,
    Resume,
    Stop,
    Restart,
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
}

/// One of a question's options, with the details it needs from Rob.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "answer", rename_all = "snake_case", deny_unknown_fields)]
pub enum Choice {
    Continue,
    Stop,
    Retry,
    MarkHandled,
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
pub fn recent(_tx: &mut Transaction<'_>, _limit: i64) -> Result<Vec<CommandRecord>> {
    todo!()
}

/// The open question with its options, for the board.
pub fn card_question(_question: Question) -> CardQuestion {
    todo!()
}

impl Choice {
    /// The classifier reply Rob's comment choosing this option would get.
    #[cfg(test)]
    fn reply(&self, _question: u64) -> Value {
        todo!()
    }
}

/// Apply one console command from ingress. Called on the controller thread.
pub(crate) fn apply(_controller: &mut Controller, _stored: &tl3_store::Ingress) -> Result<()> {
    todo!()
}

/// Wakes tl3d when the console files a command.
pub struct Wakes {
    _settings: tl3_config::Store,
}

impl Wakes {
    pub fn new(settings: tl3_config::Store) -> Self {
        Self {
            _settings: settings,
        }
    }
}

impl crate::ingress::Source for Wakes {
    fn run(self, _sender: Sender<crate::cli::Input>) -> Result<()> {
        todo!()
    }
}

#[cfg(test)]
mod tests;
