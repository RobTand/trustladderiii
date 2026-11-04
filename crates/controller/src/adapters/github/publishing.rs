use super::*;
use crate::publication_failure::{self, Endpoint};
use tl3_github::{
    CheckConclusion, CheckOutput, CheckRun, CheckStatus, NewCheckRun, NewPullRequest, PullRequest,
    UpdateCheckRun,
};
use tl3_worktrees::CommitId;

pub const APPROVAL_CHECK: &str = "TL3 approval";

#[derive(serde::Serialize, serde::Deserialize)]
enum ApprovalAttempt {
    Preflight,
    Dispatching,
    Observed,
    Uncertain,
    Refused,
}

pub(super) fn handles(delivery: &Delivery) -> bool {
    matches!(
        delivery.output,
        Out::Effect {
            kind: EffectKind::PostApproval | EffectKind::OpenPr | EffectKind::ClosePr,
            ..
        }
    )
}

fn head(delivery: &Delivery) -> Result<CommitId> {
    Ok(CommitId::new(delivery.head_sha.as_deref().ok_or(
        Error::Invalid("approval requires its exact commit SHA"),
    )?)?)
}

fn external_id(delivery: &Delivery) -> Result<String> {
    Ok(format!("{}:{}", delivery.key, head(delivery)?.as_str()))
}

fn matches_approval(check: &CheckRun, client: &GitHub, delivery: &Delivery) -> Result<bool> {
    Ok(
        check.external_id.as_deref() == Some(external_id(delivery)?.as_str())
            && check
                .app
                .as_ref()
                .is_some_and(|app| app.id == client.app_id())
            && check.head_sha == head(delivery)?.as_str()
            && check.name == APPROVAL_CHECK,
    )
}

fn approval_output() -> CheckOutput {
    CheckOutput {
        title: Some(APPROVAL_CHECK.into()),
        summary: Some("This head passed TL3's required checks and independent review.".into()),
        text: Some(String::new()),
    }
}

fn approved(check: &CheckRun, client: &GitHub, delivery: &Delivery) -> Result<bool> {
    Ok(matches_approval(check, client, delivery)?
        && check.status == "completed"
        && check.conclusion.as_deref() == Some("success")
        && check.output.as_ref() == Some(&approval_output()))
}

fn binding(delivery: &Delivery, pull: &PullRequest) -> Completion {
    let mut observations = vec![Observation::PullRequest {
        node: delivery.node(),
        number: pull.number,
    }];
    if pull.merged_at.is_none()
        && pull.mergeable == Some(false)
        && delivery.head_sha.as_deref() == Some(pull.head.sha.as_str())
    {
        observations.push(Observation::Kernel(tl3_kernel::Event::Fact {
            node: delivery.node(),
            fact: tl3_kernel::Fact::Conflict,
        }));
    }
    Completion::confirmed(observations)
}

impl Adapter {
    pub(super) fn close_original(&self, delivery: &Delivery) -> Result<()> {
        if !matches!(
            delivery.output,
            Out::Notice {
                notice: tl3_kernel::Notice::Summary(tl3_kernel::Outcome::Merged),
                ..
            }
        ) {
            return Ok(());
        }
        let Some(proposed) = &delivery.proposed else {
            return Ok(());
        };
        let number = delivery
            .pull_request
            .ok_or(Error::Invalid("merged adoption has no landed pull request"))?;
        let client = self.client(delivery)?;
        let repo = format!("{}/{}", delivery.repository.owner, delivery.repository.name);
        let landed = self.runtime.block_on(client.pull_request(&repo, number))?;
        if landed.merged_at.is_none() {
            return Err(Error::Invalid(
                "original can close only after TL3 pull request merged",
            ));
        }
        self.runtime.block_on(client.post_comment(&repo, proposed.number, &format!("{}:original", delivery.key), &format!("Completed in https://github.com/{repo}/pull/{number}. This pull request is superseded.")))?;
        let original = self
            .runtime
            .block_on(client.pull_request(&repo, proposed.number))?;
        if original.state != "closed" && original.merged_at.is_none() {
            self.runtime
                .block_on(client.close_pull_request(&repo, proposed.number))?;
        }
        Ok(())
    }

