//! H16/C4 worker-output decisions. JSON and reply validators stay in the shell.
#[cfg(verus_keep_ghost)]
use vstd::prelude::*;

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProgressUnit {
    ModelTurn,
    ToolResult,
    ExternalWait,
}

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModelField {
    Missing,
    GradeModel,
    Other,
}
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QuotaModel {
    Implicit,
    Grade,
    Forbidden,
}

/// State-table cdacce8f docs/state-table.md:277,787; Q53, #759.
/// An explicit charge is permitted only for the postmortem grade model.
/// Missing model remains implicit, as on the base; this is not a new Q27 rule.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
    out == match field {
        ModelField::Missing => QuotaModel::Implicit,
        ModelField::GradeModel => if postmortem { QuotaModel::Grade } else { QuotaModel::Forbidden },
        ModelField::Other => QuotaModel::Forbidden,
    },
))]
pub fn quota_model(postmortem: bool, field: ModelField) -> QuotaModel {
    match field {
        ModelField::Missing => QuotaModel::Implicit,
        ModelField::GradeModel if postmortem => QuotaModel::Grade,
        ModelField::GradeModel | ModelField::Other => QuotaModel::Forbidden,
    }
}

/// A plain domain enum, independent of serde and of the eventual contracts crate.
/// Reply bodies, wait keys and model names remain shell-owned payloads.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkerEvent {
    Progress {
        unit: ProgressUnit,
    },
    Reply,
    Quota {
        model: QuotaModel,
        reset_ms: Option<u64>,
    },
    Invalid,
}
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReplyDecision {
    Progress,
    Reply,
    Quota,
    AfterReply,
    InvalidEvent,
    InvalidQuotaModel,
    InvalidQuotaReset,
    AfterQuota,
}

/// Q53, docs/state-table.md:277,787 on d8f58cc3 define quota/model handling;
/// #759 defines the postmortem grade-model charge. Q43 and :800 define progress.
/// Positional finality enforces source.rs parse_reply's existing terminal-quota
/// rule: a quota ends at output_end, even if later event bytes are identical.
/// Other validation precedence is preserved existing behaviour (no rule).
/// Total for all events and full-width positions, without a stream-length bound.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
    out == if reply_seen { ReplyDecision::AfterReply } else { match event {
        WorkerEvent::Progress { .. } => ReplyDecision::Progress,
        WorkerEvent::Reply => ReplyDecision::Reply,
        WorkerEvent::Invalid => ReplyDecision::InvalidEvent,
        WorkerEvent::Quota { model, reset_ms } => {
            if model == QuotaModel::Forbidden { ReplyDecision::InvalidQuotaModel }
            else if match reset_ms { Some(n) => n == 0, None => true } { ReplyDecision::InvalidQuotaReset }
            else if line_end != output_end { ReplyDecision::AfterQuota }
            else { ReplyDecision::Quota }
        },
    } },
))]
pub fn reply_step(
    event: WorkerEvent,
    reply_seen: bool,
    line_end: usize,
    output_end: usize,
) -> ReplyDecision {
    if reply_seen {
        return ReplyDecision::AfterReply;
    }
    match event {
        WorkerEvent::Progress { .. } => ReplyDecision::Progress,
        WorkerEvent::Reply => ReplyDecision::Reply,
        WorkerEvent::Invalid => ReplyDecision::InvalidEvent,
        WorkerEvent::Quota { model, reset_ms } => {
            if matches!(model, QuotaModel::Forbidden) {
                return ReplyDecision::InvalidQuotaModel;
            }
            match reset_ms {
                Some(n) if n > 0 => (),
                _ => return ReplyDecision::InvalidQuotaReset,
            }
            if line_end != output_end {
                return ReplyDecision::AfterQuota;
            }
            ReplyDecision::Quota
        }
    }
}

/// State-table cdacce8f docs/state-table.md:147,800; Q43, #682/#691/#696.
/// All offsets are quantified, with no sequence bound. Replays at or behind
/// the durable offset cannot produce a write or send.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
    out == (matches!(event, WorkerEvent::Progress { .. }) && offset > recorded),
))]
pub fn progress_due(event: WorkerEvent, offset: u64, recorded: u64) -> bool {
    matches!(event, WorkerEvent::Progress { .. }) && offset > recorded
}
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProgressSignal {
    None,
    Look,
    Progress,
}

/// State-table cdacce8f docs/state-table.md:800; Q43, #682/#691/#696.
/// Only a changed external wait sends Look. Ordinary work sends Progress even
/// when it clears a previous wait, exactly as the base shell does.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
    out == if waiting { if changed { ProgressSignal::Look } else { ProgressSignal::None } }
        else { ProgressSignal::Progress },
))]
pub fn progress_signal(waiting: bool, changed: bool) -> ProgressSignal {
    if waiting {
        if changed {
            ProgressSignal::Look
        } else {
            ProgressSignal::None
        }
    } else {
        ProgressSignal::Progress
    }
}

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Claim {
    Missing,
    Cancelled,
    Live,
}
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClaimDecision {
    Retire,
    AwaitClaim,
    ReadOutput,
}
/// State-table cdacce8f docs/state-table.md:143,800,812; Q43, #682/#691/#696.
/// Each caller passes only the claim sampled at its original boundary.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
    out == match claim { Claim::Missing => ClaimDecision::AwaitClaim,
        Claim::Cancelled => ClaimDecision::Retire, Claim::Live => ClaimDecision::ReadOutput },
))]
pub fn claim_decision(claim: Claim) -> ClaimDecision {
    match claim {
        Claim::Missing => ClaimDecision::AwaitClaim,
        Claim::Cancelled => ClaimDecision::Retire,
        Claim::Live => ClaimDecision::ReadOutput,
    }
}
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RefreshGate {
    Retire,
    Inspect,
}
/// State-table cdacce8f docs/state-table.md:143,812; Q43, #682/#691/#696.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
    out == if reconcilable { RefreshGate::Inspect } else { RefreshGate::Retire },
))]
pub fn refresh_gate(reconcilable: bool) -> RefreshGate {
    if reconcilable {
        RefreshGate::Inspect
    } else {
        RefreshGate::Retire
    }
}
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ending {
    Found,
    Absent,
    Pending,
}
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EndDecision {
    Complete,
    Retire,
    KeepActive,
}
/// State-table cdacce8f docs/state-table.md:143,800,812; Q43, #682/#691/#696.
/// Only an upstream ending retires an attempt; silence never supplies one.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
    out == match ending { Ending::Found => EndDecision::Complete,
        Ending::Absent => EndDecision::Retire, Ending::Pending => EndDecision::KeepActive },
))]
pub fn end_decision(ending: Ending) -> EndDecision {
    match ending {
        Ending::Found => EndDecision::Complete,
        Ending::Absent => EndDecision::Retire,
        Ending::Pending => EndDecision::KeepActive,
    }
}
