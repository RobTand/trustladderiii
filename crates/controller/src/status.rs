use crate::{Result, board, board::plain};
use std::{collections::BTreeMap, fmt::Write};
use tl3_kernel::NodeId;
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
    pub memory: MemoryStatus,
    pub unassigned_incidents: Vec<String>,
    /// Q53: models out of quota until their announced reset.
    pub quota_models: Vec<String>,
}

#[derive(Debug, Default, PartialEq)]
pub struct MemoryStatus {
    pub pending: i64,
    pub parked: i64,
    pub http_parked: i64,
    pub rejected_status: Option<i32>,
}

impl MemoryStatus {
    fn summary(&self) -> String {
        let mut text = format!(
            "Hindsight memory. Pending: {}. Parked: {}. HTTP parked: {}.",
            self.pending, self.parked, self.http_parked
        );
        if let Some(status) = self.rejected_status {
            write!(
                text,
                " Operations incident: Hindsight rejected TL3's key (HTTP {status})."
            )
            .expect("write to string");
        }
        text
    }
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
    Ok(board::details(tx, nodes)?
        .into_iter()
        .map(|(node, details)| {
            let last_progress =
                board::last_progress(details.progress.as_ref(), details.worker_failure.as_ref());
            let open_incidents = details
                .incidents
                .into_iter()
                .map(|incident| clearable(&incident.into()))
                .collect();
            (
                node,
                RequestDetails {
                    last_progress,
                    open_incidents,
                },
            )
        })
        .collect())
}

fn details_of(card: &board::Card) -> RequestDetails {
    RequestDetails {
        last_progress: card.last_progress.clone(),
        open_incidents: card.incidents.iter().map(clearable).collect(),
    }
}

fn clearable(incident: &board::CardIncident) -> String {
    format!(
        "{} (clear: clear incident {})",
        incident.summary, incident.id
    )
}

/// The status page is a plain view of the console's board.
pub fn load(reader: &mut Reader) -> Result<Status> {
    let mut tx = reader.transaction()?;
    let board = board::read(&mut tx, chrono::Utc::now().timestamp_millis(), None)?;
    let counts = tx.query_one("SELECT count(*) FILTER (WHERE parked_reason IS NULL), count(*) FILTER (WHERE parked_reason IS NOT NULL), count(*) FILTER (WHERE parked_reason = 'http') FROM tl3_memory_outbox", &[]).map_err(tl3_store::Error::from)?;
    let health = tx
        .query_opt(
            "SELECT rejected_status FROM tl3_memory_health WHERE singleton",
            &[],
        )
        .map_err(tl3_store::Error::from)?;
    let memory = MemoryStatus {
        pending: counts.get(0),
        parked: counts.get(1),
        http_parked: counts.get(2),
        rejected_status: health.map(|row| row.get(0)),
    };
    // Q53: models out of quota until their announced reset.
    let now = chrono::Utc::now().timestamp_millis();
    let quota_models = tx
        .query(
            "SELECT row_id, value FROM tl3_world WHERE table_name = 'model_quotas' ORDER BY row_id",
            &[],
        )
        .map_err(tl3_store::Error::from)?
        .into_iter()
        .filter_map(|row| {
            let model: String = row.get(0);
            let value: serde_json::Value = row.get(1);
            let reset = value["reset_ms"].as_i64().filter(|reset| *reset > now)?;
            let reset = chrono::DateTime::from_timestamp_millis(reset)?;
            Some(format!(
                "{model} is out of quota until {}. Its steps use the next model in their ladder.",
                reset.format("%Y-%m-%d %H:%M:%S UTC")
            ))
        })
        .collect();
    tx.commit().map_err(tl3_store::Error::from)?;
    Ok(Status {
        requests: board
            .cards
            .iter()
            .map(|card| RequestStatus {
                issue: format!("{}#{}: {}", card.repo, card.issue, card.title),
                phase: card.phase.clone(),
                waiting_on: card.waiting_on.clone(),
                // Credit is zero until ingest sets it up.
                credit: card.credit.unwrap_or(0),
                details: details_of(card),
            })
            .collect(),
        memory,
        unassigned_incidents: board.unassigned_incidents.iter().map(clearable).collect(),
        quota_models,
    })
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
    if !status.unassigned_incidents.is_empty() {
        output.push_str("Open incidents without a request:\n");
        for incident in &status.unassigned_incidents {
            writeln!(output, "{}", plain(incident)).expect("write to string");
        }
    }
    for note in &status.quota_models {
        writeln!(output, "{}", plain(note)).expect("write to string");
    }
    output
}

/// The protected paths in force for each served repository (#388).
pub fn render_protected_paths(_config: &tl3_config::Config) -> String {
    String::new()
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
    output.push_str("</tbody></table>");
    if !status.unassigned_incidents.is_empty() {
        output.push_str("<h2>Open incidents without a request</h2><ul>");
        for incident in &status.unassigned_incidents {
            write!(output, "<li>{}</li>", escape_html(&plain(incident))).expect("write to string");
        }
        output.push_str("</ul>");
    }
    for note in &status.quota_models {
        write!(output, "<p>{}</p>", escape_html(&plain(note))).expect("write to string");
    }
    writeln!(
        output,
        "<p>{}</p></main></body></html>",
        escape_html(&status.memory.summary())
    )
    .expect("write to string");
    output
}

fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_shows_the_effective_protected_paths() {
        let mut config =
            tl3_config::Config::parse(include_str!("../../config/example.toml")).unwrap();
        // A file that predates a default and adds its own path.
        config.planning.protected_paths = vec!["fleet/**".into()];
        config.github.repos[0].protected_paths = vec!["CHANGELOG.md".into()];
        let text = render_protected_paths(&config);
        let repo = &config.github.repos[0];
        let line = text
            .lines()
            .find(|line| line.contains(&format!("{}/{}", repo.owner, repo.name)))
            .unwrap_or_else(|| panic!("no line for the repository: {text:?}"));
        for path in ["fleet/**", "CHANGELOG.md", ".pytest.ini", ".github/**"] {
            assert!(line.contains(path), "{path} missing from {line}");
        }
    }

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
            unassigned_incidents: vec![],
            quota_models: vec![],
            memory: MemoryStatus::default(),
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
