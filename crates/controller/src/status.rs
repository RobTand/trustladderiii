use crate::{Result, request, snapshot};
use std::{collections::BTreeMap, fmt::Write};
use tl3_kernel::{EffectKind, NodeId, Outcome, Phase, Role, Wait};
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

/// Read public progress summaries in the status reader's snapshot.
/// Active runs take precedence; retirement preserves progress while a request waits.
pub fn load_request_details(
    tx: &mut Transaction<'_>,
    nodes: &[NodeId],
) -> Result<BTreeMap<NodeId, RequestDetails>> {
    let ids: Vec<_> = nodes.iter().map(|node| node.0.to_string()).collect();
    let rows = tx.query("SELECT n.node_id, p.unit, p.last_progress_ms FROM unnest($1::text[]) WITH ORDINALITY AS n(node_id, position) LEFT JOIN LATERAL (SELECT unit, last_progress_ms FROM tl3_worker_progress WHERE node_id = n.node_id ORDER BY started_ms DESC, run_key LIMIT 1) active ON true LEFT JOIN tl3_worker_summary summary ON summary.node_id = n.node_id CROSS JOIN LATERAL (SELECT CASE WHEN active.unit IS NOT NULL THEN active.unit ELSE summary.unit END AS unit, CASE WHEN active.unit IS NOT NULL THEN active.last_progress_ms ELSE summary.last_progress_ms END AS last_progress_ms) p ORDER BY n.position", &[&ids]).map_err(tl3_store::Error::from)?;
    let mut details = BTreeMap::new();
    for (node, row) in nodes.iter().zip(rows) {
        let time: Option<i64> = row.get(2);
        let unit: Option<String> = row.get(1);
        let last_progress = time.and_then(chrono::DateTime::from_timestamp_millis).map(|time| {
            let action = match unit.as_deref() {
                Some("model_turn") => "Model turn completed",
                _ => "Tool result received",
            };
            format!("{action} at {}", time.format("%Y-%m-%d %H:%M:%S UTC"))
        });
        details.insert(*node, RequestDetails { last_progress, ..RequestDetails::default() });
    }
    Ok(details)
}

pub fn load(reader: &mut Reader) -> Result<Status> {
    let mut tx = reader.transaction()?;
    let snapshot = tl3_store::load_snapshot(&mut tx)?;
    let mut requests = Vec::new();
    if !snapshot.rows.is_empty() {
        let world = snapshot::decode(&snapshot)?;
        let ids: Vec<_> = (0..world.nodes.len())
            .map(|index| NodeId(index as u64))
            .collect();
        let mut details = load_request_details(&mut tx, &ids)?;
        for (index, node) in world.nodes.iter().enumerate() {
            let id = NodeId(index as u64);
            let request = request(&snapshot, id)?;
            let title = plain(&request.title);
            let title = if title.is_empty() {
                "title not recorded"
            } else {
                &title
            };
            let waiting_on = match node.waiting_on() {
                Wait::Incident => "incident resolution",
                Wait::Answer => "Rob's answer",
                Wait::Resume => "resume",
                Wait::Effect(kind) => effect_wait(kind),
                Wait::WorkerReply => "the current task to finish",
                Wait::CheckResults => "check results",
                Wait::MergeQueue => "merge queue",
                Wait::SlotOrFiles => "another change to finish using these files",
                Wait::AgentPool => "a planner to become available",
                Wait::None => "none",
                Wait::UpstreamEvent => match node.phase {
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
            };
            requests.push(RequestStatus {
                issue: format!(
                    "{}/{}#{}: {}",
                    request.repo.owner, request.repo.name, request.issue, title
                ),
                phase: phase(node.phase),
                waiting_on: waiting_on.into(),
                credit: node.credit.balance(),
                details: details.remove(&id).unwrap_or_default(),
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
            plain(&self.phase),
            plain(&self.waiting_on),
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
        .filter(|c| (!c.is_control() || c.is_whitespace())
            && !matches!(*c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}' | '\u{200b}'..='\u{200f}' | '\u{feff}'))
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

#[cfg(test)]
mod tests {
    use super::*;

    fn unescape(value: &str) -> String {
        value
            .replace("&lt;", "<")
            .replace("&gt;", ">")
            .replace("&quot;", "\"")
            .replace("&#39;", "'")
            .replace("&amp;", "&")
    }

    #[test]
    fn hostile_details_and_every_text_cell_match_unescaped_html() {
        let hidden: String = (0x202a..=0x202e)
            .chain(0x2066..=0x2069)
            .chain(0x200b..=0x200f)
            .chain([0xfeff])
            .map(|n| char::from_u32(n).unwrap())
            .collect();
        let hostile = format!(
            "<script title=\"'&\">&lt;مرحبا</script>\0\x1b{hidden}\t\r\n{}",
            "long".repeat(32_768)
        );
        let cleaned = format!(
            "<script title=\"'&\">&lt;مرحبا</script> {}",
            "long".repeat(32_768)
        );
        let model = Status {
            requests: vec![
                RequestStatus {
                    issue: hostile.clone(),
                    phase: hostile.clone(),
                    waiting_on: hostile.clone(),
                    credit: -4,
                    details: RequestDetails {
                        last_progress: Some(hostile.clone()),
                        open_incidents: vec![
                            hostile.clone(),
                            "second <incident> \"'&\"\u{202e}\0".into(),
                        ],
                    },
                },
                RequestStatus {
                    issue: "another request".into(),
                    phase: "Ready".into(),
                    waiting_on: "a writer".into(),
                    credit: 5,
                    details: RequestDetails::default(),
                },
            ],
        };
        let text = render_text(&model);
        let html = render_html(&model);
        assert!(!html.contains("<script title"));
        assert!(
            html.contains(
                "&lt;script title=&quot;&#39;&amp;&quot;&gt;&amp;lt;مرحبا&lt;/script&gt;"
            )
        );
        assert!(html.contains("; second &lt;incident&gt; &quot;&#39;&amp;&quot;"));
        assert!(
            !text
                .chars()
                .any(|c| hidden.contains(c) || c == '\0' || c == '\x1b')
        );
        let text_rows: Vec<Vec<_>> = text
            .lines()
            .skip(1)
            .map(|line| line.split('\t').collect())
            .collect();
        assert_eq!(
            text_rows[0],
            vec![
                cleaned.as_str(),
                cleaned.as_str(),
                cleaned.as_str(),
                "-4",
                cleaned.as_str(),
                &format!("{cleaned}; second <incident> \"'&\"")
            ]
        );
        let html_cells: Vec<_> = html
            .split("<td>")
            .skip(1)
            .map(|cell| unescape(cell.split_once("</td>").unwrap().0))
            .collect();
        let text_cells: Vec<_> = text_rows.into_iter().flatten().collect();
        assert_eq!(html_cells.len(), 12);
        assert_eq!(text_cells, html_cells);
    }
}
