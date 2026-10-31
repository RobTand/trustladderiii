use crate::{
    Bootstrap, Result,
    cli::Input,
    ingress::{GitHubDelivery, GitHubTranslator},
};
use std::collections::BTreeMap;
use tl3_github::GitHub;

pub struct Translator;
impl Translator {
    pub fn new(_clients: BTreeMap<u64, GitHub>, _bootstrap: Bootstrap) -> Result<Self> {
        Ok(Self)
    }
}
impl GitHubTranslator for Translator {
    fn translate(
        &mut self,
        _delivery: &GitHubDelivery,
        _snapshot: &tl3_store::Snapshot,
    ) -> Result<Vec<Input>> {
        todo!("Q45 labelled pull intake")
    }
}
