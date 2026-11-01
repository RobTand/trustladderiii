#![forbid(unsafe_code)]

use base64::{Engine, engine::general_purpose::STANDARD};
use serde::Deserialize;
use tl3_github::{CheckRun, GitHub, PullRequest};

pub const QUEUE_CHECK: &str = "Mergify Merge Queue";
pub const MERGIFY_APP_ID: u64 = 10562;
const MERGIFY_BOT_ID: u64 = 37929162;
pub const QUEUE_LABEL: &str = "tl3-queue";
pub const QUEUE_RULE_CHECK: &str = "Rule: Queue TL3-approved PRs (queue)";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QueueState {
    Waiting,
    Queued,
    ChecksRunning,
    Merged,
    Dequeued,
    DequeuedByPerson,
    FailedCi,
    Conflict,
    Unknown,
}

#[derive(Clone, Debug)]
pub struct Observation {
    pub pull_request: PullRequest,
    pub state: QueueState,
    pub check: Option<CheckRun>,
    pub check_runs: Vec<CheckRun>,
    pub unapproved_merge: bool,
    pub dequeue_rule_concluded: bool,
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
    #[error("GitHub has not computed mergeability; read it on the next look")]
    MergeabilityPending,
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

    /// The queue command resets a previously dequeued PR; label presence cannot do that.
    pub async fn requeue(
        &self,
        repo: &str,
        number: u64,
        approved_head: &str,
        effect_key: &str,
    ) -> Result<(), Error> {
        let pull = self.github.pull_request(repo, number).await?;
        if pull.head.sha != approved_head {
            return Err(Error::HeadChanged);
        }
        if pull.state != "open" || pull.merged_at.is_some() {
            return Err(Error::PullRequestClosed);
        }
        self.github
            .post_command_comment(repo, number, effect_key, "@mergifyio queue")
            .await?;
        Ok(())
    }

    pub async fn dequeue(&self, repo: &str, number: u64) -> Result<(), Error> {
        Ok(self.github.remove_label(repo, number, QUEUE_LABEL).await?)
    }

    /// A command reply quotes its command, but has no parent comment ID.
    pub async fn command_refusal(
        &self,
        repo: &str,
        number: u64,
        effect_key: &str,
    ) -> Result<Option<String>, Error> {
        let mut comments = self.github.comments(repo, number).await?;
        comments.sort_by_key(|comment| comment.id);
        let marker = GitHub::marker(effect_key);
        let login = format!("{}[bot]", self.github.app_slug());
        let Some(command) = comments.iter().find(|comment| {
            comment.user.login == login && comment.body.lines().any(|line| line == marker)
        }) else {
            return Ok(None);
        };
        let Some(name @ ("queue" | "requeue")) = command
            .body
            .lines()
            .next()
            .and_then(|line| line.strip_prefix("@mergifyio "))
        else {
            return Ok(None);
        };
        let quoted = format!("> `{name}`");
        // Another sender's command ends the unambiguous reply window.
        for comment in comments.iter().filter(|comment| comment.id > command.id) {
            if comment.body.trim_start().starts_with("@mergifyio ") {
                break;
            }
            if comment.user.id != MERGIFY_BOT_ID
                || comment.user.login != "mergify[bot]"
                || comment.body.lines().next().map(str::trim) != Some(quoted.as_str())
            {
                continue;
            }
            for heading in comment
                .body
                .lines()
                .map(str::trim)
                .filter(|line| line.starts_with('#'))
            {
                let lower = heading.to_ascii_lowercase();
                if lower.contains("command disallowed") || lower.contains("command restrictions") {
                    return Ok(Some(heading.trim_start_matches('#').trim().into()));
                }
            }
        }
        Ok(None)
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
        let observation = observe(after, &checks, approved_head);
        if observation.state == QueueState::Dequeued
            && observation.pull_request.mergeable != Some(true)
            && matches!(
                observation.pull_request.mergeable_state.as_deref(),
                None | Some("unknown")
            )
        {
            return Err(Error::MergeabilityPending);
        }
        Ok(observation)
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
        } else if reported == QueueState::Dequeued
            && (pull_request.mergeable == Some(false)
                || pull_request.mergeable_state.as_deref() == Some("dirty"))
        {
            QueueState::Conflict
        } else {
            reported
        }
    };
    let dequeue_rule_concluded = check.as_ref().is_some_and(|queue| {
        checks
            .iter()
            .filter(|rule| {
                rule.head_sha == pull_request.head.sha
                    && rule.name == QUEUE_RULE_CHECK
                    && rule
                        .app
                        .as_ref()
                        .is_some_and(|app| app.id == MERGIFY_APP_ID && app.slug == "mergify")
            })
            .max_by_key(|rule| rule.id)
            .is_some_and(|rule| {
                rule.status == "completed"
                    && rule.conclusion.is_some()
                    && rule
                        .completed_at
                        .zip(queue.completed_at)
                        .is_some_and(|(finished, dequeued)| finished >= dequeued)
            })
    });
    let unapproved_merge =
        pull_request.merged_at.is_some() && pull_request.head.sha != approved_head;
    Observation {
        pull_request,
        state,
        check,
        check_runs: checks.to_vec(),
        unapproved_merge,
        dequeue_rule_concluded,
    }
}

