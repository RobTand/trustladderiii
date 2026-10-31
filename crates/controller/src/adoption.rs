use crate::{
    Bootstrap, Error, LabelledIssue, Repository, Result,
    cli::Input,
    ingress::{GitHubDelivery, GitHubTranslator},
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use tl3_github::{GitHub, webhook::Event};

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ProposedPullRequest {
    pub number: u64,
    pub head: String,
    pub head_repository: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Clarification {
    pub repository: Repository,
    pub number: u64,
}
impl Clarification {
    pub fn key(&self) -> String {
        format!(
            "adoption-question:{}/{}/{}:{}",
            self.repository.owner.to_ascii_lowercase(),
            self.repository.name.to_ascii_lowercase(),
            self.repository.installation,
            self.number
        )
    }
}

pub struct Translator {
    clients: BTreeMap<u64, GitHub>,
    bootstrap: Bootstrap,
}
impl Translator {
    pub fn new(clients: BTreeMap<u64, GitHub>, bootstrap: Bootstrap) -> Result<Self> {
        Ok(Self { clients, bootstrap })
    }
}
impl GitHubTranslator for Translator {
    fn translate(
        &mut self,
        delivery: &GitHubDelivery,
        _snapshot: &tl3_store::Snapshot,
    ) -> Result<Vec<Input>> {
        let GitHubDelivery::Webhook(delivery) = delivery else {
            return Ok(vec![]);
        };
        let Some(Event::PullRequestLabeled {
            context,
            pull_request,
            label,
            actor,
        }) = &delivery.event
        else {
            return Ok(vec![]);
        };
        if actor.id != self.bootstrap.rob || label.name != "tl3" || pull_request.state != "open" {
            return Ok(vec![]);
        }
        let Some(repository) = self
            .bootstrap
            .repos
            .iter()
            .find(|r| {
                r.installation == context.installation_id
                    && r.owner
                        .eq_ignore_ascii_case(&context.repository.owner.login)
                    && r.name.eq_ignore_ascii_case(&context.repository.name)
            })
            .cloned()
        else {
            return Ok(vec![]);
        };
        let repo = format!("{}/{}", repository.owner, repository.name);
        if !pull_request
            .head
            .repo
            .as_ref()
            .is_some_and(|r| r.full_name().eq_ignore_ascii_case(&repo))
        {
            return Ok(vec![]);
        }
        let issues = tl3_github::closing_issues(pull_request.body.as_deref().unwrap_or(""));
        let [number] = issues.as_slice() else {
            return Ok(vec![Input::AdoptionClarification(Clarification {
                repository,
                number: pull_request.number,
            })]);
        };
        tl3_worktrees::CommitId::new(&pull_request.head.sha)
            .map_err(|_| Error::Invalid("proposed pull request needs an exact head"))?;
        let client = self
            .clients
            .get(&repository.installation)
            .ok_or(Error::Invalid("adoption installation client missing"))?;
        let issue = crate::executor::runtime()?.block_on(client.issue(&repo, *number))?;
        if issue.pull_request.is_some() || issue.state != "open" {
            return Ok(vec![]);
        }
        Ok(vec![Input::ProposedIssue {
            issue: LabelledIssue {
                delivery: delivery.id.clone(),
                owner: repository.owner,
                repo: repository.name,
                installation: repository.installation,
                number: issue.number,
                title: issue.title,
                actor: actor.id,
                label: label.name.clone(),
            },
            proposed: ProposedPullRequest {
                number: pull_request.number,
                head: pull_request.head.sha.clone(),
                head_repository: repo,
            },
        }])
    }
}
