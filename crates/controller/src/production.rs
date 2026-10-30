use crate::{Result, adapters, executor::Registry, ingress::Webhooks};

pub struct Registration {
    pub adapters: Registry,
    pub webhooks: Webhooks,
    pub workers: adapters::workers::WorkerOutputSource,
    pub bell: adapters::prismabuild::Source,
    pub mergify: adapters::mergify::Translator,
}

pub fn register(_config: &tl3_config::Config) -> Result<Registration> {
    todo!("production registration")
}
