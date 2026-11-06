use crate::{Repository, Result};

/// Select the delivery event translator using the persisted repository configuration.
pub struct Translator {
    queue: super::mergify::Translator,
    direct: super::direct_merge::Translator,
}
impl Translator {
    pub fn new(clients: std::collections::BTreeMap<u64, tl3_github::GitHub>) -> Result<Self> {
        Ok(Self {
            queue: super::mergify::Translator::new(clients.clone())?,
            direct: super::direct_merge::Translator::new(clients)?,
        })
    }
    pub fn with_wakes(mut self, wakes: super::direct_merge::Wakes) -> Self {
        self.direct = self.direct.with_wakes(wakes);
        self
    }
}
impl crate::ingress::GitHubTranslator for Translator {
    fn translate(
        &mut self,
        delivery: &crate::ingress::GitHubDelivery,
        snapshot: &tl3_store::Snapshot,
    ) -> Result<Vec<crate::cli::Input>> {
        let repository = match delivery {
            crate::ingress::GitHubDelivery::CatchUp { repository, .. } => Some(repository.clone()),
            crate::ingress::GitHubDelivery::Webhook(webhook) => {
                let value = &webhook.payload;
                value["repository"]["owner"]["login"]
                    .as_str()
                    .zip(value["repository"]["name"].as_str())
                    .zip(value["installation"]["id"].as_u64())
                    .map(|((owner, name), installation)| Repository {
                        owner: owner.into(),
                        name: name.into(),
                        installation,
                    })
            }
        };
        if let Some(repository) = repository
            && super::direct_merge::repository_config::direct(snapshot, &repository)?
        {
            self.direct.translate(delivery, snapshot)
        } else {
            self.queue.translate(delivery, snapshot)
        }
    }
}
