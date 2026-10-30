use crate::{Result, request, snapshot};
use std::fmt::Write;
use tl3_kernel::{EffectKind, NodeId, Outcome, Phase, Role};
use tl3_store::{Reader, Transaction};

const COLUMNS: [&str; 6] = [
    "Request",
    "Phase",
    "Waiting on",
    "Credit",
    "Last progress",
    "Open incidents",
];
const NONE_RECORDED: &str = "none recorded";

#[derive(Debug, PartialEq)]
pub struct Status {
    pub requests: Vec<RequestStatus>,
}

#[derive(Debug, PartialEq)]
pub struct RequestStatus {
    pub issue: String,
    pub phase: String,
    pub waiting_on: String,
    pub credit: i64,
    pub details: RequestDetails,
}

#[derive(Debug, Default, PartialEq)]
pub struct RequestDetails {
    pub last_progress: Option<String>,
    pub open_incidents: Vec<String>,
}

/// Tickets #9 and #15 fill this hook from their store tables. Keep queries in
/// this read-only transaction and return only public progress/incident summaries.
pub fn load_request_details(_tx: &mut Transaction<'_>, _node: NodeId) -> Result<RequestDetails> {
    Ok(RequestDetails::default())
}

pub fn load(reader: &mut Reader) -> Result<Status> {
    let mut tx = reader.transaction()?;
    let snapshot = tl3_store::load_snapshot(&mut tx)?;
    let mut requests = Vec::new();
    if !snapshot.rows.is_empty() {
        let world = snapshot::decode(&snapshot)?;
        for (index, node) in world.nodes.iter().enumerate() {
            let id = NodeId(index as u64);
            let request = request(&snapshot, id)?;
            let title = plain(&request.title);
            let title = if title.is_empty() {
                "title not recorded"
            } else {
                &title
            };
            let waiting_on = if node.waiting_incident {
                "incident resolution"
            } else if node.question.is_some() {
                "Rob's answer"
            } else if node.paused {
                "resume"
            } else if let Some(effect) = node.effect {
                effect_wait(effect.kind)
            } else if node.run.is_some() {
                "worker reply"
            } else if node.job.is_some() {
                "check results"
            } else {
                match node.phase {
                    Phase::Queued => "merge queue",
                    Phase::Ready(_) => "slot or files",
                    Phase::Planning { .. } => "agent pool",
                    Phase::Closed(_) => "none",
                    _ => "upstream event",
                }
            };
            requests.push(RequestStatus {
                issue: format!(
                    "{}/{}#{}: {}",
                    request.repo.owner, request.repo.name, request.issue, title
                ),
                phase: phase(node.phase),
                waiting_on: waiting_on.into(),
                credit: node.credit.balance(),
                details: load_request_details(&mut tx, id)?,
            });
        }
    }
    tx.commit().map_err(tl3_store::Error::from)?;
    Ok(Status { requests })
}

impl RequestStatus {
    fn cells(&self) -> [String; 6] {
        [
            plain(&self.issue),
            self.phase.clone(),
            self.waiting_on.clone(),
            self.credit.to_string(),
            self.details
                .last_progress
                .as_deref()
                .map(plain)
                .unwrap_or_else(|| NONE_RECORDED.into()),
            if self.details.open_incidents.is_empty() {
                NONE_RECORDED.into()
            } else {
                self.details
                    .open_incidents
                    .iter()
                    .map(|s| plain(s))
                    .collect::<Vec<_>>()
                    .join("; ")
            },
        ]
    }
}

pub fn render_text(status: &Status) -> String {
    let mut output = COLUMNS.join("\t");
    output.push('\n');
    for request in &status.requests {
        writeln!(output, "{}", request.cells().join("\t")).expect("write to string");
    }
    output
}

pub fn render_html(status: &Status) -> String {
    let mut output = String::from(
        "<!doctype html>\n<html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width, initial-scale=1\"><title>TL3 status</title><style>body{font-family:system-ui,sans-serif;margin:2rem;color:#17252a;background:#f8fafb}table{border-collapse:collapse;width:100%;background:white}th,td{text-align:left;vertical-align:top;padding:.8rem;border-bottom:1px solid #ccd5da}th{background:#e8eef1}caption{text-align:left;padding:.8rem 0}main{overflow-x:auto}</style></head><body><main><h1>TL3 status</h1><p>Reload to refresh.</p><table><caption>Requests</caption><thead><tr>",
    );
    for column in COLUMNS {
        write!(output, "<th scope=\"col\">{column}</th>").expect("write to string");
    }
    output.push_str("</tr></thead><tbody>");
    for request in &status.requests {
        output.push_str("<tr>");
        for cell in request.cells() {
            write!(output, "<td>{}</td>", escape_html(&cell)).expect("write to string");
        }
        output.push_str("</tr>");
    }
    if status.requests.is_empty() {
        output.push_str("<tr><td colspan=\"6\">No requests recorded.</td></tr>");
    }
    output.push_str("</tbody></table></main></body></html>\n");
    output
}

fn plain(value: &str) -> String {
    value
        .chars()
        .filter(|c| !c.is_control() || c.is_whitespace())
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

fn phase(phase: Phase) -> String {
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
        Phase::Closed(outcome) => {
            return format!(
                "Closed: {}",
                match outcome {
                    Outcome::Merged => "merged",
                    Outcome::Stopped => "stopped",
                    Outcome::Replaced => "replaced",
                    Outcome::Duplicate => "duplicate",
                    Outcome::AlreadyDone => "already done",
                    Outcome::NotActionable => "not actionable",
                }
            );
        }
    }
    .into()
}

fn effect_wait(kind: EffectKind) -> &'static str {
    match kind {
        EffectKind::StartRun(role) => match role {
            Role::Ingest => "request reader to start",
            Role::Audit => "goals auditor to start",
            Role::Plan => "planner to start",
            Role::Make => "maker to start",
            Role::Review => "reviewer to start",
        },
        EffectKind::SubmitChecks => "checks to be submitted",
        EffectKind::StartRebase => "branch update to start",
        EffectKind::PrepareWorktree => "worktree preparation",
        EffectKind::Push => "branch push",
        EffectKind::PostApproval => "approval to be posted",
        EffectKind::OpenPr => "pull request to open",
        EffectKind::Enqueue => "merge queue admission",
        EffectKind::StopRun => "worker to stop",
        EffectKind::WithdrawJob => "check job withdrawal",
        EffectKind::ClosePr => "pull request to close",
        EffectKind::RemoveWorktree => "worktree removal",
    }
}
