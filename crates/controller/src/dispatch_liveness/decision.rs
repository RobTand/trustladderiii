use tl3_kernel::Wait;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProcessEvidence {
    Live,
    Ended,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeliveryClass {
    Worker,
    Once,
    Converges,
    AtLeastOnce,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DispatchStatus {
    Dispatched(DeliveryClass),
    Ending,
    Retry,
    Incident,
    Paused,
    PoolWait,
    Answer,
    MergeQueue,
    SlotOrFiles,
    Prerequisite,
    Closed,
    Silent,
}

#[derive(Clone, Copy, Debug)]
pub struct Evidence {
    pub class: DeliveryClass,
    pub intent: bool,
    pub watch: bool,
    pub process: ProcessEvidence,
    pub ending: bool,
    pub deadline: bool,
    pub incident: bool,
    pub paused: bool,
    pub ticket: bool,
    pub obstruction: bool,
    pub question: bool,
    pub external: bool,
    pub closed: bool,
}

/// #817 / state-table waits: pending implementation, exercised by the red commit.
pub fn classify(_wait: Wait, _evidence: Evidence) -> DispatchStatus {
    DispatchStatus::Silent
}