fn check_state(check: &CheckRun) -> QueueState {
    if matches!(
        check.conclusion.as_deref(),
        Some("timed_out" | "startup_failure")
    ) {
        return QueueState::FailedCi;
    }
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
    match output.title.as_deref() {
        Some("In merge queue") => QueueState::Queued,
        Some("Running merge queue checks") => QueueState::ChecksRunning,
        Some("Merged via merge queue") => QueueState::Merged,
        Some(
            "Dequeued — checks failed"
            | "Dequeued — checks timeout"
            | "Dequeued — checks interrupted",
        ) => QueueState::FailedCi,
        Some(
            "Dequeued — conflict with base branch"
            | "Dequeued — conflict with another pull request",
        ) => QueueState::Conflict,
        Some(title) if title.starts_with("Dequeued — ") => {
            eprintln!("unrecognized Mergify dequeue title: {title:?}");
            QueueState::Dequeued
        }
        Some(title)
            if title
                .strip_prefix("The pull request #")
                .and_then(|number| number.strip_suffix(" is queued"))
                .is_some_and(|number| number.parse::<u64>().is_ok()) =>
        {
            QueueState::Queued
        }
        Some("Merge queue is ready") => QueueState::Waiting,
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
            // https://docs.mergify.com/api/statistics/ names the manual-command reason.
            Some("PR_MANUALLY_DEQUEUED") => QueueState::DequeuedByPerson,
            Some("CHECKS_FAILED" | "CHECKS_TIMEOUT" | "CHECKS_INTERRUPTED") => QueueState::FailedCi,
            Some("CONFLICT_WITH_BASE_BRANCH" | "CONFLICT_WITH_ANOTHER_PR") => QueueState::Conflict,
            code => {
                eprintln!("unrecognized Mergify dequeue code: {code:?}");
                QueueState::Dequeued
            }
        },
        _ => QueueState::Unknown,
    })
}

/// A timeout or an interrupted run reports runner/setup failure rather than a test failure.
pub fn ci_is_infrastructure(check: &CheckRun) -> bool {
    if matches!(
        check.conclusion.as_deref(),
        Some("timed_out" | "startup_failure")
    ) {
        return true;
    }
    let payload = check
        .output
        .as_ref()
        .and_then(|output| output.summary.as_ref())
        .and_then(|summary| summary.split_once("<span itemprop=\"mergify.v1."))
        .and_then(|(_, encoded)| encoded.split_once('"'))
        .and_then(|(encoded, _)| STANDARD.decode(encoded).ok())
        .and_then(|bytes| serde_json::from_slice::<Payload>(&bytes).ok());
    if let Some(payload) = payload {
        return payload.version == 1
            && payload.dequeue_reason.is_some_and(|reason| {
                matches!(
                    reason.code.as_str(),
                    "CHECKS_TIMEOUT" | "CHECKS_INTERRUPTED"
                )
            });
    }
    matches!(
        check
            .output
            .as_ref()
            .and_then(|output| output.title.as_deref()),
        Some("Dequeued — checks timeout" | "Dequeued — checks interrupted")
    )
}

/// The selected check belongs to Mergify on the original PR head.
pub fn merged_via_queue(observation: &Observation) -> bool {
    observation.check.as_ref().is_some_and(|check| {
        check.status == "completed"
            && check.conclusion.as_deref() == Some("success")
            && check_state(check) == QueueState::Merged
    })
}
