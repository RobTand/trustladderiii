//! Read-only replay verification for #783.
use serde::{Deserialize, Serialize};
use tl3_kernel::{Event, World};
use tl3_store::EventRecord;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StepRecord {
    pub input: World,
    pub event: Event,
    pub after: World,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReplayRecord {
    pub before: World,
    pub steps: Vec<StepRecord>,
    pub after: World,
}

pub fn check_record(
    _record: &EventRecord,
    _previous: Option<&World>,
) -> Result<Option<World>, String> {
    Ok(None)
}
