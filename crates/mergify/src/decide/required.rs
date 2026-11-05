//! Required-check decisions. Parsing and the I/O schedule belong to the adapter.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Presence {
    Missing,
    Reported,
    Unavailable,
}

pub fn presence(reports: bool, expects_app: bool, statuses_available: bool) -> Presence {
    let _ = (reports, expects_app, statuses_available);
    todo!("CC-M3 required-check core")
}
