//! Counts-only store diagnostics for #783.
use serde::Serialize;
#[derive(Default, Debug, Serialize, PartialEq, Eq)]
pub struct Stats {
    pub world_rows: std::collections::BTreeMap<String, i64>,
    pub events: i64,
    pub min_event_revision: Option<i64>,
    pub max_event_revision: Option<i64>,
    pub min_world_version: Option<i64>,
    pub replay_from_revision: Option<i64>,
}
pub fn load(_reader: &mut tl3_store::Reader) -> crate::Result<Stats> {
    Ok(Stats::default())
}
