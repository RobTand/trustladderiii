//! H19: typed decisions, with I/O retained in the PrismaBuild adapter.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Record {
    Absent,
    Moving,
    Live,
    Ended,
}

/// State-table delivery row: Submit a PrismaBuild job, Once by action key.
pub fn publish(_record: Record) -> bool {
    todo!("CC-P publication decision")
}
