use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct Actor {
    pub id: u64,
    pub login: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct Repository {
    pub id: u64,
    pub name: String,
    pub owner: Actor,
}

impl Repository {
    pub fn full_name(&self) -> String {
        format!("{}/{}", self.owner.login, self.name)
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct Label {
    pub name: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct Issue {
    pub number: u64,
    pub title: String,
    pub body: Option<String>,
    pub state: String,
    pub updated_at: DateTime<Utc>,
    #[serde(default)]
    pub labels: Vec<Label>,
    pub pull_request: Option<PullLink>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct PullLink {
    pub url: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct Comment {
    pub id: u64,
    pub body: String,
    pub user: Actor,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub issue_url: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct PullRequest {
    pub number: u64,
    pub user: Actor,
    pub state: String,
    pub merged_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub merge_commit_sha: Option<String>,
    pub head: PullHead,
    pub base: PullHead,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct PullHead {
    #[serde(rename = "ref")]
    pub branch: String,
    pub sha: String,
    pub repo: Option<Repository>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct CheckRun {
    pub id: u64,
    #[serde(default)]
    pub pull_requests: Vec<CheckPullRequest>,
    pub name: String,
    pub head_sha: String,
    pub external_id: Option<String>,
    pub status: String,
    pub conclusion: Option<String>,
    pub app: Option<CheckApp>,
    pub output: Option<CheckOutput>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct CheckOutput {
    pub title: String,
    pub summary: Option<String>,
    pub text: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct CheckApp {
    pub id: u64,
    pub slug: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct CheckSuite {
    pub id: u64,
    #[serde(default)]
    pub pull_requests: Vec<CheckPullRequest>,
    pub head_sha: String,
    pub status: String,
    pub conclusion: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct Branch {
    pub name: String,
    pub commit: Commit,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct Commit {
    pub sha: String,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CloseReason {
    Completed,
    NotPlanned,
}

#[derive(Clone, Debug, Serialize)]
pub struct NewPullRequest {
    pub title: String,
    pub head: String,
    pub base: String,
    pub body: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct NewCheckRun {
    pub name: String,
    pub head_sha: String,
    pub external_id: String,
    pub status: CheckStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conclusion: Option<CheckConclusion>,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckStatus {
    Queued,
    InProgress,
    Completed,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckConclusion {
    Success,
    Failure,
    Neutral,
    Cancelled,
    TimedOut,
    ActionRequired,
    Skipped,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct LabelChange {
    pub id: u64,
    pub event: String,
    pub actor: Actor,
    pub label: Label,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone, Debug)]
pub struct RepoChanges {
    pub repository: String,
    pub issues: Vec<Issue>,
    pub comments: Vec<Comment>,
    pub labels: Vec<(u64, LabelChange)>,
    pub closed: Vec<(u64, PullClosed)>,
    pub pull_requests: Vec<PullRequest>,
    pub check_runs: Vec<CheckRun>,
    pub check_suites: Vec<CheckSuite>,
}

#[derive(Clone, Debug)]
pub struct CatchUp {
    pub next_cursor: DateTime<Utc>,
    pub repositories: Vec<RepoChanges>,
}

/// Heads include closed requests whose late check or merge facts still matter.
#[derive(Clone, Debug)]
pub struct ServedRepo {
    pub full_name: String,
    pub tracked_heads: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct PullClosed {
    pub actor: Actor,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct MergeGroup {
    pub head_sha: String,
    pub head_ref: String,
    pub base_ref: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct CheckPullRequest {
    pub number: u64,
}
