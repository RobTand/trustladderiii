//! The board projection (PRD #327). This is the one place that maps stored
//! controller state to the console's columns, cards, counts and badges. The
//! status page renders from it too, so the two can't disagree.

use crate::{Result, executor::Output, looks::Incident, notices::FailedNotice};
use serde::Serialize;
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::Write,
};
use tl3_kernel::{EffectKind, Node, NodeId, Notice, Outcome, Phase, Priority, Role, Wait};
use tl3_store::Transaction;
use ts_rs::TS;

const HOUR_MS: i64 = 3_600_000;
const DAY_MS: i64 = 24 * HOUR_MS;
const NONE_RECORDED: &str = "none recorded";

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
        match phase {
            Phase::Ingesting | Phase::Auditing => Column::Intake,
            Phase::Unplanned | Phase::Planning { .. } => Column::Planning,
            Phase::Ready(resume) => Column::of(resume.phase()),
            Phase::Making => Column::Making,
            Phase::Checking => Column::Checking,
            Phase::Reviewing => Column::Reviewing,
            Phase::Publishing | Phase::Queued | Phase::Rebasing => Column::Merging,
            Phase::AwaitingRob => Column::NeedsRob,
            Phase::Stopping(_) | Phase::Closed(_) => Column::Done,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Column::Backlog => "Backlog",
            Column::Intake => "Intake",
            Column::Planning => "Planning",
            Column::Making => "Making",
            Column::Checking => "Checking",
            Column::Reviewing => "Reviewing",
            Column::Merging => "Merging",
            Column::NeedsRob => "Needs Rob",
            Column::Done => "Done",
        }
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

