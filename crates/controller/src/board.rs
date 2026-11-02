//! The board projection (PRD #327). This is the one place that maps stored
//! controller state to the console's columns, cards, counts and badges. The
//! status page renders from it too, so the two can't disagree.

use serde::Serialize;
use tl3_kernel::{Node, NodeId, Phase};
use tl3_store::Transaction;
use ts_rs::TS;

/// A workflow stage on the board, in board order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, TS)]
pub enum Column {
    Backlog,
    Intake,
    Planning,
    Making,
    Checking,
    Reviewing,
    Merging,
    NeedsRob,
    Done,
}

impl Column {
    pub const ALL: [Column; 9] = [
        Column::Backlog,
        Column::Intake,
        Column::Planning,
        Column::Making,
        Column::Checking,
        Column::Reviewing,
        Column::Merging,
        Column::NeedsRob,
        Column::Done,
    ];

    /// The PRD's phase-to-column mapping.
    pub fn of(phase: Phase) -> Column {
        let _ = phase;
        todo!()
    }

    pub fn label(self) -> &'static str {
        todo!()
    }
}

/// How a request ended, or is ending while it cleans up.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, TS)]
pub enum Ending {
    Merged,
    Stopped,
    Replaced,
    Duplicate,
    AlreadyDone,
    NotActionable,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, TS)]
pub enum Badge {
    Paused,
    Incident,
    MakerBlocked,
    NeedsRob,
    WaitingForCapacity,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, TS)]
pub struct CardIncident {
    pub id: String,
    pub summary: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, TS)]
pub struct Retries {
    pub plan: u8,
    pub audit: u8,
    pub run_failures: u8,
    pub effect_failures: u8,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, TS)]
pub struct Card {
    /// The request's node id.
    pub id: u64,
    /// `owner/name`.
    pub repo: String,
    pub issue: u64,
    pub title: String,
    pub column: Column,
    pub phase: String,
    pub ending: Option<Ending>,
    pub waiting_on: String,
    pub role: Option<String>,
    pub model: Option<String>,
    pub attempt: u8,
    pub round: u8,
    pub retries: Retries,
    pub credit: i64,
    /// When the request entered its current phase, in Unix milliseconds.
    pub phase_since_ms: Option<i64>,
    pub last_progress: Option<String>,
    pub incidents: Vec<CardIncident>,
    pub badges: Vec<Badge>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, TS)]
pub struct ColumnCount {
    pub column: Column,
    pub label: String,
    pub count: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, TS)]
pub struct Flow {
    /// One entry per column, in board order.
    pub columns: Vec<ColumnCount>,
    /// Requests that haven't reached Done.
    pub in_flight: u32,
    pub merged_last_hour: u32,
    pub merged_last_day: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, TS)]
pub struct Board {
    /// The controller revision this board shows.
    pub revision: i64,
    pub built_at_ms: i64,
    pub flow: Flow,
    /// Ordered by id.
    pub cards: Vec<Card>,
    pub unassigned_incidents: Vec<CardIncident>,
}

/// What changed between two boards: replace or add `upserts` by id, drop
/// `removals`, and take the rest as given.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, TS)]
pub struct BoardPatch {
    pub revision: i64,
    pub built_at_ms: i64,
    pub flow: Flow,
    pub upserts: Vec<Card>,
    pub removals: Vec<u64>,
    pub unassigned_incidents: Vec<CardIncident>,
}

impl Board {
    pub fn patch_since(&self, earlier: &Board) -> BoardPatch {
        let _ = earlier;
        todo!()
    }
}

/// Stored rows for one board, already read in one snapshot.
pub struct Input<'a> {
    pub revision: i64,
    pub now_ms: i64,
    pub requests: Vec<Request>,
    pub unassigned_incidents: Vec<StoredIncident>,
    /// Configured model ladders, to name each card's model.
    pub models: Option<&'a tl3_config::Models>,
}

/// A request's newest node and what the controller's side tables say about it.
#[derive(Clone, Debug)]
pub struct Request {
    pub id: NodeId,
    pub node: Node,
    pub repo: String,
    pub issue: u64,
    pub title: String,
    /// The question was asked, but posting it failed.
    pub question_post_failed: bool,
    pub phase_since_ms: Option<i64>,
    pub progress: Option<Progress>,
    pub worker_failure: Option<WorkerFailure>,
    /// Open incidents.
    pub incidents: Vec<StoredIncident>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Progress {
    pub at_ms: i64,
    pub model_turn: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkerFailure {
    pub class: String,
    pub message: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredIncident {
    pub id: String,
    pub summary: String,
    pub maker_blocked: bool,
}

pub fn project(input: Input<'_>) -> Board {
    let _ = input;
    todo!()
}

/// Read and project the board inside the caller's read transaction.
pub fn read(
    tx: &mut Transaction<'_>,
    now_ms: i64,
    models: Option<&tl3_config::Models>,
) -> crate::Result<Board> {
    let _ = (tx, now_ms, models);
    todo!()
}

#[cfg(test)]
mod tests;
