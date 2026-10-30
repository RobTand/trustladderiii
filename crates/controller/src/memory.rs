use crate::Result;
use std::path::Path;
use tl3_store::Store;

pub fn queue_attempt(_store: &mut Store, _key: &str, _attempt: &Path) -> Result<()> {
    todo!("durably queue a bounded run summary")
}

pub fn flush(_store: &mut Store, _settings: &tl3_config::Hindsight) -> Result<usize> {
    todo!("retain queued runs using Hindsight operation identities")
}