impl Ending {
    fn of(outcome: Outcome) -> Ending {
        match outcome {
            Outcome::Merged => Ending::Merged,
            Outcome::Stopped => Ending::Stopped,
            Outcome::Replaced => Ending::Replaced,
            Outcome::Duplicate => Ending::Duplicate,
            Outcome::AlreadyDone => Ending::AlreadyDone,
            Outcome::NotActionable => Ending::NotActionable,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Ending::Merged => "merged",
            Ending::Stopped => "stopped",
            Ending::Replaced => "replaced",
            Ending::Duplicate => "duplicate",
            Ending::AlreadyDone => "already done",
            Ending::NotActionable => "not actionable",
        }
    }
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
    /// Q39: `P0`–`P3`, or `unranked`.
    pub priority: String,
    /// Q39: 1-based place in the agent-pool line, while the request waits for a slot.
    pub pool_position: Option<u32>,
    pub role: Option<String>,
    pub model: Option<String>,
    pub attempt: u8,
    pub round: u8,
    pub retries: Retries,
    /// None until ingest sets up the request's credit.
    pub credit: Option<i64>,
    /// When the request entered its current phase, in Unix milliseconds.
    pub phase_since_ms: Option<i64>,
    pub last_progress: Option<String>,
    pub incidents: Vec<CardIncident>,
    pub badges: Vec<Badge>,
    /// The open question, with the options Rob can answer from the console.
    pub question: Option<crate::console::CardQuestion>,
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
    /// Merged requests, by when they entered cleanup or closed.
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
        let before: BTreeMap<_, _> = earlier.cards.iter().map(|card| (card.id, card)).collect();
        let now: BTreeSet<_> = self.cards.iter().map(|card| card.id).collect();
        BoardPatch {
            revision: self.revision,
            built_at_ms: self.built_at_ms,
            flow: self.flow.clone(),
            upserts: self
                .cards
                .iter()
                .filter(|card| before.get(&card.id) != Some(card))
                .cloned()
                .collect(),
            removals: before.into_keys().filter(|id| !now.contains(id)).collect(),
            unassigned_incidents: self.unassigned_incidents.clone(),
        }
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
    /// Q53: models that reported quota, with their announced resets in Unix ms.
    pub quota_resets: BTreeMap<String, u64>,
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
    /// The model the maker ran for this attempt, which its reviewer must differ from.
    pub maker_model: Option<String>,
    /// The open question's stored context, with the deciding worker's findings.
    pub question_context: Option<serde_json::Value>,
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

/// Closed requests stay on the board for a day; the counts include them all.
pub fn project(input: Input<'_>) -> Board {
    let mut cards = Vec::with_capacity(input.requests.len());
    let mut expired = BTreeSet::new();
    // Q39: the kernel grants the pool to the lowest key first.
    let keys: Vec<_> = input
        .requests
        .iter()
        .filter_map(|request| tl3_kernel::pool_key(&request.node))
        .collect();
    for request in input.requests {
        let pool_position = tl3_kernel::pool_key(&request.node)
            .map(|key| 1 + keys.iter().filter(|theirs| **theirs < key).count() as u32);
        if matches!(request.node.phase, Phase::Closed(_))
            && request
                .phase_since_ms
                .is_none_or(|since| input.now_ms - since > DAY_MS)
        {
            expired.insert(request.id.0);
        }
        cards.push(card(
            request,
            pool_position,
            input.models,
            &input.quota_resets,
            input.now_ms,
        ));
    }
    cards.sort_by_key(|card| card.id);
    let merged_within = |window: i64| {
        cards
            .iter()
            .filter(|card| {
                card.ending == Some(Ending::Merged)
                    && card
                        .phase_since_ms
                        .is_some_and(|since| input.now_ms - since <= window)
            })
            .count() as u32
    };
    let flow = Flow {
        columns: Column::ALL
            .into_iter()
            .map(|column| ColumnCount {
                column,
                label: column.label().into(),
                count: cards.iter().filter(|card| card.column == column).count() as u32,
            })
            .collect(),
        in_flight: cards
            .iter()
            .filter(|card| !matches!(card.column, Column::Done | Column::Backlog))
            .count() as u32,
        merged_last_hour: merged_within(HOUR_MS),
        merged_last_day: merged_within(DAY_MS),
    };
    cards.retain(|card| !expired.contains(&card.id));
    Board {
        revision: input.revision,
        built_at_ms: input.now_ms,
        flow,
        cards,
        unassigned_incidents: input
            .unassigned_incidents
            .into_iter()
            .map(CardIncident::from)
            .collect(),
    }
}

impl From<StoredIncident> for CardIncident {
    fn from(incident: StoredIncident) -> Self {
        CardIncident {
            id: incident.id,
            summary: plain(&incident.summary),
        }
    }
}

fn card(
    request: Request,
    pool_position: Option<u32>,
    models: Option<&tl3_config::Models>,
    quota_resets: &BTreeMap<String, u64>,
    now_ms: i64,
) -> Card {
    let node = &request.node;
    let title = plain(&request.title);
    let ending = match node.phase {
        Phase::Stopping(outcome) | Phase::Closed(outcome) => Some(Ending::of(outcome)),
        _ => None,
    };
    let wait = node.waiting_on();
    let priority = match node.priority {
        Priority::P0 => "P0",
        Priority::P1 => "P1",
        Priority::P2 => "P2",
        Priority::P3 => "P3",
        Priority::Unranked => "unranked",
    };
    let role = role(node.phase);
    let model = role
        .zip(models)
        .and_then(|((role, _), models)| {
            let rung = if role == "planner" {
                node.plan_retries
            } else {
                node.attempt.saturating_sub(1)
            };
            // The worker adapter chooses with the same two functions.
            let ladder = crate::adapters::workers::ladder::candidates(
                models,
                role,
                usize::from(rung),
                node.review_restarted,
                request.maker_model.as_deref(),
            )
            .ok()?;
            Some(crate::adapters::workers::ladder::running(
                &ladder,
                quota_resets,
                u64::try_from(now_ms).unwrap_or(0),
            ))
        })
        .map(|model| model.name.clone());
    let mut badges = BTreeSet::new();
    if node.paused {
        badges.insert(Badge::Paused);
    }
    if node.waiting_incident || !request.incidents.is_empty() {
        badges.insert(Badge::Incident);
    }
    if request
        .incidents
        .iter()
        .any(|incident| incident.maker_blocked)
    {
        badges.insert(Badge::MakerBlocked);
    }
    if node.question.is_some() || node.incident_needs_rob || node.phase == Phase::AwaitingRob {
        badges.insert(Badge::NeedsRob);
    }
    if matches!(wait, Wait::AgentPool | Wait::SlotOrFiles) {
        badges.insert(Badge::WaitingForCapacity);
    }
    let last_progress = last_progress(request.progress.as_ref(), request.worker_failure.as_ref());
    Card {
        id: request.id.0,
        repo: plain(&request.repo),
        issue: request.issue,
        title: if title.is_empty() {
            "title not recorded".into()
        } else {
            title
        },
        column: Column::of(node.phase),
        phase: phase_label(node.phase, ending),
        ending,
        priority: priority.into(),
        pool_position,
        waiting_on: if request.question_post_failed {
            "the question post that failed; the question was never posted".into()
        } else if node.effect.is_some_and(|effect| effect.cancelling) {
            // The node moved on; it waits only for the old delivery to settle.
            "a cancelled step to settle".into()
        } else if let Some(position) = pool_position.filter(|_| wait == Wait::AgentPool) {
            format!(
                "{} ({priority}, {} in line)",
                waiting_on(wait, node.phase),
                ordinal(position)
            )
        } else {
            waiting_on(wait, node.phase).into()
        },
        role: role.map(|(_, label)| label.into()),
        model,
        attempt: node.attempt,
        round: node.round,
        retries: Retries {
            plan: node.plan_retries,
            audit: node.audit_rounds,
            run_failures: node.run_failures,
            effect_failures: node.fx_failures,
        },
        credit: node.credit.ready.then(|| node.credit.balance()),
        phase_since_ms: request.phase_since_ms,
        last_progress,
        incidents: request
            .incidents
            .into_iter()
            .map(CardIncident::from)
            .collect(),
        badges: badges.into_iter().collect(),
        question: node.question.map(|question| {
            crate::console::card_question(question, request.question_context.as_ref())
        }),
    }
}

/// The latest worker progress and the current generation's worker failure, as text.
pub(crate) fn last_progress(
    progress: Option<&Progress>,
    failure: Option<&WorkerFailure>,
) -> Option<String> {
    if progress.is_none() && failure.is_none() {
        return None;
    }
    let mut text = progress
        .and_then(|progress| {
            let time = chrono::DateTime::from_timestamp_millis(progress.at_ms)?;
            let action = if progress.model_turn {
                "Model turn completed"
            } else {
                "Tool result received"
            };
            Some(format!(
                "{action} at {}",
                time.format("%Y-%m-%d %H:%M:%S UTC")
            ))
        })
        .unwrap_or_else(|| NONE_RECORDED.into());
    if let Some(failure) = failure {
        write!(
            text,
            "; last worker failure: {}: {}",
            failure.class, failure.message
        )
        .expect("write to string");
    }
    Some(plain(&text))
}

/// The worker role that owns a phase, by its config name and its label.
fn role(phase: Phase) -> Option<(&'static str, &'static str)> {
    match phase {
        Phase::Ingesting => Some(("ingest", "Reader")),
        Phase::Auditing => Some(("audit", "Auditor")),
        Phase::Planning { .. } => Some(("planner", "Planner")),
        Phase::Making => Some(("maker", "Maker")),
        Phase::Reviewing => Some(("reviewer", "Reviewer")),
        _ => None,
    }
}

fn phase_label(phase: Phase, ending: Option<Ending>) -> String {
    match phase {
        Phase::Ingesting => "Understanding the request",
        Phase::Auditing => "Auditing the goals",
        Phase::Unplanned => "Waiting for a plan",
        Phase::Planning { .. } => "Planning",
        Phase::Ready(_) => "Ready",
        Phase::Making => "Making the change",
        Phase::Checking => "Running checks",
        Phase::Reviewing => "Reviewing",
        Phase::Publishing => "Publishing",
        Phase::Queued => "In the merge queue",
        Phase::Rebasing => "Updating the branch",
        Phase::AwaitingRob => "Awaiting Rob",
        Phase::Stopping(_) => "Cleaning up",
        Phase::Closed(_) => {
            return format!("Closed: {}", ending.map_or("", Ending::label));
        }
    }
    .into()
}

fn waiting_on(wait: Wait, phase: Phase) -> &'static str {
    match wait {
        Wait::Incident => "incident resolution",
        Wait::Answer => "Rob's answer",
        Wait::Resume => "resume",
        Wait::Effect(kind) => effect_wait(kind),
        Wait::WorkerReply => "the current task to finish",
        Wait::CheckResults => "check results",
        Wait::MergeQueue => "merge queue",
        Wait::SlotOrFiles => "another change to finish using these files",
        Wait::AgentPool => "an agent slot",
        Wait::None => "none",
        Wait::UpstreamEvent => match phase {
            Phase::Ingesting => "the request to be read",
            Phase::Auditing => "the goals to be checked",
            Phase::Unplanned => "planning to begin",
            Phase::Planning { .. } => "a planner to become available",
            Phase::Ready(_) => "another change to finish using these files",
            Phase::Making => "the change to be written",
            Phase::Checking => "checks to finish",
            Phase::Reviewing => "the review to finish",
            Phase::Publishing => "the pull request to be published",
            Phase::Queued => "merge queue",
            Phase::Rebasing => "the branch update to finish",
            Phase::AwaitingRob => "Rob's answer",
            Phase::Stopping(_) => "cleanup to finish",
            Phase::Closed(_) => "none",
        },
    }
}

