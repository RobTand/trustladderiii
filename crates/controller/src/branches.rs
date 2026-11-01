//! Repository branch conventions shared by worktrees and publication.
use crate::{Repository, Result, executor::Delivery};
use tl3_worktrees::{BranchName, branch_template::BranchTemplate};

#[derive(Clone)]
pub(crate) struct Branches(Vec<(Repository, BranchTemplate, BranchName)>);

impl Branches {
    pub(crate) fn new(repos: &[tl3_config::Repository]) -> Result<Self> {
        Ok(Self(
            repos
                .iter()
                .map(|repo| {
                    Ok((
                        Repository {
                            owner: repo.owner.clone(),
                            name: repo.name.clone(),
                            installation: repo.installation_id,
                        },
                        BranchTemplate::new(&repo.branch_template)?,
                        BranchName::new(&repo.base_branch)?,
                    ))
                })
                .collect::<Result<_>>()?,
        ))
    }

    fn find(&self, repo: &Repository) -> Result<&(Repository, BranchTemplate, BranchName)> {
        self.0
            .iter()
            .find(|(identity, _, _)| {
                identity.installation == repo.installation
                    && identity.owner.eq_ignore_ascii_case(&repo.owner)
                    && identity.name.eq_ignore_ascii_case(&repo.name)
            })
            .ok_or(crate::Error::Invalid(
                "repository branch conventions are not configured",
            ))
    }

    pub(crate) fn branch(&self, delivery: &Delivery) -> Result<BranchName> {
        Ok(self
            .find(&delivery.repository)?
            .1
            .format(delivery.issue, delivery.node().0)?)
    }

    pub(crate) fn matches(&self, delivery: &Delivery, branch: &str) -> Result<bool> {
        Ok(self.find(&delivery.repository)?.1.parse(branch)
            == Some((delivery.issue, delivery.node().0)))
    }

    pub(crate) fn base(&self, repo: &Repository) -> Result<&str> {
        Ok(self.find(repo)?.2.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repo_conventions_empty_registry_requires_configuration() {
        let fixture = crate::git_tests::Repo::new();
        let branches = Branches::new(&[]).unwrap();
        assert!(
            branches.branch(&fixture.delivery()).is_err(),
            "an empty registry must not invent repository conventions"
        );
    }

    #[test]
    fn repo_conventions_unknown_repository_requires_configuration() {
        let fixture = crate::git_tests::Repo::new();
        let config = tl3_config::Config::parse(include_str!("../../config/example.toml")).unwrap();
        let branches = Branches::new(&config.github.repos).unwrap();
        let mut delivery = fixture.delivery();
        delivery.repository.name = "unconfigured".into();
        assert!(
            branches.branch(&delivery).is_err(),
            "an unknown repository must not inherit default conventions"
        );
    }
}
