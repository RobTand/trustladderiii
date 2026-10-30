#![forbid(unsafe_code)]

use base64::{Engine, engine::general_purpose::STANDARD};
use serde::Deserialize;
use tl3_github::{CheckRun, Comment, GitHub, PullRequest};

pub const QUEUE_CHECK: &str = "Mergify Merge Queue";
pub const MERGIFY_APP_ID: u64 = 10562;
pub const MERGIFY_BOT_ID: u64 = 37929162;

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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QueueEvent {
    pub comment: Comment,
    pub state: QueueState,
}

#[derive(Clone, Debug)]
pub struct Observation {
    pub pull_request: PullRequest,
    pub state: QueueState,
    pub check: Option<CheckRun>,
    /// Historical reports, not proof of the current head's queue membership.
    pub events: Vec<QueueEvent>,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    GitHub(#[from] tl3_github::Error),
    #[error("effect key must not be blank")]
    EmptyEffectKey,
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

    /// A receipt means GitHub stored the command. Read state to confirm queue entry.
    pub async fn queue(
        &self,
        repo: &str,
        number: u64,
        approved_head: &str,
        effect_key: &str,
    ) -> Result<Comment, Error> {
        if effect_key.trim().is_empty() {
            return Err(Error::EmptyEffectKey);
        }
        let pull = self.github.pull_request(repo, number).await?;
        if pull.head.sha != approved_head {
            return Err(Error::HeadChanged);
        }
        if pull.state != "open" || pull.merged_at.is_some() {
            return Err(Error::PullRequestClosed);
        }
        Ok(self
            .github
            .post_comment(
                repo,
                number,
                &format!("mergify:queue:{effect_key}"),
                "@mergifyio queue",
            )
            .await?)
    }

    /// Cancels pending queue commands as well as existing queue membership.
    pub async fn dequeue(
        &self,
        repo: &str,
        number: u64,
        effect_key: &str,
    ) -> Result<Comment, Error> {
        if effect_key.trim().is_empty() {
            return Err(Error::EmptyEffectKey);
        }
        Ok(self
            .github
            .post_comment(
                repo,
                number,
                &format!("mergify:dequeue:{effect_key}"),
                "@mergifyio dequeue",
            )
            .await?)
    }

    pub async fn read(&self, repo: &str, number: u64) -> Result<Observation, Error> {
        let before = self.github.pull_request(repo, number).await?;
        let checks = self.github.check_runs(repo, &before.head.sha).await?;
        let comments = self.github.comments(repo, number).await?;
        let after = self.github.pull_request(repo, number).await?;
        if before != after {
            return Err(Error::ChangedDuringRead);
        }
        Ok(observe(after, &checks, &comments))
    }
}

/// Only a check on this head from Mergify's GitHub App describes current state.
pub fn observe(
    pull_request: PullRequest,
    checks: &[CheckRun],
    comments: &[Comment],
) -> Observation {
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
    let mut events: Vec<_> = comments
        .iter()
        .filter(|c| c.user.id == MERGIFY_BOT_ID && c.user.login == "mergify[bot]")
        .filter_map(|comment| {
            let payload = comment.body.split_once("-*- Mergify Payload -*-\n")?.1;
            let payload = payload.split_once("\n-*- Mergify Payload End -*-")?.0;
            let mut state = payload_state(payload)?;
            if state == QueueState::Dequeued {
                // Older reports have no reason object. Read only their Reason section.
                let reason = comment.body.split_once("\n## Reason\n\n").map(|(_, s)| s);
                state = match reason.and_then(|s| s.lines().next()) {
                    Some("The pull request conflicts with the base branch") => QueueState::Conflict,
                    Some("The merge conditions cannot be satisfied due to failing checks") => {
                        QueueState::FailedCi
                    }
                    _ => state,
                };
            }
            Some(QueueEvent {
                comment: comment.clone(),
                state,
            })
        })
        .collect();
    events.sort_by_key(|event| (event.comment.updated_at, event.comment.id));
    Observation {
        pull_request,
        state,
        check,
        events,
    }
}

fn check_state(check: &CheckRun) -> QueueState {
    let Some(output) = &check.output else {
        return QueueState::Unknown;
    };
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
        "Dequeued — checks failed" | "Dequeued — checks timeout" => QueueState::FailedCi,
        "Dequeued — conflict with base branch"
        | "Dequeued — conflict with another pull request" => QueueState::Conflict,
        title if title.starts_with("Dequeued — ") => QueueState::Dequeued,
        "Merge queue is ready"
        | "Waiting for queue conditions"
        | "Waiting for queue conditions to match" => QueueState::Waiting,
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

// Mergify embeds versioned JSON in comments and base64 JSON in check summaries.
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
