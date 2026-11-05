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
        facts: Option<QuotaFacts>,
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

/// State-table cdacce8f docs/state-table.md:277,787,800; Q43/Q53;
/// #759 and reply-shape incidents #682/#691/#696. Total over every event kind.
/// The base checks equality with the last line, not its index. Preserve that
/// distinction for duplicate quota bytes; changing it requires a separate fix.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
    out == if reply_seen { ReplyDecision::AfterReply } else { match event {
        WorkerEvent::Progress { .. } => ReplyDecision::Progress,
        WorkerEvent::Reply => ReplyDecision::Reply,
        WorkerEvent::Invalid => ReplyDecision::InvalidEvent,
        WorkerEvent::Quota { model, reset_ms, .. } => {
            if model == QuotaModel::Forbidden { ReplyDecision::InvalidQuotaModel }
            else if match reset_ms { Some(n) => n == 0, None => true } { ReplyDecision::InvalidQuotaReset }
            else if !last_line_equal { ReplyDecision::AfterQuota }
            else { ReplyDecision::Quota }
        },
    } },
))]
pub fn reply_step(event: WorkerEvent, reply_seen: bool, last_line_equal: bool) -> ReplyDecision {
    if reply_seen {
        return ReplyDecision::AfterReply;
    }
    match event {
        WorkerEvent::Progress { .. } => ReplyDecision::Progress,
        WorkerEvent::Reply => ReplyDecision::Reply,
        WorkerEvent::Invalid => ReplyDecision::InvalidEvent,
        WorkerEvent::Quota {
            model, reset_ms, ..
        } => {
            if matches!(model, QuotaModel::Forbidden) {
                return ReplyDecision::InvalidQuotaModel;
            }
            match reset_ms {
                Some(n) if n > 0 => (),
                _ => return ReplyDecision::InvalidQuotaReset,
            }
            if !last_line_equal {
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

/// Q53 / #759: total probe table over every typed HTTP status.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
    out == (if status == 400 || status == 404 {
        if unroutable { 900u64 } else { 0u64 }
    } else if status == 503 || (status == 429 && stale) { 120u64 }
    else if status == 429 { 900u64 } else { 0u64 }),
))]
pub fn probe(status: i64, unroutable: bool, stale: bool) -> u64 {
    if status == 400 || status == 404 {
        if unroutable { 900 } else { 0 }
    } else if status == 503 || (status == 429 && stale) {
        120
    } else if status == 429 {
        900
    } else {
        0
    }
}

/// Q53: eligibility and ceil of a finite, normalized binary rational in milliseconds.
/// The parser supplies its integer part and whether its exact remainder is nonzero.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
    (out.is_some() <==> status == 429 && finite && future && below_limit
        && (whole < u64::MAX || !fraction)),
    out.is_some() ==> out.unwrap() as int == whole as int + if fraction { 1int } else { 0int },
))]
pub fn announced(
    status: i64,
    finite: bool,
    future: bool,
    below_limit: bool,
    whole: u64,
    fraction: bool,
) -> Option<u64> {
    if status != 429 || !finite || !future || !below_limit {
        return None;
    }
    if fraction {
        if whole == u64::MAX {
            None
        } else {
            Some(whole + 1)
        }
    } else {
        Some(whole)
    }
}

/// The exact worker binary64 sample and its normalized probe deadlines.
/// Binary64 arithmetic and header parsing belong to the untrusted boundary.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct QuotaSample {
    pub bits: u64,
    pub short_ms: Option<u64>,
    pub long_ms: Option<u64>,
}

/// One exception link, in original chain order. Comparisons and fractional
/// milliseconds are normalized at the parser boundary, never from a new clock.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct QuotaFacts {
    pub sampled_now: QuotaSample,
    pub response_status: i64,
    pub finite: bool,
    pub future: bool,
    pub below_limit: bool,
    pub whole: u64,
    pub fraction: bool,
    pub stale: bool,
    pub postmortem: bool,
    pub judge: bool,
    pub api_status: Option<i64>,
    pub api_unroutable: bool,
    pub judge_status: Option<i64>,
    pub judge_unroutable: bool,
}

/// Q53/#759: announced reset precedes the total probe table for each link.
/// The boundary iterates at most eight links, preserving exception precedence.
pub fn quota_reset(facts: QuotaFacts) -> Option<u64> {
    if let Some(reset) = announced(
        facts.response_status,
        facts.finite,
        facts.future,
        facts.below_limit,
        facts.whole,
        facts.fraction,
    ) {
        return Some(reset);
    }
    let (status, unroutable) = if facts.postmortem && facts.judge {
        (facts.judge_status, facts.judge_unroutable)
    } else {
        (facts.api_status, facts.api_unroutable)
    };
    match probe(status?, unroutable, facts.stale) {
        120 => facts.sampled_now.short_ms,
        900 => facts.sampled_now.long_ms,
        _ => None,
    }
}