/// `1st`, `2nd`, `3rd`, `4th`, ..., `11th`, ..., `21st`.
fn ordinal(n: u32) -> String {
    let suffix = match (n % 10, n % 100) {
        (_, 11..=13) => "th",
        (1, _) => "st",
        (2, _) => "nd",
        (3, _) => "rd",
        _ => "th",
    };
    format!("{n}{suffix}")
}

fn effect_wait(kind: EffectKind) -> &'static str {
    match kind {
        EffectKind::StartRun(role) => match role {
            Role::Ingest => "request reader to start",
            Role::Audit => "goals auditor to start",
            Role::Plan => "planner to start",
            Role::Make => "writer to start",
            Role::Review => "reviewer to start",
        },
        EffectKind::SubmitChecks => "checks to be submitted",
        EffectKind::StartRebase => "branch update to start",
        EffectKind::PrepareWorktree => "preparing the change files",
        EffectKind::Push => "branch push",
        EffectKind::PostApproval => "approval to be posted",
        EffectKind::OpenPr => "pull request to open",
        EffectKind::Enqueue => "merge queue admission",
        EffectKind::StopRun => "the current task to stop",
        EffectKind::WithdrawJob => "checks to stop",
        EffectKind::ClosePr => "pull request to close",
        EffectKind::RemoveWorktree => "removing temporary change files",
    }
}

