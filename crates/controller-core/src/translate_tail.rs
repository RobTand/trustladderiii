//! Finding selection after translate's variant match. Wire parsing, report
//! formatting, material/document reads, diagnostics and row writes stay in the shell.
#[cfg(verus_keep_ghost)]
use vstd::prelude::*;

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Checking,
    Reviewing,
    Making,
    Queued,
    Other,
}
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Node {
    pub phase: Phase,
    pub generation: u64,
    pub run: Option<u64>,
    pub job: Option<u64>,
}
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reply {
    ChecksDone {
        report: bool,
        pass: bool,
        in_footprint: bool,
    },
    ChangesRequested,
    MakerCantMeet,
    Other,
}
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Input {
    Reply { generation: u64, reply: Reply },
    CiFailed { infra: bool },
    Other,
}
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Finding {
    None,
    Check { pass: bool, in_footprint: bool },
    Review,
    Maker,
    Ci,
}

/// Base f5b0ba2f, docs/state-table.md:139 defines current-operation staleness;
/// :489-490, :507-508, :519, :574 describe the four failure sources, and
/// :560-566 require feedback in later rounds/attempts. The exact finding
/// selection, run-or-job admission, report flag and detailed-review suppression
/// are preserved existing behaviour (no rule), lib.rs:3844-3889.
/// Total over every input and u64 generation; no loop or finite proof bound.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
    out == match node {
        None => Finding::None,
        Some(n) => match input {
            Input::Reply { generation, reply } =>
                if (n.run == Some(generation) || n.job == Some(generation)) && n.generation == generation {
                    match (n.phase, reply) {
                        (Phase::Checking, Reply::ChecksDone { report, pass, in_footprint }) =>
                            if report && (!pass || !in_footprint) { Finding::Check { pass, in_footprint } } else { Finding::None },
                        (Phase::Reviewing, Reply::ChangesRequested) =>
                            if !detailed_review { Finding::Review } else { Finding::None },
                        (Phase::Making, Reply::MakerCantMeet) => Finding::Maker,
                        _ => Finding::None,
                    }
                } else { Finding::None },
            Input::CiFailed { infra } =>
                if !infra && n.phase == Phase::Queued { Finding::Ci } else { Finding::None },
            Input::Other => Finding::None,
        },
    },
))]
pub fn finding(node: Option<Node>, input: Input, detailed_review: bool) -> Finding {
    let Some(n) = node else {
        return Finding::None;
    };
    match input {
        Input::Reply { generation, reply }
            if (n.run == Some(generation) || n.job == Some(generation))
                && n.generation == generation =>
        {
            match (n.phase, reply) {
                (
                    Phase::Checking,
                    Reply::ChecksDone {
                        report: true,
                        pass,
                        in_footprint,
                    },
                ) if !pass || !in_footprint => Finding::Check { pass, in_footprint },
                (Phase::Reviewing, Reply::ChangesRequested) if !detailed_review => Finding::Review,
                (Phase::Making, Reply::MakerCantMeet) => Finding::Maker,
                _ => Finding::None,
            }
        }
        Input::CiFailed { infra: false } if n.phase == Phase::Queued => Finding::Ci,
        _ => Finding::None,
    }
}
