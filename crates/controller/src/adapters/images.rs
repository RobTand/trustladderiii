use crate::Repository;
use std::collections::BTreeMap;

#[derive(Clone, Default)]
pub(super) struct Images(BTreeMap<(String, String, u64), String>);

impl Images {
    pub(super) fn new(repos: &[tl3_config::Repository]) -> Self {
        Self(
            repos
                .iter()
                .filter_map(|repo| {
                    repo.container_image.as_ref().map(|image| {
                        (
                            (
                                repo.owner.to_ascii_lowercase(),
                                repo.name.to_ascii_lowercase(),
                                repo.installation_id,
                            ),
                            image.clone(),
                        )
                    })
                })
                .collect(),
        )
    }

    pub(super) fn for_repository<'a>(&'a self, repo: &Repository, fallback: &'a str) -> &'a str {
        self.0
            .get(&(
                repo.owner.to_ascii_lowercase(),
                repo.name.to_ascii_lowercase(),
                repo.installation,
            ))
            .map(String::as_str)
            .unwrap_or(fallback)
    }
}