/// Stored text as one plain line: no control or direction-override characters.
pub(crate) fn plain(value: &str) -> String {
    value
        .chars()
        .filter(|c| (!c.is_control() || c.is_whitespace())
            && !matches!(*c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}' | '\u{200b}'..='\u{200f}' | '\u{feff}'))
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// What the side tables say about each node: worker progress, the current
/// generation's worker failure, and open incidents.
pub(crate) struct Details {
    pub progress: Option<Progress>,
    pub worker_failure: Option<WorkerFailure>,
    pub incidents: Vec<StoredIncident>,
}

/// Each node's latest worker progress. An active run's takes precedence;
/// retirement keeps the last progress while a request waits.
fn progress(tx: &mut Transaction<'_>, nodes: &[NodeId]) -> Result<BTreeMap<NodeId, Progress>> {
    let ids: Vec<_> = nodes.iter().map(|node| node.0.to_string()).collect();
    let rows = tx
        .query(
            "SELECT active.unit, CASE WHEN active.unit IS NOT NULL THEN active.last_progress_ms
                   ELSE summary.last_progress_ms END, summary.unit
         FROM unnest($1::text[]) WITH ORDINALITY AS n(node_id, position)
         LEFT JOIN LATERAL (
             SELECT unit, last_progress_ms FROM tl3_worker_progress
             WHERE node_id = n.node_id ORDER BY started_ms DESC, run_key LIMIT 1
         ) active ON true
         LEFT JOIN tl3_worker_summary summary ON summary.node_id = n.node_id
         ORDER BY n.position",
            &[&ids],
        )
        .map_err(tl3_store::Error::from)?;
    Ok(nodes
        .iter()
        .zip(rows)
        .filter_map(|(node, row)| {
            let at_ms: Option<i64> = row.get(1);
            let unit: Option<String> = row.get::<_, Option<String>>(0).or_else(|| row.get(2));
            Some((
                *node,
                Progress {
                    at_ms: at_ms?,
                    model_turn: unit.as_deref() == Some("model_turn"),
                },
            ))
        })
        .collect())
}

/// Read side-table details inside the caller's snapshot. An active run's
/// progress takes precedence; retirement keeps progress while a request waits.
pub(crate) fn details(
    tx: &mut Transaction<'_>,
    nodes: &[NodeId],
) -> Result<BTreeMap<NodeId, Details>> {
    let ids: Vec<_> = nodes.iter().map(|node| node.0.to_string()).collect();
    let mut progress = progress(tx, nodes)?;
    let mut details: BTreeMap<_, _> = nodes
        .iter()
        .map(|node| {
            let details = Details {
                progress: progress.remove(node),
                worker_failure: None,
                incidents: Vec::new(),
            };
            (*node, details)
        })
        .collect();
    for row in tx.query("SELECT failure.row_id, failure.value, node.value->'generation'
        FROM tl3_world failure JOIN tl3_world node ON node.table_name = 'nodes' AND node.row_id = failure.row_id
        WHERE failure.table_name = 'worker_failures' AND failure.row_id = ANY($1)", &[&ids]).map_err(tl3_store::Error::from)? {
        let id: String = row.get(0);
        let failure: serde_json::Value = row.get(1);
        let generation: serde_json::Value = row.get(2);
        // RunFailed advances the generation before status reads the outcome.
        if failure["generation"].as_u64().and_then(|n| n.checked_add(1)) != generation.as_u64() {
            continue;
        }
        if let Some(details) = id.parse().ok().and_then(|id| details.get_mut(&NodeId(id))) {
            details.worker_failure = Some(WorkerFailure {
                class: failure["failure_class"].as_str().unwrap_or("unknown").into(),
                message: failure["failure_message"].as_str().unwrap_or("worker run failed").into(),
            });
        }
    }
    for row in tx.query("SELECT row_id, value FROM tl3_world WHERE table_name = 'incidents' AND value->>'node' = ANY($1) AND value->>'open' = 'true' ORDER BY row_id", &[&ids]).map_err(tl3_store::Error::from)? {
        let id: String = row.get(0);
        let incident: Incident = crate::decode_stored(row.get(1))?;
        if let Some(details) = incident.node.and_then(|node| details.get_mut(&node)) {
            details.incidents.push(StoredIncident {
                id,
                maker_blocked: incident.signature.class == crate::blocked::CLASS,
                summary: incident.summary,
            });
        }
    }
    Ok(details)
}

/// Stored rows for one board, read in one snapshot. Keep them to refresh
/// worker progress without rereading the world.
#[derive(Clone, Debug)]
pub struct Rows {
    pub revision: i64,
    pub requests: Vec<Request>,
    pub unassigned_incidents: Vec<StoredIncident>,
    pub quota_resets: BTreeMap<String, u64>,
}

impl Rows {
    /// Read every request's newest node and side tables inside the caller's
    /// read transaction.
    pub fn read(tx: &mut Transaction<'_>) -> Result<Rows> {
        let snapshot = tl3_store::load_snapshot(tx)?;
        let mut rows = Rows {
            revision: snapshot.revision.get(),
            requests: Vec::new(),
            unassigned_incidents: Vec::new(),
            quota_resets: BTreeMap::new(),
        };
        if snapshot.rows.is_empty() {
            return Ok(rows);
        }
        let world = crate::snapshot::decode(&snapshot)?;
        let ids: Vec<_> = (0..world.nodes.len())
            .map(|index| NodeId(index as u64))
            .collect();
        let mut details = details(tx, &ids)?;
        let since: BTreeMap<String, i64> = tx
            .query(
                "SELECT node_id, (extract(epoch FROM since) * 1000)::bigint FROM tl3_node_phases",
                &[],
            )
            .map_err(tl3_store::Error::from)?
            .into_iter()
            .map(|row| (row.get(0), row.get(1)))
            .collect();
        let mut maker_sessions = BTreeMap::new();
        let mut failed_notices = Vec::new();
        let mut bindings = BTreeMap::new();
        let mut incidents = Vec::new();
        let mut question_contexts = BTreeMap::new();
        for row in &snapshot.rows {
            match row.table.as_str() {
                "question_context" => {
                    question_contexts.insert(row.id.as_str(), &row.value);
                }
                "notice_failures" => {
                    failed_notices.push(crate::decode_stored::<FailedNotice>(row.value.clone())?)
                }
                "incidents" => incidents.push(crate::decode_stored::<Incident>(row.value.clone())?),
                "maker_session" => {
                    maker_sessions.insert(row.id.as_str(), &row.value);
                }
                // A binding that doesn't decode never counts as an issue's newest request.
                "requests" => {
                    if let Ok(id) = row.id.parse::<u64>() {
                        bindings.insert(
                            id,
                            crate::decode_stored::<crate::Request>(row.value.clone()).ok(),
                        );
                    }
                }
                _ => {}
            }
        }
        // Repository identity ignores ASCII case, as `Repository`'s equality does.
        let issue = |request: &crate::Request| {
            (
                request.repo.installation,
                request.repo.owner.to_ascii_lowercase(),
                request.repo.name.to_ascii_lowercase(),
                request.issue,
            )
        };
        let mut newest = BTreeMap::new();
        for (id, request) in &bindings {
            if let Some(request) = request {
                newest.insert(issue(request), *id);
            }
        }
        for (index, node) in world.nodes.iter().enumerate() {
            let id = NodeId(index as u64);
            let request = bindings
                .get_mut(&id.0)
                .and_then(Option::take)
                .ok_or(crate::Error::Invalid("missing request binding"))?;
            // A newer request for the same issue replaces this one on the board.
            if newest.get(&issue(&request)) != Some(&id.0) {
                continue;
            }
            let question_post_failed = failed_notices.iter().any(|failed| {
                matches!(failed.delivery.output,
                    Output::Notice { node: n, notice: Notice::Ask { question } }
                    if n == id && node.question.is_some_and(|q| q.id == question))
            });
            let details = details.remove(&id);
            rows.requests.push(Request {
                id,
                node: *node,
                repo: format!("{}/{}", request.repo.owner, request.repo.name),
                issue: request.issue,
                title: request.title,
                question_post_failed,
                phase_since_ms: since.get(&index.to_string()).copied(),
                progress: details.as_ref().and_then(|d| d.progress.clone()),
                worker_failure: details.as_ref().and_then(|d| d.worker_failure.clone()),
                incidents: details.map(|d| d.incidents).unwrap_or_default(),
                // The worker adapter reads the same session for this attempt.
                maker_model: maker_sessions
                    .get(index.to_string().as_str())
                    .filter(|session| {
                        session["attempt"] == node.attempt && session["replanned"] == node.replanned
                    })
                    .and_then(|session| session["model"].as_str())
                    .map(str::to_owned),
                question_context: node
                    .question
                    .and_then(|question| question_contexts.get(question.id.to_string().as_str()))
                    .map(|context| (*context).clone()),
            });
        }
        rows.quota_resets = crate::adapters::workers::ladder::quota_resets(&snapshot);
        rows.unassigned_incidents = incidents
            .into_iter()
            .filter(|incident| incident.open && incident.node.is_none())
            .map(|incident| StoredIncident {
                maker_blocked: incident.signature.class == crate::blocked::CLASS,
                id: incident.id,
                summary: incident.summary,
            })
            .collect();
        Ok(rows)
    }

    /// Reread only worker progress. Controller commits need a full `read`.
    pub fn refresh_progress(&mut self, tx: &mut Transaction<'_>) -> Result<()> {
        let ids: Vec<_> = self.requests.iter().map(|request| request.id).collect();
        let mut progress = progress(tx, &ids)?;
        for request in &mut self.requests {
            request.progress = progress.remove(&request.id);
        }
        Ok(())
    }

    pub fn project(&self, now_ms: i64, models: Option<&tl3_config::Models>) -> Board {
        project(Input {
            revision: self.revision,
            now_ms,
            requests: self.requests.clone(),
            unassigned_incidents: self.unassigned_incidents.clone(),
            models,
            quota_resets: self.quota_resets.clone(),
        })
    }
}

/// Read and project the board inside the caller's read transaction.
pub fn read(
    tx: &mut Transaction<'_>,
    now_ms: i64,
    models: Option<&tl3_config::Models>,
) -> Result<Board> {
    Ok(Rows::read(tx)?.project(now_ms, models))
}

#[cfg(test)]
mod tests;
