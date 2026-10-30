use super::*;
use tl3_github::{
    CheckConclusion, CheckRun, CheckStatus, NewCheckRun, NewPullRequest, PullRequest,
};

pub const APPROVAL_CHECK: &str = "TL3 approval";

pub(super) fn handles(delivery: &Delivery) -> bool {
    matches!(
        delivery.output,
        Out::Effect {
            kind: EffectKind::PostApproval | EffectKind::OpenPr | EffectKind::ClosePr,
            ..
        }
    )
}

fn head(delivery: &Delivery) -> Result<&str> {
    delivery
        .head_sha
        .as_deref()
        .filter(|sha| {
            matches!(sha.len(), 40 | 64)
                && sha
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        })
        .ok_or(Error::Invalid("approval requires its exact commit SHA"))
}

fn external_id(delivery: &Delivery) -> Result<String> {
    Ok(format!("{}:{}", delivery.key, head(delivery)?))
}

fn approved(check: &CheckRun, client: &GitHub, delivery: &Delivery) -> Result<bool> {
    Ok(
        check.external_id.as_deref() == Some(external_id(delivery)?.as_str())
            && check
                .app
                .as_ref()
                .is_some_and(|app| app.id == client.app_id())
            && check.head_sha == head(delivery)?
            && check.name == APPROVAL_CHECK
            && check.status == "completed"
            && check.conclusion.as_deref() == Some("success"),
    )
}

fn binding(delivery: &Delivery, pull: &PullRequest) -> Completion {
    Completion::confirmed(vec![Observation::PullRequest {
        node: delivery.node(),
        number: pull.number,
    }])
}

impl Adapter {
    fn branch_pr(
        &self,
        client: &GitHub,
        repo: &str,
        delivery: &Delivery,
    ) -> Result<Option<PullRequest>> {
        let branch = super::super::worktrees::Real::branch(delivery)?;
        let mut pulls = self.runtime.block_on(client.pull_requests_for_branch(
            repo,
            &delivery.repository.owner,
            branch.as_str(),
        ))?;
        pulls.retain(|pr| {
            pr.head.branch == branch.as_str()
                && pr.head.repo.as_ref().is_some_and(|repo| {
                    repo.full_name().eq_ignore_ascii_case(&format!(
                        "{}/{}",
                        delivery.repository.owner, delivery.repository.name
                    ))
                })
                && pr.base.branch == "main"
        });
        pulls.sort_by_key(|pr| (pr.state == "open", pr.number));
        Ok(pulls.pop())
    }

    pub(super) fn publish(&mut self, delivery: &Delivery) -> Result<Completion> {
        let client = self.client(delivery)?;
        let repo = format!("{}/{}", delivery.repository.owner, delivery.repository.name);
        match delivery.output {
            Out::Effect {
                kind: EffectKind::PostApproval,
                ..
            } => {
                let check = self.runtime.block_on(client.create_check_run(
                    &repo,
                    &NewCheckRun {
                        name: APPROVAL_CHECK.into(),
                        head_sha: head(delivery)?.into(),
                        external_id: external_id(delivery)?,
                        status: CheckStatus::Completed,
                        conclusion: Some(CheckConclusion::Success),
                    },
                ))?;
                if !approved(&check, &client, delivery)? {
                    return Err(Error::Invalid(
                        "GitHub approval evidence differs from the approved head",
                    ));
                }
                Ok(Completion::confirmed(vec![]))
            }
            Out::Effect {
                kind: EffectKind::OpenPr,
                ..
            } => {
                if let Some(mut pull) = self.branch_pr(&client, &repo, delivery)? {
                    if pull.state == "closed" && pull.merged_at.is_none() {
                        pull = self
                            .runtime
                            .block_on(client.reopen_pull_request(&repo, pull.number))?;
                    }
                    return Ok(binding(delivery, &pull));
                }
                let issue = self.runtime.block_on(client.issue(&repo, delivery.issue))?;
                let branch = super::super::worktrees::Real::branch(delivery)?;
                let pull = self.runtime.block_on(client.open_pull_request(
                    &repo,
                    &NewPullRequest {
                        title: issue.title,
                        head: branch.as_str().into(),
                        base: "main".into(),
                        body: format!(
                            "Implements the request in issue {}.\n\n{}",
                            delivery.issue,
                            GitHub::marker(&delivery.key)
                        ),
                    },
                ))?;
                Ok(binding(delivery, &pull))
            }
            Out::Effect {
                kind: EffectKind::ClosePr,
                ..
            } => {
                if let Some(pull) = self.branch_pr(&client, &repo, delivery)?
                    && pull.state != "closed"
                    && pull.merged_at.is_none()
                {
                    self.runtime
                        .block_on(client.close_pull_request(&repo, pull.number))?;
                }
                Ok(Completion::confirmed(vec![]))
            }
            _ => Err(Error::Invalid("unsupported publication effect")),
        }
    }

    pub(super) fn reconcile_publication(&self, delivery: &Delivery) -> Result<Reconciliation> {
        let client = self.client(delivery)?;
        let repo = format!("{}/{}", delivery.repository.owner, delivery.repository.name);
        let completion = match delivery.output {
            Out::Effect {
                kind: EffectKind::PostApproval,
                ..
            } => {
                let checks = self
                    .runtime
                    .block_on(client.check_runs(&repo, head(delivery)?))?;
                if checks
                    .iter()
                    .any(|check| approved(check, &client, delivery).unwrap_or(false))
                {
                    Some(Completion::confirmed(vec![]))
                } else {
                    None
                }
            }
            Out::Effect {
                kind: EffectKind::OpenPr,
                ..
            } => self
                .branch_pr(&client, &repo, delivery)?
                .filter(|pr| pr.state == "open" || pr.merged_at.is_some())
                .map(|pr| binding(delivery, &pr)),
            Out::Effect {
                kind: EffectKind::ClosePr,
                ..
            } => {
                if self
                    .branch_pr(&client, &repo, delivery)?
                    .is_none_or(|pr| pr.state == "closed" || pr.merged_at.is_some())
                {
                    Some(Completion::confirmed(vec![]))
                } else {
                    None
                }
            }
            _ => return Err(Error::Invalid("unsupported publication reconciliation")),
        };
        Ok(completion
            .map(Reconciliation::Found)
            .unwrap_or(Reconciliation::Absent))
    }
}
