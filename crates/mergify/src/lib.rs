#![forbid(unsafe_code)]

use base64::{Engine, engine::general_purpose::STANDARD};
use serde::Deserialize;
use tl3_github::{CheckRun, GitHub, PullRequest};

pub const QUEUE_CHECK: &str = "Mergify Merge Queue";
pub const MERGIFY_APP_ID: u64 = 10562;
pub const QUEUE_LABEL: &str = "tl3-queue";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QueueState {
    Waiting,
    Queued,
    ChecksRunning,
    Merged,
    Dequeued,
    FailedCi,
    Conflict,
    Unknown,
}

#[derive(Clone, Debug)]
pub struct Observation {
    pub pull_request: PullRequest,
    pub state: QueueState,
    pub check: Option<CheckRun>,
    pub unapproved_merge: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    GitHub(#[from] tl3_github::Error),
    #[error("pull request head differs from the approved head")]
    HeadChanged,
    #[error("pull request is closed or merged")]
    PullRequestClosed,
    #[error("pull request changed during observation; read it again")]
    ChangedDuringRead,
}

#[derive(Clone)]
pub struct Mergify {
    github: GitHub,
}

impl Mergify {
    pub fn new(github: GitHub) -> Self {
        Self { github }
    }

    /// Add the queue label after TL3 posts its approval check.
    pub async fn queue(&self, repo: &str, number: u64, approved_head: &str) -> Result<(), Error> {
        let pull = self.github.pull_request(repo, number).await?;
        if pull.head.sha != approved_head {
            return Err(Error::HeadChanged);
        }
        if pull.state != "open" || pull.merged_at.is_some() {
            return Err(Error::PullRequestClosed);
        }
        self.github.add_label(repo, number, QUEUE_LABEL).await?;
        Ok(())
    }

    pub async fn dequeue(&self, repo: &str, number: u64) -> Result<(), Error> {
        Ok(self.github.remove_label(repo, number, QUEUE_LABEL).await?)
    }

    pub async fn read(
        &self,
        repo: &str,
        number: u64,
        approved_head: &str,
    ) -> Result<Observation, Error> {
        let before = self.github.pull_request(repo, number).await?;
        let checks = self.github.check_runs(repo, &before.head.sha).await?;
        let after = self.github.pull_request(repo, number).await?;
        if before != after {
            return Err(Error::ChangedDuringRead);
        }
        Ok(observe(after, &checks, approved_head))
    }
}

/// TL3 must ignore events from Mergify's temporary merge-queue PRs.
pub fn is_merge_queue_pr(pull_request: &PullRequest) -> bool {
    pull_request.head.branch.starts_with("mergify/merge-queue/")
        && pull_request.user.login == "mergify[bot]"
}

/// Only a check on this head from Mergify's GitHub App describes current state.
pub fn observe(pull_request: PullRequest, checks: &[CheckRun], approved_head: &str) -> Observation {
    let check = checks
        .iter()
        .filter(|check| {
            check.head_sha == pull_request.head.sha
                && check.name == QUEUE_CHECK
                && check
                    .app
                    .as_ref()
                    .is_some_and(|app| app.id == MERGIFY_APP_ID && app.slug == "mergify")
        })
        .max_by_key(|check| check.id)
        .cloned();
    let state = if pull_request.merged_at.is_some() {
        QueueState::Merged
    } else {
        let reported = check
            .as_ref()
            .map(check_state)
            .unwrap_or(QueueState::Unknown);
        // GitHub's merge record wins over a queue report, including a delayed one.
        if reported == QueueState::Merged {
            QueueState::Unknown
        } else {
            reported
        }
    };
    let unapproved_merge =
        pull_request.merged_at.is_some() && pull_request.head.sha != approved_head;
    Observation {
        pull_request,
        state,
        check,
        unapproved_merge,
    }
}

fn check_state(check: &CheckRun) -> QueueState {
    let Some(output) = &check.output else {
        return QueueState::Unknown;
    };
    if output.summary.as_ref().is_some_and(|summary| {
        summary.contains("<span itemprop=\"mergify.v")
            && !summary.contains("<span itemprop=\"mergify.v1.")
    }) {
        return QueueState::Unknown;
    }
    if let Some(summary) = &output.summary
        && let Some(encoded) = summary
            .split_once("<span itemprop=\"mergify.v1.")
            .map(|(_, s)| s)
    {
        let state = encoded
            .split_once('"')
            .and_then(|(encoded, _)| STANDARD.decode(encoded).ok())
            .and_then(|json| String::from_utf8(json).ok())
            .and_then(|json| payload_state(&json));
        return state.unwrap_or(QueueState::Unknown);
    }
    match output.title.as_str() {
        "In merge queue" => QueueState::Queued,
        "Running merge queue checks" => QueueState::ChecksRunning,
        "Merged via merge queue" => QueueState::Merged,
        "Dequeued — checks failed" => QueueState::FailedCi,
        "Dequeued — conflict with base branch" => QueueState::Conflict,
        "Merge queue is ready" => QueueState::Waiting,
        _ => QueueState::Unknown,
    }
}

#[derive(Deserialize)]
struct Payload {
    version: u64,
    state: String,
    step: Option<String>,
    #[serde(default)]
    dequeue_reason: Option<DequeueReason>,
}

#[derive(Deserialize)]
struct DequeueReason {
    code: String,
}

// Mergify embeds versioned base64 JSON in check summaries.
// These encodings are observed output, not a documented stable API.
fn payload_state(json: &str) -> Option<QueueState> {
    let payload: Payload = serde_json::from_str(json).ok()?;
    if payload.version != 1 {
        return Some(QueueState::Unknown);
    }
    Some(match payload.state.as_str() {
        "waiting" => QueueState::Waiting,
        "queued" => QueueState::Queued,
        "checking" => match payload.step.as_deref() {
            Some("checks_running") | None => QueueState::ChecksRunning,
            Some("checks_deferred") => QueueState::Queued,
            Some(_) => QueueState::Unknown,
        },
        "merged" => QueueState::Merged,
        "dequeued" => match payload
            .dequeue_reason
            .as_ref()
            .map(|reason| reason.code.as_str())
        {
            Some("CHECKS_FAILED" | "CHECKS_TIMEOUT") => QueueState::FailedCi,
            Some("CONFLICT_WITH_BASE_BRANCH" | "CONFLICT_WITH_ANOTHER_PR") => QueueState::Conflict,
            _ => QueueState::Dequeued,
        },
        _ => QueueState::Unknown,
    })
}
