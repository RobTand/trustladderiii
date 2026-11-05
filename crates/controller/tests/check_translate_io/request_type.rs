#[derive(Clone, Debug, Serialize, Deserialize)]
struct Request {
    repo: Repository,
    issue: u64,
    #[serde(default)]
    title: String,
    issue_id: i64,
    #[serde(default)]
    source: Option<String>,
    #[serde(default)]
    proposed: Option<adoption::ProposedPullRequest>,
    /// P0–P3 from the issue, read once at intake (#416); `None` for unranked
    /// and older rows, and omitted then so the row keeps its older shape (#375).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    priority: Option<u8>,
}