    fn branch_pr(
        &self,
        client: &GitHub,
        repo: &str,
        delivery: &Delivery,
    ) -> Result<Option<PullRequest>> {
        let branch = self.branches.branch(delivery)?;
        let base = self.branches.base(&delivery.repository)?;
        let pulls = self
            .runtime
            .block_on(client.pull_requests_for_branch(
                repo,
                &delivery.repository.owner,
                branch.as_str(),
            ))
            .map_err(|error| publication_failure::http(error, Endpoint::Pulls))?;
        let mut owned = Vec::new();
        for pr in pulls {
            if self.branches.matches(delivery, &pr.head.branch)?
                && pr.head.repo.as_ref().is_some_and(|repo| {
                    repo.full_name().eq_ignore_ascii_case(&format!(
                        "{}/{}",
                        delivery.repository.owner, delivery.repository.name
                    ))
                })
                && pr.base.branch == base
            {
                owned.push(pr);
            }
        }
        owned.sort_by_key(|pr| (pr.state == "open", pr.number));
        Ok(owned.pop())
    }

    pub(super) fn publish(&mut self, delivery: &Delivery) -> Result<Completion> {
        let client = self.client(delivery)?;
        let repo = format!("{}/{}", delivery.repository.owner, delivery.repository.name);
        match delivery.output {
            Out::Effect {
                kind: EffectKind::PostApproval,
                ..
            } => {
                let attempt = self.approval_path(delivery)?;
                // Preflight has no outstanding mutation. A crash or read error here is retryable.
                self.records
                    .write(&attempt, &serde_json::to_vec(&ApprovalAttempt::Preflight)?)?;
                let checks = self
                    .runtime
                    .block_on(client.check_runs(&repo, head(delivery)?.as_str()))
                    .map_err(|error| publication_failure::http(error, Endpoint::HeadChecks))?;
                if self.converge_approvals(&client, &repo, delivery, &checks)? {
                    self.records
                        .write(&attempt, &serde_json::to_vec(&ApprovalAttempt::Observed)?)?;
                    return Ok(Completion::confirmed(vec![]));
                }
                // Dispatch intent closes the crash window before POST. Uncertain is
                // recorded only after a sent attempt returns an unknown outcome.
                self.records.write(
                    &attempt,
                    &serde_json::to_vec(&ApprovalAttempt::Dispatching)?,
                )?;
                let check = self.runtime.block_on(client.post_check_run_tracked(
                    &repo,
                    &NewCheckRun {
                        name: APPROVAL_CHECK.into(),
                        head_sha: head(delivery)?.as_str().into(),
                        external_id: external_id(delivery)?,
                        status: CheckStatus::Completed,
                        conclusion: Some(CheckConclusion::Success),
                    },
                    Some(&approval_output()),
                ));
                if matches!(
                    &check,
                    Err(tl3_github::PostFailure {
                        error: tl3_github::Error::Http { status: 404 },
                        ..
                    })
                ) {
                    self.records
                        .write(&attempt, &serde_json::to_vec(&ApprovalAttempt::Refused)?)?;
                }
                let check = match check {
                    Ok(check) => check,
                    Err(failure) if !failure.sent => {
                        self.records
                            .write(&attempt, &serde_json::to_vec(&ApprovalAttempt::Refused)?)?;
                        return Err(publication_failure::http(
                            failure.error,
                            Endpoint::CheckRuns,
                        ));
                    }
                    Err(tl3_github::PostFailure { error, .. })
                        if matches!(
                            error.failure_class(),
                            tl3_github::FailureClass::Permanent(_)
                        ) =>
                    {
                        return Err(publication_failure::http(error, Endpoint::CheckRuns));
                    }
                    Err(_) => {
                        self.records
                            .write(&attempt, &serde_json::to_vec(&ApprovalAttempt::Uncertain)?)?;
                        return Err(Error::ApprovalAmbiguous);
                    }
                };
                if !approved(&check, &client, delivery)? {
                    self.records
                        .write(&attempt, &serde_json::to_vec(&ApprovalAttempt::Uncertain)?)?;
                    return Err(Error::ApprovalAmbiguous);
                }
                self.records
                    .write(&attempt, &serde_json::to_vec(&ApprovalAttempt::Observed)?)?;
                Ok(Completion::confirmed(vec![]))
            }
            Out::Effect {
                kind: EffectKind::OpenPr,
                ..
            } => {
                let branch = self.branches.branch(delivery)?;
                let issue = self
                    .runtime
                    .block_on(client.issue(&repo, delivery.issue))
                    .map_err(|error| publication_failure::http(error, Endpoint::Issue))?;
                let request = delivery
                    .request_text
                    .as_deref()
                    .or(issue.body.as_deref())
                    .unwrap_or("");
                let evidence = crate::evidence::publication(delivery, request)?;
                if let Some(mut pull) = self.branch_pr(&client, &repo, delivery)? {
                    let body = pull.body.as_deref().unwrap_or_default();
                    let body = self.publication_body(
                        delivery,
                        request,
                        body,
                        &evidence,
                        (&pull.head.branch, pull.draft),
                    )?;
                    if pull.body.as_deref().unwrap_or_default() != body {
                        pull = self
                            .runtime
                            .block_on(client.update_pull_request_body(&repo, pull.number, &body))
                            .map_err(|error| publication_failure::http(error, Endpoint::Pull))?;
                        self.validate_publication_body(
                            delivery,
                            &pull.head.branch,
                            pull.draft,
                            pull.body.as_deref().unwrap_or_default(),
                        )?;
                    }
                    if pull.state == "closed" && pull.merged_at.is_none() {
                        pull = self
                            .runtime
                            .block_on(client.reopen_pull_request(&repo, pull.number))
                            .map_err(|error| publication_failure::http(error, Endpoint::Pull))?;
                    }
                    return Ok(binding(delivery, &pull));
                }
                let body = format!(
                    "Refs #{}.{}\n\n{}",
                    delivery.issue,
                    delivery
                        .proposed
                        .as_ref()
                        .map(|proposed| format!(
                            "\n\nCompletes https://github.com/{repo}/pull/{}.",
                            proposed.number
                        ))
                        .unwrap_or_default(),
                    GitHub::marker(&delivery.key)
                );
                let body = self.publication_body(
                    delivery,
                    request,
                    &body,
                    &evidence,
                    (branch.as_str(), false),
                )?;
                let pull = self
                    .runtime
                    .block_on(client.open_pull_request(
                        &repo,
                        &NewPullRequest {
                            title: crate::redaction::text(&issue.title)?,
                            head: branch.as_str().into(),
                            base: self.branches.base(&delivery.repository)?.into(),
                            body,
                        },
                    ))
                    .map_err(publication_failure::open_pr)?;
                self.validate_publication_body(
                    delivery,
                    &pull.head.branch,
                    pull.draft,
                    pull.body.as_deref().unwrap_or_default(),
                )?;
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
                        .block_on(client.close_pull_request(&repo, pull.number))
                        .map_err(|error| publication_failure::http(error, Endpoint::Pull))?;
                }
                Ok(Completion::confirmed(vec![]))
            }
            _ => Err(Error::Invalid("unsupported publication effect")),
        }
    }

    fn publication_body(
        &self,
        delivery: &Delivery,
        request: &str,
        body: &str,
        evidence: &str,
        target: (&str, bool),
    ) -> Result<String> {
        let (branch, draft) = target;
        let rules = self.body_lines(&delivery.repository);
        let body = crate::pr_body::compose(
            body,
            rules,
            Some(request),
            &self.semantic_paths(delivery),
            &crate::evidence::required_publication_lines(delivery),
        )?;
        let body = crate::evidence::publication_lines(delivery, &body);
        let body = crate::redaction::text(&body)?;
        let body = crate::evidence::with_publication(&body, evidence);
        self.validate_publication_body(delivery, branch, draft, &body)?;
        Ok(body)
    }

    fn validate_publication_body(
        &self,
        delivery: &Delivery,
        branch: &str,
        draft: bool,
        body: &str,
    ) -> Result<()> {
        crate::evidence::validate_body(body)?;
        crate::pr_body::validate(body, self.body_lines(&delivery.repository))?;
        crate::evidence::validate_publication(delivery, branch, draft, body)
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
                    .block_on(client.check_runs(&repo, head(delivery)?.as_str()))
                    .map_err(|error| publication_failure::http(error, Endpoint::HeadChecks))?;
                if self.converge_approvals(&client, &repo, delivery, &checks)? {
                    Some(Completion::confirmed(vec![]))
                } else if self.approval_was_refused(delivery)? {
                    None
                } else {
                    return Ok(Reconciliation::Pending);
                }
            }
            Out::Effect {
                kind: EffectKind::OpenPr,
                ..
            } => {
                let evidence = crate::evidence::publication(
                    delivery,
                    delivery.request_text.as_deref().unwrap_or(""),
                )?;
                self.branch_pr(&client, &repo, delivery)?
                    .filter(|pr| pr.state == "open" || pr.merged_at.is_some())
                    .filter(|pr| {
                        let body = pr.body.as_deref().unwrap_or_default();
                        crate::evidence::validate_body(body).is_ok()
                            && crate::evidence::validate_publication(
                                delivery,
                                &pr.head.branch,
                                pr.draft,
                                body,
                            )
                            .is_ok()
                            && crate::pr_body::gaps(body, self.body_lines(&delivery.repository))
                                .is_empty()
                            && crate::evidence::with_publication(body, &evidence) == body
                    })
                    .map(|pr| binding(delivery, &pr))
            }
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
            .map(|completion| Reconciliation::Found(Box::new(completion)))
            .unwrap_or(Reconciliation::Absent))
    }

    fn approval_path(&self, delivery: &Delivery) -> Result<std::path::PathBuf> {
        Ok(self
            .records
            .path(&external_id(delivery)?)
            .with_extension("approval"))
    }

    pub(super) fn retire_approval(&self, delivery: &Delivery) -> Result<()> {
        let path = self.approval_path(delivery)?;
        match std::fs::remove_file(&path) {
            Ok(()) => std::fs::File::open(
                path.parent()
                    .ok_or(Error::Invalid("approval directory missing"))?,
            )?
            .sync_all()?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        Ok(())
    }

    /// Q27, 2026-11-04: every own-app copy for this head/key has the intended state.
    /// PATCH is idempotent; partial progress and concurrent reconcilers converge.
    fn converge_approvals(
        &self,
        client: &GitHub,
        repo: &str,
        delivery: &Delivery,
        checks: &[CheckRun],
    ) -> Result<bool> {
        let mut found = false;
        for check in checks {
            if !matches_approval(check, client, delivery)? {
                continue;
            }
            found = true;
            if !approved(check, client, delivery)? {
                let updated = self
                    .runtime
                    .block_on(client.update_check_run(
                        repo,
                        check.id,
                        &UpdateCheckRun {
                            status: CheckStatus::Completed,
                            conclusion: CheckConclusion::Success,
                            output: approval_output(),
                        },
                    ))
                    .map_err(|error| publication_failure::http(error, Endpoint::CheckRuns))?;
                if !approved(&updated, client, delivery)? {
                    return Err(Error::Invalid("approval update evidence differs"));
                }
            }
        }
        Ok(found)
    }

    fn approval_was_refused(&self, delivery: &Delivery) -> Result<bool> {
        let path = self
            .records
            .path(&external_id(delivery)?)
            .with_extension("approval");
        match std::fs::read(path) {
            Ok(bytes) => Ok(matches!(
                serde_json::from_slice::<ApprovalAttempt>(&bytes)?,
                ApprovalAttempt::Refused | ApprovalAttempt::Preflight
            )),
            // Legacy attempts have no record proving they were refused.
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(error.into()),
        }
    }
}
