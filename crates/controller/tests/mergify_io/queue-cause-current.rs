//! #854b boundary: existing PR/check payloads provide passive cause evidence.
use super::*;
use crate::queue_cause::{GateEvidence, QueueContext};

pub(super) fn context(pull: &tl3_github::PullRequest) -> QueueContext {
    QueueContext {
        base: pull.base.sha.clone(),
        branch: pull.base.branch.clone(),
        batch: None,
        members: vec![pull.number],
    }
}

/// Mergify's bounded CI metadata, not display prose, identifies batch members.
/// Unknown/malformed shapes supply no independence evidence.
pub(super) fn batch_context(pull: &tl3_github::PullRequest) -> Option<QueueContext> {
    if !is_merge_queue_pr(pull) {
        return None;
    }
    let body = pull.body.as_deref()?;
    if body.len() > 65536 {
        return None;
    }
    let metadata = body.split_once("```yaml\n---\n")?.1.split_once("\n```")?.0;
    let mut base = None;
    let mut members = Vec::new();
    let mut in_members = false;
    for line in metadata.lines().take(512) {
        if let Some(sha) = line.strip_prefix("checking_base_sha: ") {
            if base.is_some()
                || sha.len() != 40
                || !sha.bytes().all(|byte| byte.is_ascii_hexdigit())
            {
                return None;
            }
            base = Some(sha.to_owned());
        }
        if line == "pull_requests:" {
            if in_members || !members.is_empty() {
                return None;
            }
            in_members = true;
            continue;
        }
        if in_members {
            if let Some(number) = line.strip_prefix("  - number: ") {
                let number = number.parse::<u64>().ok().filter(|number| *number > 0)?;
                if members.len() == 64 || members.contains(&number) {
                    return None;
                }
                members.push(number);
            } else if !line.starts_with(' ') {
                in_members = false;
            } else if !line.starts_with("    scopes:") && !line.starts_with("      - ") {
                return None;
            }
        }
    }
    let base = base?;
    if members.is_empty() || base != pull.base.sha {
        return None;
    }
    Some(QueueContext {
        base,
        branch: pull.base.branch.clone(),
        batch: Some(pull.head.sha.clone()),
        members,
    })
}

pub(super) fn merge(repo: &Repository, pull: &tl3_github::PullRequest) -> Option<QueueFact> {
    if is_merge_queue_pr(pull) || pull.state != "closed" {
        return None;
    }
    let at = pull.merged_at?;
    let base = pull.merge_commit_sha.as_ref()?;
    if base.len() != 40 || !base.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    Some(QueueFact {
        repository: repo.clone(),
        number: pull.number,
        evidence: Evidence::Gate(GateEvidence {
            context: QueueContext {
                base: base.clone(),
                branch: pull.base.branch.clone(),
                batch: None,
                members: vec![],
            },
            at,
            fingerprint: None,
            successful: true,
            passed: vec![],
        }),
    })
}

pub(super) fn batch(
    repo: &Repository,
    pull: &tl3_github::PullRequest,
    checks: &[CheckRun],
) -> Option<QueueFact> {
    let context = batch_context(pull)?;
    let mut latest = BTreeMap::new();
    for check in checks.iter().filter(|check| check.head_sha == pull.head.sha) {
        let entry = latest.entry((check.app.as_ref().map(|app| app.id), &check.name)).or_insert(check);
        if check.id > entry.id { *entry = check; }
    }
    let checks: Vec<_> = latest.values().copied().collect();
    if checks.is_empty() || checks.len() > 128 || checks.iter().any(|check| check.status != "completed") { return None; }
    let at = checks.iter().filter_map(|check| check.completed_at).max()?;
    let mut parts = Vec::new();
    let mut complete = true;
    let mut failed = false;
    let mut passed = Vec::new();
    let mut successful = true;
    for check in checks {
        if check.conclusion.as_deref() == Some("success") {
            if let Some(app) = &check.app { passed.push(crate::queue_cause::CheckIdentity { app: app.id, name: check.name.clone() }); }
        } else if check.conclusion.as_deref() != Some("skipped") { successful = false; }
        if check.conclusion.as_deref() != Some("failure") {
            continue;
        }
        failed = true;
        let app = check.app.as_ref()?;
        let output = check.output.as_ref()?;
        let text = format!(
            "{}\n{}\n{}",
            output.title.as_deref().unwrap_or_default(),
            output.summary.as_deref().unwrap_or_default(),
            output.text.as_deref().unwrap_or_default()
        );
        let tests = ci_evidence::failures(&text);
        complete &= !tests.is_empty();
        parts.extend(tests.into_iter().map(|test| {
            (
                Some(app.id),
                check.name.clone(),
                test.test,
            )
        }));
    }
    if !failed && (!successful || passed.is_empty()) { return None; }
    Some(QueueFact {
        repository: repo.clone(),
        number: pull.number,
        evidence: Evidence::Gate(GateEvidence {
            context,
            at,
            fingerprint: (failed && complete).then(|| crate::queue_cause::fingerprint(&mut parts)),
            successful,
            passed,
        }),
    })
}

