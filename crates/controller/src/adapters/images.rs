use crate::Repository;
use std::collections::BTreeMap;

#[derive(Clone, Default)]
pub(super) struct Images(BTreeMap<(String, String, u64), (Option<String>, bool)>);

impl Images {
    pub(super) fn new(repos: &[tl3_config::Repository]) -> Self {
        Self(
            repos
                .iter()
                .map(|repo| {
                    (
                        (repo.owner.to_ascii_lowercase(), repo.name.to_ascii_lowercase(), repo.installation_id),
                        (repo.container_image.clone(), repo.exec_scratch),
                    )
                })
                .collect(),
        )
    }

    pub(super) fn exec_scratch(&self, repo: &Repository) -> bool {
        self.0.get(&(
            repo.owner.to_ascii_lowercase(),
            repo.name.to_ascii_lowercase(),
            repo.installation,
        )).is_some_and(|(_, enabled)| *enabled)
    }

    pub(super) fn for_repository<'a>(&'a self, repo: &Repository, fallback: &'a str) -> &'a str {
        self.0
            .get(&(
                repo.owner.to_ascii_lowercase(),
                repo.name.to_ascii_lowercase(),
                repo.installation,
            ))
            .and_then(|(image, _)| image.as_deref())
            .unwrap_or(fallback)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::branches::Branches;

    #[test]
    fn reopening_adapter_settings_uses_current_config_by_identity() {
        let fixture = crate::git_tests::Repo::new();
        let delivery = fixture.delivery();
        let mut config =
            tl3_config::Config::parse(include_str!("../../../config/example.toml")).unwrap();
        let previous_branches = Branches::new(&config.github.repos).unwrap();
        let previous_images = Images::new(&config.github.repos);
        assert_eq!(
            previous_branches.base(&delivery.repository).unwrap(),
            "main"
        );
        assert_eq!(
            previous_images.for_repository(&delivery.repository, "global"),
            "global"
        );

        let image = format!("sha256:{}", "1".repeat(64));
        let repo = &mut config.github.repos[0];
        repo.owner = repo.owner.to_ascii_uppercase();
        repo.name = repo.name.to_ascii_uppercase();
        repo.branch_template = "fresh-{issue}-{node}".into();
        repo.base_branch = "release".into();
        repo.container_image = Some(image.clone());
        let mut other = repo.clone();
        other.name = "another-repository".into();
        other.branch_template = "other-{issue}-{node}".into();
        other.base_branch = "other-base".into();
        other.container_image = Some(format!("sha256:{}", "2".repeat(64)));
        config.github.repos.insert(0, other);

        let current_branches = Branches::new(&config.github.repos).unwrap();
        let current_images = Images::new(&config.github.repos);
        assert_eq!(
            current_branches.branch(&delivery).unwrap().as_str(),
            "fresh-42-0"
        );
        assert_eq!(
            current_branches.base(&delivery.repository).unwrap(),
            "release"
        );
        assert_eq!(
            current_images.for_repository(&delivery.repository, "global"),
            image
        );
        assert_ne!(
            previous_branches.branch(&delivery).unwrap(),
            current_branches.branch(&delivery).unwrap()
        );
    }
}
