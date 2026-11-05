//! H1/H2/H6 decisions over samples taken by the Mergify adapter.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Receipt {
    Confirmed,
    Other,
    Pending,
    Absent,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReceiptStage {
    Initial,
    Final,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReceiptAction {
    ReadRecord,
    SaveRecord,
    Return,
    Prepare,
    Unknown,
}

pub fn receipt(stage: ReceiptStage, read: Receipt, recorded: Option<bool>) -> ReceiptAction {
    let _ = (stage, read, recorded);
    todo!("CC-M1 receipt extraction")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AdmissionQueue {
    Merged,
    Active,
    Terminal,
    Other,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AckRead {
    pub current: bool,
    pub merged: bool,
    pub conflicts: bool,
    pub confirmed: bool,
    pub person_removed: Option<bool>,
    pub queue: AdmissionQueue,
    pub no_requeue: bool,
    pub check_changed: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AckDecision {
    Retryable,
    Conflict,
    ReadPerson,
    Confirmed,
    ReadIssue,
}

pub fn acknowledgement(read: AckRead) -> AckDecision {
    let _ = read;
    todo!("CC-M1 acknowledgement extraction")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DequeueProbe {
    Clear,
    Pending,
    Events,
}

pub fn dequeue_probe(retry: bool, concluded: bool, labeled: bool) -> DequeueProbe {
    let _ = (retry, concluded, labeled);
    todo!("CC-M1 dequeue extraction")
}
